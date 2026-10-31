//! The cuts a review or a split starts from: what the scan found, what the
//! file's chapter marks add, and what the limits on scene length make of
//! the two.
//!
//! The window and the command line both come here, so they cannot disagree
//! about which cuts a video gets. Nothing here reads a file or decodes a
//! frame: it all works on a finished scan, which is why changing a limit
//! needs no new scan.

use crate::chapters::{ChapterSplits, ChapterTitle};
use crate::detect::{Detection, ScanKind};
use crate::exact_cut::{self, CutPrecision};
use crate::hardcut::HardCutCandidate;
use crate::scene_length::{self, SceneLengthLimits, SplitReason, SuggestedSplit};
use crate::scenes::{self, CutOutsideVideo, CutTimes};
use crate::timecode::TimeSpan;

/// Whether the chapter marks of the file become splits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChapterMarksUse {
    SplitAtThem,
    LeaveOut,
}

/// What decides which splits are suggested for a video, apart from its scan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutSources {
    pub scan_kind: ScanKind,
    /// Whether each cut goes on the nearest point where the video can be
    /// cut, or where the scan saw the scene change.
    pub placement: CutPrecision,
    pub chapter_marks: ChapterMarksUse,
}

impl CutSources {
    /// What a review starts from: the cuts of the scan the person chose, on
    /// the nearest points where the video can be cut, and the chapter marks.
    pub fn for_review(scan_kind: ScanKind) -> Self {
        Self {
            scan_kind,
            placement: CutPrecision::NearestKeyframe,
            chapter_marks: ChapterMarksUse::SplitAtThem,
        }
    }
}

/// Every split suggested for one video, each with the reason for it, before
/// any limit on scene length. Kept by a review, so that other limits can be
/// applied later without the scan.
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestedCuts {
    /// The cuts of the scan: at black screens and at sudden picture changes.
    detected: CutTimes,
    /// Every suggested split, of the scan and of the chapter marks, in
    /// ascending order of time and each time once.
    splits: Vec<SuggestedSplit>,
    chapter_titles: Vec<ChapterTitle>,
    keyframe_times: Vec<f64>,
}

/// What the limits on scene length did to the suggested splits.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LimitsEffect {
    /// Splits left out so that no scene is shorter than the shortest scene.
    /// Each one merged a scene into its neighbour.
    pub splits_left_out: usize,
    /// Splits added so that no scene is longer than the longest scene.
    pub splits_added: usize,
}

/// The cuts a review or a split starts from.
#[derive(Debug, Clone, PartialEq)]
pub struct StartingCuts {
    /// The cuts of the scan that the limits left in.
    pub detected: CutTimes,
    /// The splits from chapter marks that the limits left in, and the title
    /// of every chapter.
    pub chapter_splits: ChapterSplits,
    /// The cuts that divide scenes longer than the longest scene, ascending.
    pub added_for_long_scenes: Vec<f64>,
    pub limits_effect: LimitsEffect,
}

impl StartingCuts {
    /// Every cut, whatever it is there for. Not in order.
    pub fn every_cut_time(&self) -> Vec<f64> {
        self.detected
            .as_slice()
            .iter()
            .chain(&self.chapter_splits.cuts)
            .chain(&self.added_for_long_scenes)
            .copied()
            .collect()
    }
}

impl SuggestedCuts {
    /// Gathers what the scan and the chapter marks suggest. Fails for a
    /// scan with a cut outside its video, which only a stored scan that was
    /// damaged on disk can hold.
    pub fn of(detection: &Detection, sources: &CutSources) -> Result<Self, CutOutsideVideo> {
        let detected_times = match sources.placement {
            CutPrecision::NearestKeyframe => detection.cuts_for(sources.scan_kind),
            CutPrecision::ExactFrame => exact_cut::exact_cut_times(detection, sources.scan_kind),
        };
        let detected = CutTimes::inside(detection.video, detected_times)?;
        let chapter_splits = match sources.chapter_marks {
            ChapterMarksUse::SplitAtThem => detection.chapter_splits(detected.as_slice()),
            ChapterMarksUse::LeaveOut => ChapterSplits::default(),
        };

        let reasons = ReasonsForCuts::of(detection, sources.placement);
        let mut splits: Vec<SuggestedSplit> = detected
            .as_slice()
            .iter()
            .map(|&time| SuggestedSplit {
                time,
                reason: reasons.for_detected_cut(time),
            })
            .collect();
        // A chapter mark only adds a split where the scan has none, so no
        // time comes twice.
        splits.extend(chapter_splits.cuts.iter().map(|&time| SuggestedSplit {
            time,
            reason: SplitReason::ChapterMark,
        }));
        splits.sort_by(|one, other| one.time.total_cmp(&other.time));

        Ok(Self {
            detected,
            splits,
            chapter_titles: chapter_splits.titles,
            keyframe_times: detection.keyframe_times.clone(),
        })
    }

    /// The suggested splits with the limits on scene length applied. What
    /// is left out and what is added is decided by
    /// `scene_length::splits_within`, for every kind of split together.
    pub fn within(&self, limits: SceneLengthLimits) -> StartingCuts {
        let limited = scene_length::splits_within(
            limits,
            &self.splits,
            &self.keyframe_times,
            self.whole_video(),
        );
        let stays = |time: f64| limited.kept.iter().any(|split| split.time == time);
        let chapter_cuts = limited
            .kept
            .iter()
            .filter(|split| split.reason == SplitReason::ChapterMark)
            .map(|split| split.time)
            .collect();
        StartingCuts {
            detected: self.detected.only(stays),
            chapter_splits: ChapterSplits {
                cuts: chapter_cuts,
                titles: self.chapter_titles.clone(),
            },
            limits_effect: LimitsEffect {
                splits_left_out: limited.left_out.len(),
                splits_added: limited.added.len(),
            },
            added_for_long_scenes: limited.added,
        }
    }

    pub fn whole_video(&self) -> TimeSpan {
        self.detected.whole_video()
    }

    /// The points where the video can be cut without encoding.
    pub fn keyframe_times(&self) -> &[f64] {
        &self.keyframe_times
    }

    /// How many scene changes the scan suggests a cut for.
    pub fn scene_change_count(&self) -> usize {
        self.detected.len()
    }

    /// How many of those are sudden picture changes, with no black screen.
    pub fn sudden_change_count(&self) -> usize {
        self.count_of(|reason| matches!(reason, SplitReason::SuddenChange { .. }))
    }

    /// How many splits come only from a chapter mark of the file.
    pub fn chapter_split_count(&self) -> usize {
        self.count_of(|reason| reason == SplitReason::ChapterMark)
    }

    fn count_of(&self, is_counted: impl Fn(SplitReason) -> bool) -> usize {
        self.splits
            .iter()
            .filter(|split| is_counted(split.reason))
            .count()
    }
}

/// What a scan knows about why each of its cuts is where it is.
struct ReasonsForCuts<'scan> {
    black_screens: &'scan [TimeSpan],
    /// The points black-screen cuts were placed on.
    black_cut_points: Vec<f64>,
    /// Where the cut for each sudden change would go, and how strong the
    /// change is. Empty for a scan that did not look for them.
    changes_by_cut: Vec<(f64, u8)>,
    whole_video: TimeSpan,
}

impl<'scan> ReasonsForCuts<'scan> {
    fn of(detection: &'scan Detection, placement: CutPrecision) -> Self {
        let candidates: &[HardCutCandidate] =
            detection.hard_cut_candidates.as_deref().unwrap_or(&[]);
        let (black_cut_points, changes_by_cut) = match placement {
            CutPrecision::NearestKeyframe => {
                let changes_by_cut = candidates
                    .iter()
                    .filter_map(|candidate| {
                        let moment = TimeSpan {
                            start: candidate.time,
                            end: candidate.time,
                        };
                        scenes::nearest_keyframe(&moment, &detection.keyframe_times)
                            .map(|cut| (cut, candidate.strength))
                    })
                    .collect();
                (detection.keyframe_times.clone(), changes_by_cut)
            }
            CutPrecision::ExactFrame => (
                exact_cut::black_screen_middles(detection),
                candidates
                    .iter()
                    .map(|candidate| (candidate.time, candidate.strength))
                    .collect(),
            ),
        };
        Self {
            black_screens: &detection.black_screens,
            black_cut_points,
            changes_by_cut,
            whole_video: detection.video,
        }
    }

    fn for_detected_cut(&self, cut: f64) -> SplitReason {
        let black_seconds = scenes::black_seconds_behind(
            cut,
            self.black_screens,
            &self.black_cut_points,
            self.whole_video,
        );
        if let Some(seconds) = black_seconds {
            return SplitReason::BlackScreen { seconds };
        }
        let strongest_change = self
            .changes_by_cut
            .iter()
            .filter(|(change_cut, _)| *change_cut == cut)
            .map(|&(_, strength)| strength)
            .max();
        match strongest_change {
            Some(strength) => SplitReason::SuddenChange { strength },
            // A stored scan can hold a cut without the black screen behind
            // it. It is a black-screen cut all the same, of no known length.
            None => SplitReason::BlackScreen { seconds: 0.0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chapters::Chapter;
    use crate::detect::ChapterMarks;
    use crate::hardcut::{HardCutSettings, Sensitivity};
    use crate::scene_length::SceneLength;

    const VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 1000.0,
    };

    fn black(start: f64, end: f64) -> TimeSpan {
        TimeSpan { start, end }
    }

    fn keyframes_every_ten_seconds() -> Vec<f64> {
        (0..100)
            .map(|keyframe| f64::from(keyframe) * 10.0)
            .collect()
    }

    /// A scan of a 1000 second video with these black screens, cut where
    /// the video can be cut every ten seconds.
    fn scan_with(black_screens: Vec<TimeSpan>) -> Detection {
        let keyframe_times = keyframes_every_ten_seconds();
        Detection {
            video: VIDEO,
            cuts: scenes::cut_points(&black_screens, &keyframe_times, VIDEO),
            keyframe_times,
            frame_count: 30_000,
            black_screens,
            decoded_frames: 0,
            decoded_windows: 0,
            hard_cut_candidates: None,
            chapters: ChapterMarks::Read(Vec::new()),
        }
    }

    fn untitled_chapter(start: f64) -> Chapter {
        Chapter { start, title: None }
    }

    fn sudden_changes(sensitivity: Sensitivity, min_scene_seconds: f64) -> ScanKind {
        ScanKind::HardCuts(HardCutSettings {
            sensitivity,
            min_scene_seconds,
        })
    }

    fn shortest(seconds: f64) -> SceneLengthLimits {
        SceneLengthLimits::new(Some(SceneLength::from_seconds(seconds).unwrap()), None).unwrap()
    }

    fn longest(seconds: f64) -> SceneLengthLimits {
        SceneLengthLimits::new(None, Some(SceneLength::from_seconds(seconds).unwrap())).unwrap()
    }

    fn for_review(detection: &Detection) -> SuggestedCuts {
        SuggestedCuts::of(detection, &CutSources::for_review(ScanKind::BlackScreens)).unwrap()
    }

    #[test]
    fn without_limits_the_cuts_are_those_of_the_scan_and_the_chapter_marks() {
        let mut detection = scan_with(vec![black(299.0, 301.0), black(699.0, 701.0)]);
        detection.chapters = ChapterMarks::Read(vec![untitled_chapter(500.0)]);

        let starting = for_review(&detection).within(SceneLengthLimits::default());
        assert_eq!(starting.detected.as_slice(), [300.0, 700.0]);
        assert_eq!(starting.chapter_splits.cuts, [500.0]);
        assert!(starting.added_for_long_scenes.is_empty());
        assert_eq!(starting.limits_effect, LimitsEffect::default());
    }

    #[test]
    fn each_cut_knows_the_black_screen_behind_it() {
        // A clutter of title cards, then a long black screen.
        let detection = scan_with(vec![
            black(99.5, 100.0),
            black(109.0, 111.0),
            black(119.75, 120.0),
            black(699.0, 701.0),
        ]);
        let starting = for_review(&detection).within(shortest(60.0));

        // Of the three title cards, the one with the longest black screen.
        assert_eq!(starting.detected.as_slice(), [110.0, 700.0]);
        assert_eq!(
            starting.limits_effect,
            LimitsEffect {
                splits_left_out: 2,
                splits_added: 0
            }
        );
    }

    #[test]
    fn the_limits_weigh_every_kind_of_split_together() {
        let mut detection = scan_with(vec![black(299.0, 301.0)]);
        detection.chapters =
            ChapterMarks::Read(vec![untitled_chapter(320.0), untitled_chapter(600.0)]);
        detection.hard_cut_candidates = Some(vec![
            HardCutCandidate {
                time: 590.0,
                strength: 250,
            },
            HardCutCandidate {
                time: 800.0,
                strength: 90,
            },
        ]);
        let sources = CutSources::for_review(sudden_changes(Sensitivity::Medium, 5.0));
        let suggested = SuggestedCuts::of(&detection, &sources).unwrap();
        assert_eq!(suggested.scene_change_count(), 3);
        assert_eq!(suggested.sudden_change_count(), 2);
        assert_eq!(suggested.chapter_split_count(), 2);

        let starting = suggested.within(shortest(60.0));
        // The chapter mark at 320 is too close to the black screen, and the
        // sudden change at 590 too close to the chapter mark at 600.
        assert_eq!(starting.detected.as_slice(), [300.0, 800.0]);
        assert_eq!(starting.chapter_splits.cuts, [600.0]);
        assert_eq!(starting.limits_effect.splits_left_out, 2);
    }

    #[test]
    fn a_long_scene_is_cut_where_the_video_can_be_cut() {
        let detection = scan_with(vec![black(99.0, 101.0)]);
        let starting = for_review(&detection).within(longest(400.0));
        assert_eq!(starting.detected.as_slice(), [100.0]);
        // 900 seconds in three parts of 300.
        assert_eq!(starting.added_for_long_scenes, [400.0, 700.0]);
        assert_eq!(starting.limits_effect.splits_added, 2);
        let mut every_cut = starting.every_cut_time();
        every_cut.sort_by(f64::total_cmp);
        assert_eq!(every_cut, [100.0, 400.0, 700.0]);
    }

    #[test]
    fn chapter_titles_are_kept_whatever_the_limits_leave_out() {
        let mut detection = scan_with(Vec::new());
        detection.chapters = ChapterMarks::Read(vec![Chapter {
            start: 20.0,
            title: Some("Opening".to_owned()),
        }]);
        let starting = for_review(&detection).within(shortest(60.0));
        assert!(starting.chapter_splits.cuts.is_empty());
        assert_eq!(
            starting.chapter_splits.title_of_scene_starting_at(20.0),
            Some("Opening")
        );
    }

    #[test]
    fn chapter_marks_can_be_left_out() {
        let mut detection = scan_with(Vec::new());
        detection.chapters = ChapterMarks::Read(vec![untitled_chapter(500.0)]);
        let sources = CutSources {
            chapter_marks: ChapterMarksUse::LeaveOut,
            ..CutSources::for_review(ScanKind::BlackScreens)
        };
        let suggested = SuggestedCuts::of(&detection, &sources).unwrap();
        assert_eq!(suggested.chapter_split_count(), 0);
    }

    #[test]
    fn exact_cuts_are_weighed_by_their_black_screens_too() {
        let detection = scan_with(vec![
            black(99.0, 100.0),
            black(108.0, 111.0),
            black(698.0, 701.0),
        ]);
        let sources = CutSources {
            placement: CutPrecision::ExactFrame,
            ..CutSources::for_review(ScanKind::BlackScreens)
        };
        let starting = SuggestedCuts::of(&detection, &sources)
            .unwrap()
            .within(shortest(60.0));
        // Each in the middle of its black screen.
        assert_eq!(starting.detected.as_slice(), [109.5, 699.5]);
    }

    #[test]
    fn a_stored_cut_without_its_black_screen_counts_as_the_shortest_one() {
        let mut detection = scan_with(vec![black(329.0, 331.0)]);
        detection.cuts = vec![300.0, 330.0];
        let starting = for_review(&detection).within(shortest(60.0));
        assert_eq!(starting.detected.as_slice(), [330.0]);
    }

    #[test]
    fn a_scan_with_a_cut_outside_its_video_is_refused() {
        let mut detection = scan_with(Vec::new());
        detection.cuts = vec![300.0, 1400.0];
        let refused =
            SuggestedCuts::of(&detection, &CutSources::for_review(ScanKind::BlackScreens));
        assert_eq!(refused.unwrap_err().time, 1400.0);
    }
}
