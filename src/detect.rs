//! Finds the scene changes of a video: read the packet table, decode the
//! promising stretches, group the black frames, and place a cut on each.
//! On request it also decodes every frame to look for hard cuts.

use std::path::Path;

use anyhow::Result;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::black::{self, BlackCriteria};
use crate::chapters::{self, Chapter, ChapterSplits};
use crate::hardcut::{self, HardCutCandidate, HardCutSettings};
use crate::scan::{self, UndecodedFrames};
use crate::timecode::TimeSpan;
use crate::{probe, scenes, windows};

/// How much of the video to decode while looking for black screens.
#[derive(Debug, Clone, Copy, PartialEq, ValueEnum, Serialize, Deserialize)]
pub enum ScanMode {
    /// Decode only where the packet sizes hint at a black screen. Fastest.
    Suspicious,
    /// Decode every frame. Use this when the black screens are noisy or
    /// grainy, since those do not compress into tiny packets.
    EveryFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanSettings {
    pub mode: ScanMode,
    pub criteria: BlackCriteria,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            mode: ScanMode::Suspicious,
            criteria: BlackCriteria::default(),
        }
    }
}

/// What a scan looks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScanDepth {
    /// Black screens only. Takes a couple of seconds.
    BlackScreens,
    /// Black screens and hard cuts. Decodes every frame, which takes about
    /// as long as playing the video at 250 times its speed.
    WithHardCuts,
}

/// Which scan the person chose for a video: what it looks for, and with
/// which settings the hard cuts it finds become cuts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScanKind {
    /// The normal, fast scan.
    BlackScreens,
    /// The slow scan that also finds scene changes without a black screen.
    HardCuts(HardCutSettings),
}

impl ScanKind {
    pub fn depth(self) -> ScanDepth {
        match self {
            Self::BlackScreens => ScanDepth::BlackScreens,
            Self::HardCuts(_) => ScanDepth::WithHardCuts,
        }
    }
}

/// Everything the scan learned about one video that later steps need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    /// From the first frame appearing to the last frame disappearing.
    pub video: TimeSpan,
    /// The only times a split without re-encoding can cut on.
    pub keyframe_times: Vec<f64>,
    pub frame_count: usize,
    pub black_screens: Vec<TimeSpan>,
    /// One suggested cut per black screen, on the nearest keyframe.
    pub cuts: Vec<f64>,
    pub decoded_frames: usize,
    pub decoded_windows: usize,
    /// The abrupt picture changes in the video, or `None` when the scan did
    /// not look for hard cuts.
    #[serde(default)]
    pub hard_cut_candidates: Option<Vec<HardCutCandidate>>,
    /// The chapter marks the file carries, as far as they are known. Left
    /// out of a stored scan unless they were read, so the file keeps the
    /// layout earlier versions wrote, and marks that could not be read are
    /// looked for again the next time.
    #[serde(default, skip_serializing_if = "ChapterMarks::are_not_known")]
    pub chapters: ChapterMarks,
}

/// What is known of the chapter marks of a video.
///
/// Chapter marks are an extra beside what the scan finds. Failing to read
/// them is therefore no reason to fail a scan, or to throw away a stored
/// one: the scan goes on without them and carries the reason.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(from = "Option<Vec<Chapter>>", into = "Option<Vec<Chapter>>")]
pub enum ChapterMarks {
    /// Nobody looked: the scan was stored before chapters were read.
    #[default]
    NeverRead,
    /// What the file carries. Most videos have none, which is an empty
    /// list here and not the same as never having looked.
    Read(Vec<Chapter>),
    /// Looking failed, for this reason. The video is treated as having no
    /// chapter marks, and the person is told.
    CouldNotRead(String),
}

impl ChapterMarks {
    /// Reads the chapter marks of `video`. Never fails: a file whose marks
    /// cannot be read is still a video that can be scanned and split.
    pub fn read_from(video: &Path) -> Self {
        match probe::read_chapters(video) {
            Ok(chapters) => Self::Read(chapters),
            Err(error) => Self::CouldNotRead(format!("{error:#}")),
        }
    }

    fn are_not_known(&self) -> bool {
        match self {
            Self::NeverRead | Self::CouldNotRead(_) => true,
            Self::Read(_) => false,
        }
    }

    /// Why the marks could not be read, when that is so.
    pub fn problem(&self) -> Option<&str> {
        match self {
            Self::CouldNotRead(reason) => Some(reason),
            Self::NeverRead | Self::Read(_) => None,
        }
    }
}

impl From<Option<Vec<Chapter>>> for ChapterMarks {
    fn from(stored: Option<Vec<Chapter>>) -> Self {
        stored.map_or(Self::NeverRead, Self::Read)
    }
}

impl From<ChapterMarks> for Option<Vec<Chapter>> {
    fn from(marks: ChapterMarks) -> Self {
        match marks {
            ChapterMarks::Read(chapters) => Some(chapters),
            ChapterMarks::NeverRead | ChapterMarks::CouldNotRead(_) => None,
        }
    }
}

impl Detection {
    pub fn satisfies(&self, depth: ScanDepth) -> bool {
        match depth {
            ScanDepth::BlackScreens => true,
            ScanDepth::WithHardCuts => self.hard_cut_candidates.is_some(),
        }
    }

    /// The cuts a review of this scan starts from: its black-screen cuts,
    /// and for a hard-cut scan the hard cuts its settings choose as well.
    pub fn cuts_for(&self, scan_kind: ScanKind) -> Vec<f64> {
        match scan_kind {
            ScanKind::BlackScreens => self.cuts.clone(),
            ScanKind::HardCuts(settings) => self.cuts_with_hard_cuts(&settings),
        }
    }

    /// The chapter marks of the file: none when it has none, or when they
    /// were never read or could not be.
    pub fn chapter_marks(&self) -> &[Chapter] {
        match &self.chapters {
            ChapterMarks::Read(chapters) => chapters,
            ChapterMarks::NeverRead | ChapterMarks::CouldNotRead(_) => &[],
        }
    }

    /// How long one picture of the video is shown, on average.
    pub fn frame_seconds(&self) -> f64 {
        self.video.seconds() / self.frame_count.max(1) as f64
    }

    /// What the chapter marks add to `detected_cuts`, the cuts a review or a
    /// split starts from.
    pub fn chapter_splits(&self, detected_cuts: &[f64]) -> ChapterSplits {
        chapters::chapter_splits(
            self.chapter_marks(),
            detected_cuts,
            &self.keyframe_times,
            self.video,
        )
    }

    /// The black-screen cuts plus the hard cuts chosen by `settings`. Without
    /// a hard-cut scan this is the black-screen cuts alone.
    pub fn cuts_with_hard_cuts(&self, settings: &HardCutSettings) -> Vec<f64> {
        let Some(candidates) = &self.hard_cut_candidates else {
            return self.cuts.clone();
        };
        hardcut::cuts_with_hard_cuts(
            &self.cuts,
            candidates,
            &self.keyframe_times,
            self.video,
            settings,
        )
    }
}

/// The two parts of a scan, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScanStage {
    BlackScreens,
    /// Only part of a scan that also looks for hard cuts.
    HardCuts,
}

/// How far a scan has come.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanProgress {
    pub stage: ScanStage,
    /// The share of the whole scan that is done, from 0.0 to 1.0, counted in
    /// frames to decode. A scan that also looks for hard cuts passes through
    /// both stages on the way to 1.0.
    pub fraction: f32,
}

/// Receives the progress of a scan. Called from the threads that decode, so
/// two reports close together can arrive out of order.
pub type OnScanProgress<'a> = &'a (dyn Fn(ScanProgress) + Sync);

/// For callers that do not show progress.
pub fn no_scan_progress(_progress: ScanProgress) {}

/// A finished scan, and what the person should know about it.
#[derive(Debug, PartialEq)]
pub struct ScanOutcome {
    pub detection: Detection,
    /// Stretches of the video that could not be decoded, so scene changes in
    /// them may have been missed. Empty for a healthy video.
    pub undecoded: Vec<UndecodedFrames>,
}

/// How many frames each stage of a scan decodes, which is what the scan's
/// time goes into.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ScanWork {
    black_screen_frames: usize,
    hard_cut_frames: usize,
}

impl ScanWork {
    /// The share of the whole scan done once `stage` is `stage_fraction`
    /// through its own frames.
    fn overall_fraction(self, stage: ScanStage, stage_fraction: f32) -> f32 {
        let all_frames = self.black_screen_frames + self.hard_cut_frames;
        if all_frames == 0 {
            return stage_fraction;
        }
        let (frames_before, stage_frames) = match stage {
            ScanStage::BlackScreens => (0, self.black_screen_frames),
            ScanStage::HardCuts => (self.black_screen_frames, self.hard_cut_frames),
        };
        let frames_done = frames_before as f64 + f64::from(stage_fraction) * stage_frames as f64;
        (frames_done / all_frames as f64) as f32
    }
}

/// Finds the black screens of the video and, when asked, its hard cuts.
pub fn scan_for_scene_changes(
    video: &Path,
    settings: &ScanSettings,
    depth: ScanDepth,
    on_progress: OnScanProgress,
) -> Result<ScanOutcome> {
    let index = probe::read_packet_index(video)?;
    let chapters = ChapterMarks::read_from(video);
    let packets = &index.packets;
    let criteria = &settings.criteria;

    let windows = match settings.mode {
        ScanMode::Suspicious => windows::suspicious_windows(packets, criteria.min_seconds),
        ScanMode::EveryFrame => windows::whole_video_windows(packets, rayon::current_num_threads()),
    };
    let decoded_frames = windows.iter().map(|window| window.len()).sum();
    let work = ScanWork {
        black_screen_frames: decoded_frames,
        hard_cut_frames: match depth {
            ScanDepth::BlackScreens => 0,
            ScanDepth::WithHardCuts => packets.len(),
        },
    };
    let report_stage = |stage: ScanStage| {
        move |stage_fraction: f32| {
            on_progress(ScanProgress {
                stage,
                fraction: work.overall_fraction(stage, stage_fraction),
            });
        }
    };

    let black_frames = scan::find_black_frames(
        video,
        packets,
        &windows,
        criteria,
        &report_stage(ScanStage::BlackScreens),
    )?;
    let mut undecoded = black_frames.undecoded;

    let frame_times: Vec<f64> = packets.iter().map(|packet| packet.time).collect();
    let black_screens = black::black_screens(
        &frame_times,
        &black_frames.frame_is_black,
        index.video.end,
        criteria.min_seconds,
    );
    let keyframe_times = index.keyframe_times();
    let cuts = scenes::cut_points(&black_screens, &keyframe_times, index.video);

    let hard_cut_candidates = match depth {
        ScanDepth::BlackScreens => None,
        ScanDepth::WithHardCuts => {
            let whole_video = windows::whole_video_windows(packets, rayon::current_num_threads());
            let hard_cut_scan = hardcut::find_candidates(
                video,
                packets,
                &whole_video,
                &report_stage(ScanStage::HardCuts),
            )?;
            // Both stages decode the same frames when the black screens are
            // looked for in every frame, and then report the same stretches.
            for stretch in hard_cut_scan.undecoded {
                if !undecoded.contains(&stretch) {
                    undecoded.push(stretch);
                }
            }
            Some(hard_cut_scan.candidates)
        }
    };

    Ok(ScanOutcome {
        detection: Detection {
            video: index.video,
            keyframe_times,
            frame_count: packets.len(),
            black_screens,
            cuts,
            decoded_frames,
            decoded_windows: windows.len(),
            hard_cut_candidates,
            chapters,
        },
        undecoded,
    })
}

#[cfg(test)]
mod chapter_mark_tests {
    use super::*;

    fn stored(marks: &ChapterMarks) -> String {
        serde_json::to_string(marks).unwrap()
    }

    #[test]
    fn chapter_marks_that_were_read_are_stored_as_the_list_they_always_were() {
        let read = ChapterMarks::Read(vec![Chapter {
            start: 12.5,
            title: Some("Opening".to_owned()),
        }]);
        assert_eq!(stored(&read), r#"[{"start":12.5,"title":"Opening"}]"#);
        assert_eq!(
            serde_json::from_str::<ChapterMarks>(&stored(&read)).unwrap(),
            read
        );
        // A file without chapters is not one that was never looked at.
        assert_eq!(
            serde_json::from_str::<ChapterMarks>("[]").unwrap(),
            ChapterMarks::Read(Vec::new())
        );
    }

    #[test]
    fn marks_that_could_not_be_read_are_looked_for_again_next_time() {
        let failed = ChapterMarks::CouldNotRead("ffprobe ran out of time".to_owned());
        assert_eq!(failed.problem(), Some("ffprobe ran out of time"));
        assert!(failed.are_not_known());
        assert!(ChapterMarks::NeverRead.are_not_known());
        assert!(!ChapterMarks::Read(Vec::new()).are_not_known());
        // What is not known is stored as nothing, which loads as never read.
        assert_eq!(stored(&failed), "null");
        assert_eq!(
            serde_json::from_str::<ChapterMarks>("null").unwrap(),
            ChapterMarks::NeverRead
        );
        assert_eq!(ChapterMarks::NeverRead.problem(), None);
    }

    #[test]
    fn a_file_that_is_no_video_has_no_chapter_marks_and_a_reason() {
        // The tools are not even needed to fail on a path that is not there.
        let marks = ChapterMarks::read_from(Path::new("no_such_folder/no_such_video.mp4"));
        assert!(marks.problem().is_some(), "{marks:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK_SCREENS_ONLY: ScanWork = ScanWork {
        black_screen_frames: 500,
        hard_cut_frames: 0,
    };
    /// A quick look at a few stretches, then every frame for the hard cuts.
    const WITH_HARD_CUTS: ScanWork = ScanWork {
        black_screen_frames: 1_000,
        hard_cut_frames: 9_000,
    };

    #[test]
    fn a_scan_kind_decodes_as_deeply_as_what_it_looks_for() {
        assert_eq!(ScanKind::BlackScreens.depth(), ScanDepth::BlackScreens);
        assert_eq!(
            ScanKind::HardCuts(HardCutSettings::default()).depth(),
            ScanDepth::WithHardCuts
        );
    }

    #[test]
    fn a_scan_for_black_screens_alone_is_its_only_stage() {
        let overall =
            |fraction| BLACK_SCREENS_ONLY.overall_fraction(ScanStage::BlackScreens, fraction);
        assert_eq!(overall(0.0), 0.0);
        assert_eq!(overall(0.5), 0.5);
        assert_eq!(overall(1.0), 1.0);
    }

    #[test]
    fn each_stage_counts_for_its_share_of_the_frames() {
        let black = |fraction| WITH_HARD_CUTS.overall_fraction(ScanStage::BlackScreens, fraction);
        let hard_cuts = |fraction| WITH_HARD_CUTS.overall_fraction(ScanStage::HardCuts, fraction);

        assert_eq!(black(0.5), 0.05);
        assert_eq!(black(1.0), 0.1);
        // The second stage picks up exactly where the first one ended.
        assert_eq!(hard_cuts(0.0), black(1.0));
        assert_eq!(hard_cuts(0.5), 0.55);
        assert_eq!(hard_cuts(1.0), 1.0);
    }

    #[test]
    fn a_video_with_nothing_worth_decoding_still_reaches_the_end() {
        const NOTHING_TO_DECODE: ScanWork = ScanWork {
            black_screen_frames: 0,
            hard_cut_frames: 0,
        };
        assert_eq!(
            NOTHING_TO_DECODE.overall_fraction(ScanStage::BlackScreens, 1.0),
            1.0
        );
    }

    #[test]
    fn nothing_suspicious_before_a_hard_cut_scan_starts_that_scan_at_zero() {
        const ONLY_HARD_CUTS: ScanWork = ScanWork {
            black_screen_frames: 0,
            hard_cut_frames: 9_000,
        };
        // The first stage has nothing to decode and says it is done, which
        // is no progress on the whole.
        assert_eq!(
            ONLY_HARD_CUTS.overall_fraction(ScanStage::BlackScreens, 1.0),
            0.0
        );
        assert_eq!(
            ONLY_HARD_CUTS.overall_fraction(ScanStage::HardCuts, 0.5),
            0.5
        );
    }
}
