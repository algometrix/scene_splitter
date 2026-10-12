//! Scans videos ahead of time, without a window and without splitting
//! anything. The results go into the scan cache, so the window later opens
//! the same videos ready for review.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;

use crate::cache::{self, CacheUse, DetectionSource};
use crate::detect::{self, Detection, ScanDepth, ScanSettings};
use crate::library;
use crate::scan::UndecodedFrames;
use crate::scene_name::SceneNames;

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

    fn count(&mut self, video: &Path, outcome: &VideoOutcome) {
        match outcome {
            VideoOutcome::AlreadyScanned { .. } => self.already_stored += 1,
            VideoOutcome::Scanned { .. } => self.freshly_scanned += 1,
            VideoOutcome::Failed { reason } => self.failed.push(FailedScan {
                video: video.to_path_buf(),
                reason: reason.clone(),
            }),
        }
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

/// How scanning one video ahead of time went.
#[derive(Debug, Clone, PartialEq)]
pub enum VideoOutcome {
    /// A stored scan still applied. `found` says what it holds.
    AlreadyScanned {
        found: String,
    },
    Scanned {
        found: String,
        seconds: f64,
    },
    Failed {
        reason: String,
    },
}

/// One video of a run is done.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoDone<'a> {
    /// Counted from one.
    pub position: usize,
    pub video_count: usize,
    pub video: &'a Path,
    pub outcome: VideoOutcome,
    /// Stretches of the video a fresh scan could not decode.
    pub undecoded: Vec<UndecodedFrames>,
}

impl fmt::Display for VideoDone<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.video.file_name().unwrap_or(self.video.as_os_str());
        write!(
            formatter,
            "[{}/{}] {}: ",
            self.position,
            self.video_count,
            name.to_string_lossy()
        )?;
        match &self.outcome {
            VideoOutcome::AlreadyScanned { found } => {
                write!(formatter, "{found}, already scanned")
            }
            VideoOutcome::Scanned { found, seconds } => {
                write!(formatter, "{found}, scanned in {seconds:.1}s")
            }
            VideoOutcome::Failed { reason } => write!(formatter, "failed, {reason}"),
        }
    }
}

/// Scans the video at `path`, or every video directly inside that folder,
/// and tells `on_video_done` about each one as it finishes. One video
/// failing does not stop the rest. Fails outright only when the path itself
/// cannot be read. In a folder, files that `scene_names` take for the scenes
/// of an earlier split are left out.
pub fn scan_ahead(
    path: &Path,
    scene_names: SceneNames,
    settings: &ScanSettings,
    depth: ScanDepth,
    mut on_video_done: impl FnMut(&VideoDone),
) -> Result<ScanAheadSummary> {
    let videos = library::videos_at(path, scene_names)?;
    let mut summary = ScanAheadSummary::default();

    for (position, video) in videos.iter().enumerate() {
        let started = Instant::now();
        // A run from the command line shows no progress bar.
        let scanned = cache::detect_with_cache(
            video,
            settings,
            CacheUse::ReuseStoredScan,
            depth,
            &detect::no_scan_progress,
        );
        let (outcome, undecoded) = match scanned {
            Ok(sourced) => {
                let found = describe_detection(&sourced.detection);
                let outcome = match sourced.source {
                    DetectionSource::StoredScan => VideoOutcome::AlreadyScanned { found },
                    DetectionSource::FreshScan => VideoOutcome::Scanned {
                        found,
                        seconds: started.elapsed().as_secs_f64(),
                    },
                };
                (outcome, sourced.undecoded)
            }
            Err(error) => (
                VideoOutcome::Failed {
                    reason: format!("{error:#}"),
                },
                Vec::new(),
            ),
        };
        summary.count(video, &outcome);
        on_video_done(&VideoDone {
            position: position + 1,
            video_count: videos.len(),
            video,
            outcome,
            undecoded,
        });
    }
    Ok(summary)
}

fn describe_detection(detection: &Detection) -> String {
    let mut found = format!("black screens: {}", detection.black_screens.len());
    if let Some(candidates) = &detection.hard_cut_candidates {
        found.push_str(&format!(", sudden picture changes: {}", candidates.len()));
    }
    // Most videos have no chapters, and then nothing is said about them.
    let chapter_mark_count = detection.chapter_marks().len();
    if chapter_mark_count > 0 {
        found.push_str(&format!(", chapter marks: {chapter_mark_count}"));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardcut::HardCutCandidate;
    use crate::timecode::TimeSpan;

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
            chapters: crate::detect::ChapterMarks::NeverRead,
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
            "black screens: 1, sudden picture changes: 1"
        );
    }

    #[test]
    fn a_video_with_chapters_reports_how_many_marks_it_has() {
        let with_chapters = Detection {
            chapters: crate::detect::ChapterMarks::Read(vec![
                crate::chapters::Chapter {
                    start: 0.0,
                    title: None,
                },
                crate::chapters::Chapter {
                    start: 50.0,
                    title: None,
                },
            ]),
            ..detection_with(None)
        };
        assert_eq!(
            describe_detection(&with_chapters),
            "black screens: 1, chapter marks: 2"
        );
    }

    #[test]
    fn a_path_that_does_not_exist_fails_outright() {
        let settings = ScanSettings::default();
        let result = scan_ahead(
            Path::new("no/such/folder"),
            SceneNames::from(crate::scene_name::SceneNamePattern::default()),
            &settings,
            ScanDepth::BlackScreens,
            |_video_done| {},
        );
        assert!(result.is_err());
    }

    fn video_done(outcome: VideoOutcome) -> String {
        VideoDone {
            position: 2,
            video_count: 5,
            video: Path::new("videos/holiday.mp4"),
            outcome,
            undecoded: Vec::new(),
        }
        .to_string()
    }

    #[test]
    fn a_stored_scan_is_reported_as_already_scanned() {
        let outcome = VideoOutcome::AlreadyScanned {
            found: "black screens: 3".to_owned(),
        };
        assert_eq!(
            video_done(outcome),
            "[2/5] holiday.mp4: black screens: 3, already scanned"
        );
    }

    #[test]
    fn a_fresh_scan_is_reported_with_how_long_it_took() {
        let outcome = VideoOutcome::Scanned {
            found: "black screens: 3".to_owned(),
            seconds: 4.26,
        };
        assert_eq!(
            video_done(outcome),
            "[2/5] holiday.mp4: black screens: 3, scanned in 4.3s"
        );
    }

    #[test]
    fn a_failed_scan_is_reported_with_its_reason() {
        let outcome = VideoOutcome::Failed {
            reason: "not a video".to_owned(),
        };
        assert_eq!(
            video_done(outcome),
            "[2/5] holiday.mp4: failed, not a video"
        );
    }

    #[test]
    fn each_outcome_is_counted_once() {
        let mut summary = ScanAheadSummary::default();
        let video = Path::new("holiday.mp4");
        let found = || "black screens: 0".to_owned();

        summary.count(video, &VideoOutcome::AlreadyScanned { found: found() });
        summary.count(
            video,
            &VideoOutcome::Scanned {
                found: found(),
                seconds: 1.0,
            },
        );
        summary.count(
            video,
            &VideoOutcome::Failed {
                reason: "not a video".to_owned(),
            },
        );

        assert_eq!(
            summary.describe(),
            "1 scanned, 1 already scanned, 1 failed."
        );
        assert_eq!(summary.failed[0].video, video);
        assert_eq!(summary.failed[0].reason, "not a video");
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
