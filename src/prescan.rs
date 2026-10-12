//! Scans videos ahead of time, without a window and without splitting
//! anything. The results go into the scan cache, so the window later opens
//! the same videos ready for review.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;

use crate::cache::{self, CacheUse, DetectionSource};
use crate::detect::{Detection, ScanDepth, ScanSettings};
use crate::library;

#[derive(Debug)]
pub struct FailedScan {
    pub video: PathBuf,
    pub reason: String,
}

/// What a run of scanning ahead did.
#[derive(Debug, Default)]
pub struct ScanAheadSummary {
    pub freshly_scanned: usize,
    pub already_stored: usize,
    pub failed: Vec<FailedScan>,
}

impl ScanAheadSummary {
    /// True when there was nothing to scan.
    pub fn is_empty(&self) -> bool {
        self.freshly_scanned == 0 && self.already_stored == 0 && self.failed.is_empty()
    }

    pub fn describe(&self) -> String {
        format!(
            "{} scanned, {} already scanned, {} failed.",
            self.freshly_scanned,
            self.already_stored,
            self.failed.len()
        )
    }
}

/// Scans the video at `path`, or every video directly inside that folder.
/// One video failing does not stop the rest. Fails outright only when the
/// path itself cannot be read.
pub fn scan_ahead(
    path: &Path,
    settings: &ScanSettings,
    depth: ScanDepth,
) -> Result<ScanAheadSummary> {
    let videos = library::videos_at(path)?;
    let mut summary = ScanAheadSummary::default();

    for (position, video) in videos.iter().enumerate() {
        let name = video.file_name().unwrap_or(video.as_os_str());
        let progress = format!(
            "[{}/{}] {}",
            position + 1,
            videos.len(),
            name.to_string_lossy()
        );

        let started = Instant::now();
        match cache::detect_with_cache(video, settings, CacheUse::ReuseStoredScan, depth) {
            Ok(sourced) => {
                let found = describe_detection(&sourced.detection);
                match sourced.source {
                    DetectionSource::StoredScan => {
                        summary.already_stored += 1;
                        println!("{progress}: {found}, already scanned");
                    }
                    DetectionSource::FreshScan => {
                        summary.freshly_scanned += 1;
                        let seconds = started.elapsed().as_secs_f64();
                        println!("{progress}: {found}, scanned in {seconds:.1}s");
                    }
                }
            }
            Err(error) => {
                println!("{progress}: failed, {error:#}");
                summary.failed.push(FailedScan {
                    video: video.clone(),
                    reason: format!("{error:#}"),
                });
            }
        }
    }
    Ok(summary)
}

fn describe_detection(detection: &Detection) -> String {
    let black_screens = format!("black screens: {}", detection.black_screens.len());
    match &detection.hard_cut_candidates {
        Some(candidates) => format!(
            "{black_screens}, abrupt picture changes: {}",
            candidates.len()
        ),
        None => black_screens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::black::TimeSpan;
    use crate::hardcut::HardCutCandidate;

    fn detection_with(hard_cut_candidates: Option<Vec<HardCutCandidate>>) -> Detection {
        Detection {
            video: TimeSpan {
                start: 0.0,
                end: 100.0,
            },
            keyframe_times: vec![0.0],
            frame_count: 3000,
            black_screens: vec![TimeSpan {
                start: 10.0,
                end: 11.0,
            }],
            cuts: vec![10.0],
            decoded_frames: 40,
            decoded_windows: 1,
            hard_cut_candidates,
        }
    }

    #[test]
    fn a_normal_scan_reports_its_black_screens() {
        assert_eq!(
            describe_detection(&detection_with(None)),
            "black screens: 1"
        );
    }

    #[test]
    fn a_hard_cut_scan_also_reports_the_picture_changes() {
        let candidates = vec![HardCutCandidate {
            time: 50.0,
            strength: 80,
        }];
        assert_eq!(
            describe_detection(&detection_with(Some(candidates))),
            "black screens: 1, abrupt picture changes: 1"
        );
    }

    #[test]
    fn a_path_that_does_not_exist_fails_outright() {
        let settings = ScanSettings::default();
        let result = scan_ahead(
            Path::new("no/such/folder"),
            &settings,
            ScanDepth::BlackScreens,
        );
        assert!(result.is_err());
    }

    #[test]
    fn the_summary_counts_each_outcome() {
        let summary = ScanAheadSummary {
            freshly_scanned: 3,
            already_stored: 2,
            failed: vec![FailedScan {
                video: PathBuf::from("broken.mp4"),
                reason: "not a video".to_owned(),
            }],
        };
        assert_eq!(
            summary.describe(),
            "3 scanned, 2 already scanned, 1 failed."
        );
    }
}
