//! Remembers scan results on disk, so a video scanned once is ready at once
//! the next time the app is opened.
//!
//! A stored scan is only reused when the video's path, size and modified
//! time, and the scan settings, are all the same as when it was made.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::detect::{self, Detection, OnScanProgress, ScanDepth, ScanSettings};
use crate::scan::UndecodedFrames;

/// Raised whenever the stored layout changes, or detection changes in a way
/// that would give different results for the same settings, so older files
/// are ignored.
const STORED_SCAN_VERSION: u32 = 1;
const APP_FOLDER: &str = "scene_splitter";
const SCANS_FOLDER: &str = "scans";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CacheUse {
    ReuseStoredScan,
    /// Scan the video again and replace whatever was stored for it.
    ScanFresh,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DetectionSource {
    FreshScan,
    StoredScan,
}

#[derive(Debug)]
pub struct SourcedDetection {
    pub detection: Detection,
    pub source: DetectionSource,
    /// Stretches a fresh scan could not decode, so scene changes in them may
    /// have been missed. Empty for a stored scan, which does not keep them.
    pub undecoded: Vec<UndecodedFrames>,
}

/// Identifies one version of a video file without reading its contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct VideoFingerprint {
    path: PathBuf,
    size_bytes: u64,
    modified_unix_millis: u128,
}

impl VideoFingerprint {
    fn of(video: &Path) -> Result<Self> {
        let path = std::fs::canonicalize(video)
            .with_context(|| format!("could not find {}", video.display()))?;
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("could not read the details of {}", video.display()))?;
        let modified = metadata
            .modified()
            .with_context(|| format!("{} has no modified time", video.display()))?;
        // A file dated before 1970 simply counts as modified at time zero.
        let modified_unix_millis = modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_millis());
        Ok(Self {
            path,
            size_bytes: metadata.len(),
            modified_unix_millis,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredScan {
    version: u32,
    fingerprint: VideoFingerprint,
    settings: ScanSettings,
    detection: Detection,
}

/// Scans the video, or returns the stored scan when one still applies.
/// `on_progress` only hears from a scan that really runs.
pub fn detect_with_cache(
    video: &Path,
    settings: &ScanSettings,
    cache_use: CacheUse,
    depth: ScanDepth,
    on_progress: OnScanProgress,
) -> Result<SourcedDetection> {
    let fingerprint = VideoFingerprint::of(video)?;
    let scans_dir = scans_dir();

    if cache_use == CacheUse::ReuseStoredScan {
        // A stored scan that never looked for hard cuts cannot answer a
        // request for them.
        let stored = load_stored_scan(&scans_dir, &fingerprint, settings)
            .filter(|detection| detection.satisfies(depth));
        if let Some(detection) = stored {
            return Ok(SourcedDetection {
                detection,
                source: DetectionSource::StoredScan,
                undecoded: Vec::new(),
            });
        }
    }

    let scan = detect::scan_for_scene_changes(video, settings, depth, on_progress)?;
    let mut detection = scan.detection;
    if detection.hard_cut_candidates.is_none() {
        // The hard-cut pass takes minutes and does not depend on the
        // black-screen settings, so what an earlier scan of this same file
        // found is kept.
        detection.hard_cut_candidates = read_stored_scan(&scans_dir, &fingerprint)
            .and_then(|stored| stored.detection.hard_cut_candidates);
    }
    // The scan itself succeeded, so failing to remember it only costs a
    // rescan next time.
    if let Err(error) = store_scan(&scans_dir, &fingerprint, settings, &detection) {
        eprintln!("Could not store the scan of {}: {error:#}", video.display());
    }
    Ok(SourcedDetection {
        detection,
        source: DetectionSource::FreshScan,
        undecoded: scan.undecoded,
    })
}

/// The system's temporary folder, which on Windows is the user's
/// AppData\Local\Temp. If the system clears it, the only cost is scanning
/// those videos again.
fn scans_dir() -> PathBuf {
    std::env::temp_dir().join(APP_FOLDER).join(SCANS_FOLDER)
}

/// Returns `None` when nothing usable is stored. A missing, unreadable or
/// outdated file all mean the same thing here: scan the video.
fn load_stored_scan(
    scans_dir: &Path,
    fingerprint: &VideoFingerprint,
    settings: &ScanSettings,
) -> Option<Detection> {
    let stored = read_stored_scan(scans_dir, fingerprint)?;
    (stored.settings == *settings).then_some(stored.detection)
}

/// What is stored for this exact file, whatever settings it was scanned
/// with.
fn read_stored_scan(scans_dir: &Path, fingerprint: &VideoFingerprint) -> Option<StoredScan> {
    let json = std::fs::read_to_string(stored_scan_path(scans_dir, fingerprint)).ok()?;
    let stored: StoredScan = serde_json::from_str(&json).ok()?;
    let same_file = stored.version == STORED_SCAN_VERSION && stored.fingerprint == *fingerprint;
    same_file.then_some(stored)
}

fn store_scan(
    scans_dir: &Path,
    fingerprint: &VideoFingerprint,
    settings: &ScanSettings,
    detection: &Detection,
) -> Result<()> {
    std::fs::create_dir_all(scans_dir)
        .with_context(|| format!("could not create {}", scans_dir.display()))?;
    let stored = StoredScan {
        version: STORED_SCAN_VERSION,
        fingerprint: fingerprint.clone(),
        settings: *settings,
        detection: detection.clone(),
    };
    let json = serde_json::to_string(&stored).context("could not encode the scan")?;

    // Written under a temporary name first, so a crash mid-write cannot
    // leave a half-written file under the real name.
    let path = stored_scan_path(scans_dir, fingerprint);
    let unfinished_path = path.with_extension("json.part");
    std::fs::write(&unfinished_path, json)
        .with_context(|| format!("could not write {}", unfinished_path.display()))?;
    std::fs::rename(&unfinished_path, &path)
        .with_context(|| format!("could not write {}", path.display()))
}

/// One file per video path. The fingerprint inside the file decides whether
/// it still applies, the name only has to be stable.
fn stored_scan_path(scans_dir: &Path, fingerprint: &VideoFingerprint) -> PathBuf {
    let path_hash = fnv1a_64(fingerprint.path.to_string_lossy().as_bytes());
    scans_dir.join(format!("{path_hash:016x}.json"))
}

/// A small hash with a fixed definition. The standard library's hasher may
/// change between Rust versions, which would orphan every stored scan. The
/// split uses it too, to name a folder after a video without repeating the
/// video's name.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::black::BlackCriteria;
    use crate::detect::ScanMode;
    use crate::timecode::TimeSpan;

    /// A stored scan exactly as the version of 548a54e wrote it, for
    /// `fingerprint()`, `settings()` and `detection()` with one hard cut.
    const SCAN_STORED_BY_AN_EARLIER_VERSION: &str = concat!(
        r#"{"version":1,"#,
        r#""fingerprint":{"path":"E:\\Videos\\film.mp4","size_bytes":700000000,"#,
        r#""modified_unix_millis":1760000000000},"#,
        r#""settings":{"mode":"Suspicious","criteria":"#,
        r#"{"max_dark_luma":32,"min_dark_fraction":0.98,"min_seconds":0.25}},"#,
        r#""detection":{"video":{"start":0.0,"end":7200.0},"#,
        r#""keyframe_times":[0.0,2.0,3792.0],"frame_count":216000,"#,
        r#""black_screens":[{"start":3792.25,"end":3793.6}],"cuts":[3792.0],"#,
        r#""decoded_frames":1219,"decoded_windows":23,"#,
        r#""hard_cut_candidates":[{"time":601.5,"strength":77}]}}"#,
    );

    /// A scans folder of its own for one test, removed when the test ends.
    struct ScansDir(PathBuf);

    impl ScansDir {
        fn for_test(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("scene_splitter_test_{}_{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for ScansDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fingerprint() -> VideoFingerprint {
        VideoFingerprint {
            path: PathBuf::from(r"E:\Videos\film.mp4"),
            size_bytes: 700_000_000,
            modified_unix_millis: 1_760_000_000_000,
        }
    }

    fn settings() -> ScanSettings {
        ScanSettings {
            mode: ScanMode::Suspicious,
            criteria: BlackCriteria {
                max_dark_luma: 32,
                min_dark_fraction: 0.98,
                min_seconds: 0.25,
            },
        }
    }

    fn detection() -> Detection {
        Detection {
            video: TimeSpan {
                start: 0.0,
                end: 7200.0,
            },
            keyframe_times: vec![0.0, 2.0, 3792.0],
            frame_count: 216_000,
            black_screens: vec![TimeSpan {
                start: 3792.25,
                end: 3793.6,
            }],
            cuts: vec![3792.0],
            decoded_frames: 1219,
            decoded_windows: 23,
            hard_cut_candidates: None,
        }
    }

    #[test]
    fn a_stored_scan_comes_back_unchanged() {
        let dir = ScansDir::for_test("round_trip");
        store_scan(&dir.0, &fingerprint(), &settings(), &detection()).unwrap();
        assert_eq!(
            load_stored_scan(&dir.0, &fingerprint(), &settings()),
            Some(detection())
        );
    }

    #[test]
    fn hard_cut_candidates_are_stored_with_the_scan() {
        let dir = ScansDir::for_test("hard_cuts");
        let with_hard_cuts = Detection {
            hard_cut_candidates: Some(vec![crate::hardcut::HardCutCandidate {
                time: 601.5,
                strength: 77,
            }]),
            ..detection()
        };
        store_scan(&dir.0, &fingerprint(), &settings(), &with_hard_cuts).unwrap();

        let loaded = load_stored_scan(&dir.0, &fingerprint(), &settings()).unwrap();
        assert_eq!(loaded, with_hard_cuts);
        assert!(loaded.satisfies(ScanDepth::WithHardCuts));
        assert!(!detection().satisfies(ScanDepth::WithHardCuts));
    }

    #[test]
    fn a_scan_stored_by_an_earlier_version_still_loads() {
        let dir = ScansDir::for_test("earlier_version");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(
            stored_scan_path(&dir.0, &fingerprint()),
            SCAN_STORED_BY_AN_EARLIER_VERSION,
        )
        .unwrap();

        let as_stored = Detection {
            hard_cut_candidates: Some(vec![crate::hardcut::HardCutCandidate {
                time: 601.5,
                strength: 77,
            }]),
            ..detection()
        };
        assert_eq!(
            load_stored_scan(&dir.0, &fingerprint(), &settings()),
            Some(as_stored)
        );
    }

    #[test]
    fn a_scan_is_stored_in_the_layout_earlier_versions_wrote() {
        let dir = ScansDir::for_test("same_layout");
        let with_hard_cuts = Detection {
            hard_cut_candidates: Some(vec![crate::hardcut::HardCutCandidate {
                time: 601.5,
                strength: 77,
            }]),
            ..detection()
        };
        store_scan(&dir.0, &fingerprint(), &settings(), &with_hard_cuts).unwrap();

        let written = std::fs::read_to_string(stored_scan_path(&dir.0, &fingerprint())).unwrap();
        assert_eq!(written, SCAN_STORED_BY_AN_EARLIER_VERSION);
    }

    #[test]
    fn a_stored_scan_can_be_read_whatever_settings_made_it() {
        let dir = ScansDir::for_test("any_settings");
        store_scan(&dir.0, &fingerprint(), &settings(), &detection()).unwrap();

        let stored = read_stored_scan(&dir.0, &fingerprint()).unwrap();
        assert_eq!(stored.detection, detection());

        let edited = VideoFingerprint {
            size_bytes: 1,
            ..fingerprint()
        };
        assert!(read_stored_scan(&dir.0, &edited).is_none());
    }

    #[test]
    fn nothing_is_stored_for_a_video_never_scanned() {
        let dir = ScansDir::for_test("never_scanned");
        assert_eq!(load_stored_scan(&dir.0, &fingerprint(), &settings()), None);
    }

    #[test]
    fn a_video_that_changed_on_disk_is_scanned_again() {
        let dir = ScansDir::for_test("changed_video");
        store_scan(&dir.0, &fingerprint(), &settings(), &detection()).unwrap();

        let edited = VideoFingerprint {
            size_bytes: 1,
            ..fingerprint()
        };
        assert_eq!(load_stored_scan(&dir.0, &edited, &settings()), None);
    }

    #[test]
    fn different_scan_settings_do_not_reuse_the_stored_scan() {
        let dir = ScansDir::for_test("changed_settings");
        store_scan(&dir.0, &fingerprint(), &settings(), &detection()).unwrap();

        let thorough = ScanSettings {
            mode: ScanMode::EveryFrame,
            ..settings()
        };
        assert_eq!(load_stored_scan(&dir.0, &fingerprint(), &thorough), None);
    }

    #[test]
    fn a_damaged_file_counts_as_nothing_stored() {
        let dir = ScansDir::for_test("damaged");
        store_scan(&dir.0, &fingerprint(), &settings(), &detection()).unwrap();
        std::fs::write(stored_scan_path(&dir.0, &fingerprint()), "{ not json").unwrap();

        assert_eq!(load_stored_scan(&dir.0, &fingerprint(), &settings()), None);
    }

    #[test]
    fn storing_again_replaces_the_earlier_scan() {
        let dir = ScansDir::for_test("replace");
        store_scan(&dir.0, &fingerprint(), &settings(), &detection()).unwrap();

        let rescanned = Detection {
            cuts: Vec::new(),
            ..detection()
        };
        store_scan(&dir.0, &fingerprint(), &settings(), &rescanned).unwrap();
        assert_eq!(
            load_stored_scan(&dir.0, &fingerprint(), &settings()),
            Some(rescanned)
        );
    }
}
