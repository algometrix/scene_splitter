//! The split plan a person edits during review: every proposed cut, and
//! whether to split there or merge the footage on either side.
//!
//! A plan counts three different things from zero, and each has its own
//! index type so one cannot be passed for another: scenes (what the split
//! would write now), segments (the stretches between all proposed cuts,
//! merged or not) and the cuts themselves.

use std::fmt;
use std::ops::RangeInclusive;

use crate::scenes::{self, CutTimes};
use crate::timecode::TimeSpan;

/// Where a scene sits among the scenes the split would write right now.
/// Scenes are renumbered whenever the plan changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneIndex(usize);

impl SceneIndex {
    pub fn new(position: usize) -> Self {
        Self(position)
    }

    /// Counted from zero, for looking the scene up in a list.
    pub fn position(self) -> usize {
        self.0
    }

    /// Counted from one, the way scenes are numbered for people.
    pub fn number(self) -> usize {
        self.0 + 1
    }

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Where a segment sits among all the plan's segments. Merging and splitting
/// do not renumber segments, only adding or removing a cut does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentIndex(usize);

impl SegmentIndex {
    // Cut n sits between segment n and segment n + 1.

    /// `None` for the first segment, which starts where the video starts.
    fn cut_before(self) -> Option<CutIndex> {
        self.0.checked_sub(1).map(CutIndex)
    }

    /// The last segment has no cut after it, so for that one this index
    /// points past the plan's cuts.
    fn cut_after(self) -> CutIndex {
        CutIndex(self.0)
    }
}

/// Where a cut sits among all the plan's proposed cuts, merged or not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutIndex(usize);

/// Where a proposed cut came from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CutSource {
    /// Suggested by the scan, whether for a black screen or for a sudden
    /// change of picture. The plan is not told which.
    Detected,
    AddedByUser,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CutChoice {
    Split,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ProposedCut {
    time: f64,
    source: CutSource,
    choice: CutChoice,
}

#[derive(Debug, PartialEq)]
pub enum AddCutError {
    OutsideVideo,
    /// The video has no keyframe to cut on other than its very first frame.
    NoKeyframeToCutOn,
    AlreadyProposed {
        at: f64,
    },
}

impl fmt::Display for AddCutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideVideo => write!(formatter, "That time is outside the video."),
            Self::NoKeyframeToCutOn => {
                write!(
                    formatter,
                    "There is no point near that time where the video can be cut."
                )
            }
            Self::AlreadyProposed { at } => write!(
                formatter,
                "There is already a split point at {}.",
                crate::timecode::format_timestamp(*at)
            ),
        }
    }
}

/// One scene as the split would write it, with the segments merged into it.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneLayout {
    pub span: TimeSpan,
    pub segments: Vec<TimeSpan>,
    /// Where its first segment sits among all the plan's segments.
    pub first_segment: SegmentIndex,
    /// The merged cuts between its segments. Always one fewer than the
    /// segments.
    pub merged_cuts: Vec<CutIndex>,
}

impl SceneLayout {
    /// The plan-wide index of one of this scene's segments, given its place
    /// within the scene.
    pub fn segment_index(&self, position_in_scene: usize) -> SegmentIndex {
        SegmentIndex(self.first_segment.0 + position_in_scene)
    }

    /// Whether the person joined two or more segments into this scene.
    pub fn is_merged(&self) -> bool {
        !self.merged_cuts.is_empty()
    }
}

/// A plan's cuts and choices at one moment, kept so that a later change can
/// be undone.
#[derive(Debug)]
pub struct PlanSnapshot {
    cuts: Vec<ProposedCut>,
}

#[derive(Debug)]
pub struct SplitPlan {
    whole_video: TimeSpan,
    keyframe_times: Vec<f64>,
    /// In ascending order of time, with no duplicates, each strictly inside
    /// `whole_video`.
    cuts: Vec<ProposedCut>,
}

impl SplitPlan {
    /// Starts with every detected cut set to split.
    pub fn from_detected_cuts(detected_cuts: &CutTimes, keyframe_times: Vec<f64>) -> Self {
        let cuts = detected_cuts
            .as_slice()
            .iter()
            .map(|&time| ProposedCut {
                time,
                source: CutSource::Detected,
                choice: CutChoice::Split,
            })
            .collect();
        Self {
            whole_video: detected_cuts.whole_video(),
            keyframe_times,
            cuts,
        }
    }

    pub fn whole_video(&self) -> TimeSpan {
        self.whole_video
    }

    /// The stretches of footage between all proposed cuts, merged or not.
    /// There is always one more segment than there are cuts.
    pub fn segments(&self) -> Vec<TimeSpan> {
        let every_cut_time: Vec<f64> = self.cuts.iter().map(|cut| cut.time).collect();
        scenes::scenes_between(&every_cut_time, self.whole_video)
    }

    /// The scenes that splitting would produce right now.
    pub fn scenes(&self) -> Vec<TimeSpan> {
        self.accepted_cuts().scenes()
    }

    /// The current scenes in order, each with the segments it is made of.
    pub fn scene_layouts(&self) -> Vec<SceneLayout> {
        let mut layouts: Vec<SceneLayout> = Vec::new();
        for (position, segment) in self.segments().into_iter().enumerate() {
            let segment_index = SegmentIndex(position);
            let merged_cut_before = segment_index
                .cut_before()
                .filter(|cut| self.cuts[cut.0].choice == CutChoice::Merge);
            match (layouts.last_mut(), merged_cut_before) {
                (Some(layout), Some(cut)) => {
                    layout.span.end = segment.end;
                    layout.segments.push(segment);
                    layout.merged_cuts.push(cut);
                }
                _ => layouts.push(SceneLayout {
                    span: segment,
                    segments: vec![segment],
                    first_segment: segment_index,
                    merged_cuts: Vec::new(),
                }),
            }
        }
        layouts
    }

    /// Joins the given run of current scenes into one scene.
    pub fn merge_scenes(&mut self, scenes: RangeInclusive<SceneIndex>) {
        // The nth accepted cut is the one between scene n and scene n + 1.
        let accepted_cut_positions: Vec<usize> = self
            .cuts
            .iter()
            .enumerate()
            .filter(|(_, cut)| cut.choice == CutChoice::Split)
            .map(|(position, _)| position)
            .collect();
        for boundary in scenes.start().0..scenes.end().0 {
            if let Some(&cut_position) = accepted_cut_positions.get(boundary) {
                self.cuts[cut_position].choice = CutChoice::Merge;
            }
        }
    }

    /// Takes one segment out of a merged scene and makes it a scene of its
    /// own, by restoring the split on each side of it. The segments before
    /// and after it keep whatever else they were merged with.
    pub fn isolate_segment(&mut self, segment: SegmentIndex) {
        let cuts_around = segment
            .cut_before()
            .into_iter()
            .chain([segment.cut_after()]);
        for cut_index in cuts_around {
            if let Some(cut) = self.cuts.get_mut(cut_index.0) {
                cut.choice = CutChoice::Split;
            }
        }
    }

    /// Goes back to the scan's own suggestion: every detected cut splits
    /// again, and the cuts the person added are gone.
    pub fn reset_to_detected(&mut self) {
        self.cuts.retain(|cut| cut.source == CutSource::Detected);
        for cut in &mut self.cuts {
            cut.choice = CutChoice::Split;
        }
    }

    /// The plan as it is now, to come back to with `restore`.
    pub fn snapshot(&self) -> PlanSnapshot {
        PlanSnapshot {
            cuts: self.cuts.clone(),
        }
    }

    /// Puts the plan back to how it was when `snapshot` was taken of it.
    pub fn restore(&mut self, snapshot: PlanSnapshot) {
        self.cuts = snapshot.cuts;
    }

    /// Whether nothing has changed since `snapshot` was taken.
    pub fn is_at(&self, snapshot: &PlanSnapshot) -> bool {
        self.cuts == snapshot.cuts
    }

    /// Whether the plan is still exactly what the scan suggested.
    pub fn is_as_detected(&self) -> bool {
        self.cuts
            .iter()
            .all(|cut| cut.source == CutSource::Detected && cut.choice == CutChoice::Split)
    }

    /// Undoes every merge strictly inside `span`.
    pub fn restore_splits_within(&mut self, span: TimeSpan) {
        for cut in &mut self.cuts {
            if span.has_strictly_inside(cut.time) {
                cut.choice = CutChoice::Split;
            }
        }
    }

    /// The times the split would cut at right now.
    pub fn accepted_cuts(&self) -> CutTimes {
        let accepted_times = self
            .cuts
            .iter()
            .filter(|cut| cut.choice == CutChoice::Split)
            .map(|cut| cut.time)
            .collect();
        // Cuts only enter a plan from `CutTimes` of this same video or
        // through `add_cut_near`, which refuses a time outside it.
        CutTimes::inside(self.whole_video, accepted_times)
            .expect("every cut of a plan is strictly inside the plan's video")
    }

    pub fn cut_time(&self, cut: CutIndex) -> f64 {
        self.cuts[cut.0].time
    }

    pub fn set_choice(&mut self, cut: CutIndex, choice: CutChoice) {
        self.cuts[cut.0].choice = choice;
    }

    /// Where a cut asked for at `requested_time` would land: on the nearest
    /// keyframe, since a split without re-encoding can cut nowhere else.
    pub fn cut_time_near(&self, requested_time: f64) -> Result<f64, AddCutError> {
        if !self.whole_video.has_strictly_inside(requested_time) {
            return Err(AddCutError::OutsideVideo);
        }
        let exact_moment = TimeSpan {
            start: requested_time,
            end: requested_time,
        };
        scenes::nearest_keyframe(&exact_moment, &self.keyframe_times)
            .filter(|&keyframe| self.whole_video.has_strictly_inside(keyframe))
            .ok_or(AddCutError::NoKeyframeToCutOn)
    }

    /// Adds a cut on the keyframe nearest to `requested_time` and returns the
    /// time it landed on.
    pub fn add_cut_near(&mut self, requested_time: f64) -> Result<f64, AddCutError> {
        let time = self.cut_time_near(requested_time)?;

        let position = self.cuts.partition_point(|cut| cut.time < time);
        let existing_cut = self.cuts.get_mut(position).filter(|cut| cut.time == time);
        if let Some(cut) = existing_cut {
            if cut.choice == CutChoice::Split {
                return Err(AddCutError::AlreadyProposed { at: time });
            }
            // A cut that was merged away is simply brought back.
            cut.choice = CutChoice::Split;
            return Ok(time);
        }
        self.cuts.insert(
            position,
            ProposedCut {
                time,
                source: CutSource::AddedByUser,
                choice: CutChoice::Split,
            },
        );
        Ok(time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHOLE_VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 10.0,
    };

    fn span(start: f64, end: f64) -> TimeSpan {
        TimeSpan { start, end }
    }

    fn plan_with_cuts(detected_cuts: &[f64]) -> SplitPlan {
        plan_with_keyframes(detected_cuts, vec![0.0, 2.0, 4.0, 6.0, 8.0])
    }

    fn plan_with_keyframes(detected_cuts: &[f64], keyframe_times: Vec<f64>) -> SplitPlan {
        let detected_cuts = CutTimes::inside(WHOLE_VIDEO, detected_cuts.to_vec()).unwrap();
        SplitPlan::from_detected_cuts(&detected_cuts, keyframe_times)
    }

    fn scenes(first: usize, last: usize) -> RangeInclusive<SceneIndex> {
        SceneIndex::new(first)..=SceneIndex::new(last)
    }

    #[test]
    fn every_detected_cut_starts_as_a_split() {
        let plan = plan_with_cuts(&[4.0, 8.0]);
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0, 8.0]);
        assert_eq!(plan.scenes().len(), 3);
    }

    #[test]
    fn merging_a_cut_joins_its_scenes_but_keeps_the_segments() {
        let mut plan = plan_with_cuts(&[4.0, 8.0]);
        plan.set_choice(CutIndex(0), CutChoice::Merge);

        assert_eq!(plan.scenes(), vec![span(0.0, 8.0), span(8.0, 10.0)]);
        assert_eq!(plan.segments().len(), 3);
    }

    #[test]
    fn merging_a_run_of_scenes_leaves_the_others_alone() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(1, 3));
        assert_eq!(
            plan.scenes(),
            vec![span(0.0, 2.0), span(2.0, 8.0), span(8.0, 10.0)]
        );
    }

    #[test]
    fn scene_numbers_count_the_scenes_as_they_are_now() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(0, 1));
        // After the first merge the scenes are 0-4, 4-6, 6-8 and 8-10.
        plan.merge_scenes(scenes(1, 2));
        assert_eq!(
            plan.scenes(),
            vec![span(0.0, 4.0), span(4.0, 8.0), span(8.0, 10.0)]
        );
    }

    #[test]
    fn a_scene_layout_lists_its_merged_segments_and_cuts() {
        let mut plan = plan_with_cuts(&[4.0, 8.0]);
        plan.merge_scenes(scenes(0, 1));
        assert_eq!(
            plan.scene_layouts(),
            vec![
                SceneLayout {
                    span: span(0.0, 8.0),
                    segments: vec![span(0.0, 4.0), span(4.0, 8.0)],
                    first_segment: SegmentIndex(0),
                    merged_cuts: vec![CutIndex(0)],
                },
                SceneLayout {
                    span: span(8.0, 10.0),
                    segments: vec![span(8.0, 10.0)],
                    first_segment: SegmentIndex(2),
                    merged_cuts: vec![],
                },
            ]
        );
    }

    #[test]
    fn a_middle_segment_moved_out_leaves_a_scene_on_each_side() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(0, 4));

        plan.isolate_segment(SegmentIndex(2));
        assert_eq!(
            plan.scenes(),
            vec![span(0.0, 4.0), span(4.0, 6.0), span(6.0, 10.0)]
        );
    }

    #[test]
    fn the_first_segment_moved_out_leaves_the_rest_merged() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(0, 4));

        plan.isolate_segment(SegmentIndex(0));
        assert_eq!(plan.scenes(), vec![span(0.0, 2.0), span(2.0, 10.0)]);
    }

    #[test]
    fn the_last_segment_moved_out_leaves_the_rest_merged() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(0, 4));

        plan.isolate_segment(SegmentIndex(4));
        assert_eq!(plan.scenes(), vec![span(0.0, 8.0), span(8.0, 10.0)]);
    }

    #[test]
    fn moving_a_segment_out_does_not_touch_other_merged_scenes() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(0, 1));
        // The scenes are now 0-4, 4-6, 6-8 and 8-10.
        plan.merge_scenes(scenes(2, 3));

        plan.isolate_segment(SegmentIndex(0));
        assert_eq!(
            plan.scenes(),
            vec![
                span(0.0, 2.0),
                span(2.0, 4.0),
                span(4.0, 6.0),
                span(6.0, 10.0)
            ]
        );
    }

    #[test]
    fn a_reset_undoes_merges_and_removes_added_cuts() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        assert!(plan.is_as_detected());

        plan.merge_scenes(scenes(0, 1));
        plan.add_cut_near(5.75).unwrap();
        assert!(!plan.is_as_detected());

        plan.reset_to_detected();
        assert!(plan.is_as_detected());
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 8.0]);
    }

    #[test]
    fn a_snapshot_can_be_put_back() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        let before_the_merge = plan.snapshot();

        plan.merge_scenes(scenes(0, 2));
        plan.restore(before_the_merge);
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 8.0]);
    }

    #[test]
    fn a_plan_is_at_its_snapshot_until_it_changes() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        let snapshot = plan.snapshot();
        assert!(plan.is_at(&snapshot));

        plan.merge_scenes(scenes(0, 1));
        assert!(!plan.is_at(&snapshot));
    }

    #[test]
    fn an_edit_that_changes_nothing_leaves_the_plan_at_its_snapshot() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        let snapshot = plan.snapshot();

        plan.merge_scenes(scenes(1, 1));
        assert_eq!(
            plan.add_cut_near(2.25),
            Err(AddCutError::AlreadyProposed { at: 2.0 })
        );
        assert!(plan.is_at(&snapshot));
    }

    #[test]
    fn a_snapshot_is_not_changed_by_later_edits() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        let before_the_edits = plan.snapshot();

        plan.merge_scenes(scenes(0, 1));
        plan.add_cut_near(5.75).unwrap();
        plan.restore(before_the_edits);
        assert!(plan.is_as_detected());
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 8.0]);
    }

    #[test]
    fn restoring_splits_only_touches_the_given_scene() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(0, 1));
        plan.merge_scenes(scenes(1, 2));

        plan.restore_splits_within(span(0.0, 4.0));
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 4.0, 8.0]);
    }

    #[test]
    fn a_merged_cut_can_be_split_again() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.set_choice(CutIndex(0), CutChoice::Merge);
        plan.set_choice(CutIndex(0), CutChoice::Split);
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0]);
    }

    #[test]
    fn an_added_cut_snaps_to_the_nearest_keyframe_and_stays_in_order() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);

        assert_eq!(plan.add_cut_near(5.75), Ok(6.0));
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 6.0, 8.0]);
        assert_eq!(plan.cuts[1].source, CutSource::AddedByUser);
    }

    #[test]
    fn adding_a_cut_where_one_exists_is_refused() {
        let mut plan = plan_with_cuts(&[4.0]);
        assert_eq!(
            plan.add_cut_near(4.25),
            Err(AddCutError::AlreadyProposed { at: 4.0 })
        );
    }

    #[test]
    fn adding_a_cut_where_one_was_merged_brings_it_back() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.set_choice(CutIndex(0), CutChoice::Merge);

        assert_eq!(plan.add_cut_near(4.25), Ok(4.0));
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0]);
        assert!(plan.is_as_detected());
    }

    #[test]
    fn asking_where_a_cut_would_land_changes_nothing() {
        let plan = plan_with_cuts(&[2.0]);
        assert_eq!(plan.cut_time_near(5.75), Ok(6.0));
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0]);
    }

    #[test]
    fn adding_a_cut_outside_the_video_is_refused() {
        let mut plan = plan_with_cuts(&[]);
        assert_eq!(plan.add_cut_near(12.0), Err(AddCutError::OutsideVideo));
        assert_eq!(plan.add_cut_near(0.0), Err(AddCutError::OutsideVideo));
    }

    #[test]
    fn a_cut_that_would_snap_to_the_first_frame_is_refused() {
        let mut plan = plan_with_cuts(&[]);
        assert_eq!(plan.add_cut_near(0.5), Err(AddCutError::NoKeyframeToCutOn));
    }

    #[test]
    fn a_plan_with_no_cuts_is_one_scene_of_the_whole_video() {
        let plan = plan_with_cuts(&[]);

        assert!(plan.is_as_detected());
        assert!(plan.accepted_cuts().is_empty());
        assert_eq!(
            plan.scene_layouts(),
            vec![SceneLayout {
                span: WHOLE_VIDEO,
                segments: vec![WHOLE_VIDEO],
                first_segment: SegmentIndex(0),
                merged_cuts: vec![],
            }]
        );
    }

    #[test]
    fn moving_the_only_segment_out_changes_nothing() {
        let mut plan = plan_with_cuts(&[]);
        plan.isolate_segment(SegmentIndex(0));
        assert_eq!(plan.scenes(), vec![WHOLE_VIDEO]);
    }

    #[test]
    fn merging_past_the_last_scene_stops_at_the_last_scene() {
        const SCENE_BEYOND_THE_LAST: usize = 9;
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);

        plan.merge_scenes(scenes(3, SCENE_BEYOND_THE_LAST));
        assert_eq!(
            plan.scenes(),
            vec![
                span(0.0, 2.0),
                span(2.0, 4.0),
                span(4.0, 6.0),
                span(6.0, 10.0)
            ]
        );
    }

    #[test]
    fn merging_a_single_scene_changes_nothing() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(2, 2));
        assert!(plan.is_as_detected());
    }

    #[test]
    fn scene_layouts_number_segments_and_cuts_across_the_whole_plan() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(0, 1));
        // The scenes are now 0-4, 4-6, 6-8 and 8-10.
        plan.merge_scenes(scenes(2, 3));

        assert_eq!(
            plan.scene_layouts(),
            vec![
                SceneLayout {
                    span: span(0.0, 4.0),
                    segments: vec![span(0.0, 2.0), span(2.0, 4.0)],
                    first_segment: SegmentIndex(0),
                    merged_cuts: vec![CutIndex(0)],
                },
                SceneLayout {
                    span: span(4.0, 6.0),
                    segments: vec![span(4.0, 6.0)],
                    first_segment: SegmentIndex(2),
                    merged_cuts: vec![],
                },
                SceneLayout {
                    span: span(6.0, 10.0),
                    segments: vec![span(6.0, 8.0), span(8.0, 10.0)],
                    first_segment: SegmentIndex(3),
                    merged_cuts: vec![CutIndex(3)],
                },
            ]
        );
    }

    #[test]
    fn a_reset_after_merging_and_moving_a_segment_out_gives_the_detected_scenes() {
        let detected_cuts = [2.0, 4.0, 6.0, 8.0];
        let mut plan = plan_with_cuts(&detected_cuts);
        plan.merge_scenes(scenes(0, 4));
        plan.isolate_segment(SegmentIndex(2));

        plan.reset_to_detected();
        assert_eq!(plan.scenes(), plan_with_cuts(&detected_cuts).scenes());
    }

    #[test]
    fn moving_the_only_merged_segment_back_out_returns_to_the_detected_plan() {
        let mut plan = plan_with_cuts(&[4.0, 8.0]);
        plan.merge_scenes(scenes(0, 1));
        assert!(!plan.is_as_detected());

        plan.isolate_segment(SegmentIndex(0));
        assert!(plan.is_as_detected());
    }

    #[test]
    fn an_added_cut_that_is_merged_away_still_differs_from_the_detected_plan() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        plan.add_cut_near(5.75).unwrap();
        // The added cut at 6 separates scenes 1 and 2.
        plan.merge_scenes(scenes(1, 2));

        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 8.0]);
        assert!(!plan.is_as_detected());
    }

    #[test]
    fn putting_a_snapshot_back_removes_a_cut_added_since() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        let before_the_cut = plan.snapshot();
        let same_moment = plan.snapshot();

        plan.add_cut_near(5.75).unwrap();
        plan.restore(before_the_cut);
        assert!(plan.is_at(&same_moment));
        assert_eq!(plan.segments().len(), 3);
    }

    #[test]
    fn a_cut_added_inside_a_merged_scene_splits_it_and_keeps_the_other_merges() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        plan.merge_scenes(scenes(0, 2));

        assert_eq!(plan.add_cut_near(5.75), Ok(6.0));
        // The new cut sits between the two detected ones, so the cut at 8
        // moves from index 1 to index 2.
        assert_eq!(
            plan.scene_layouts(),
            vec![
                SceneLayout {
                    span: span(0.0, 6.0),
                    segments: vec![span(0.0, 2.0), span(2.0, 6.0)],
                    first_segment: SegmentIndex(0),
                    merged_cuts: vec![CutIndex(0)],
                },
                SceneLayout {
                    span: span(6.0, 10.0),
                    segments: vec![span(6.0, 8.0), span(8.0, 10.0)],
                    first_segment: SegmentIndex(2),
                    merged_cuts: vec![CutIndex(2)],
                },
            ]
        );
    }

    #[test]
    fn restoring_splits_leaves_the_cuts_on_the_edges_of_the_span_merged() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(0, 4));

        plan.restore_splits_within(span(2.0, 6.0));
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0]);
    }

    #[test]
    fn a_time_before_the_first_keyframe_lands_on_that_keyframe() {
        let keyframes_starting_late = vec![2.0, 4.0];
        let mut plan = plan_with_keyframes(&[], keyframes_starting_late);
        assert_eq!(plan.add_cut_near(0.5), Ok(2.0));
    }

    #[test]
    fn a_time_after_the_last_keyframe_lands_on_that_keyframe() {
        let mut plan = plan_with_cuts(&[]);
        assert_eq!(plan.add_cut_near(9.5), Ok(8.0));
    }

    #[test]
    fn a_time_midway_between_two_keyframes_lands_on_the_earlier_one() {
        let plan = plan_with_cuts(&[]);
        assert_eq!(plan.cut_time_near(5.0), Ok(4.0));
    }

    #[test]
    fn a_video_without_keyframes_cannot_take_a_cut() {
        let mut plan = plan_with_keyframes(&[], Vec::new());
        assert_eq!(plan.add_cut_near(5.0), Err(AddCutError::NoKeyframeToCutOn));
        assert!(plan.cuts.is_empty());
    }

    #[test]
    fn a_time_outside_the_video_is_explained() {
        assert_eq!(
            AddCutError::OutsideVideo.to_string(),
            "That time is outside the video."
        );
    }

    #[test]
    fn a_missing_keyframe_is_explained() {
        assert_eq!(
            AddCutError::NoKeyframeToCutOn.to_string(),
            "There is no point near that time where the video can be cut."
        );
    }

    #[test]
    fn a_repeated_cut_is_explained_with_the_time_it_is_already_at() {
        assert_eq!(
            AddCutError::AlreadyProposed { at: 4.0 }.to_string(),
            "There is already a split point at 00:00:04.000."
        );
    }

    /// Runs edits of every kind on one plan, and calls `check` on the plan
    /// before the first edit and after each one.
    fn check_after_every_edit(check: impl Fn(&SplitPlan)) {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        let detected = plan.snapshot();
        check(&plan);

        plan.add_cut_near(5.75).unwrap();
        check(&plan);
        plan.merge_scenes(scenes(0, 2));
        check(&plan);
        plan.isolate_segment(SegmentIndex(1));
        check(&plan);
        plan.merge_scenes(scenes(1, 3));
        check(&plan);
        plan.add_cut_near(4.25).unwrap();
        check(&plan);
        plan.restore_splits_within(span(4.0, 10.0));
        check(&plan);
        plan.restore(detected);
        check(&plan);
        plan.reset_to_detected();
        check(&plan);
    }

    #[test]
    fn scenes_always_cover_the_whole_video_without_gaps() {
        check_after_every_edit(|plan| {
            let scenes = plan.scenes();
            assert_eq!(
                scenes.first().map(|scene| scene.start),
                Some(WHOLE_VIDEO.start)
            );
            assert_eq!(scenes.last().map(|scene| scene.end), Some(WHOLE_VIDEO.end));
            for neighbours in scenes.windows(2) {
                assert_eq!(neighbours[0].end, neighbours[1].start);
            }
        });
    }

    #[test]
    fn there_is_always_one_more_segment_than_cuts() {
        check_after_every_edit(|plan| {
            assert_eq!(plan.segments().len(), plan.cuts.len() + 1);
        });
    }

    #[test]
    fn scene_layouts_always_match_the_scenes_and_hold_every_segment_once() {
        check_after_every_edit(|plan| {
            let layouts = plan.scene_layouts();

            let spans: Vec<TimeSpan> = layouts.iter().map(|layout| layout.span).collect();
            assert_eq!(spans, plan.scenes());

            let segments: Vec<TimeSpan> = layouts
                .iter()
                .flat_map(|layout| layout.segments.iter().copied())
                .collect();
            assert_eq!(segments, plan.segments());

            let mut segments_before = 0;
            for layout in &layouts {
                assert_eq!(layout.first_segment, SegmentIndex(segments_before));
                assert_eq!(layout.merged_cuts.len() + 1, layout.segments.len());
                segments_before += layout.segments.len();
            }
        });
    }

    #[test]
    fn a_detected_cut_is_marked_as_detected_whatever_the_scan_saw_there() {
        let plan = plan_with_cuts(&[2.0, 8.0]);
        assert!(plan
            .cuts
            .iter()
            .all(|cut| cut.source == CutSource::Detected));
    }

    #[test]
    fn scenes_are_numbered_for_people_from_one() {
        let first = SceneIndex::new(0);
        assert_eq!(first.position(), 0);
        assert_eq!(first.number(), 1);
        assert_eq!(first.next(), SceneIndex::new(1));
    }

    #[test]
    fn scene_indices_order_as_their_scenes_do() {
        assert!(SceneIndex::new(1) < SceneIndex::new(2));
        assert!(scenes(1, 3).contains(&SceneIndex::new(2)));
        assert!(!scenes(1, 3).contains(&SceneIndex::new(4)));
    }

    #[test]
    fn the_first_segment_has_no_cut_before_it() {
        assert_eq!(SegmentIndex(0).cut_before(), None);
        assert_eq!(SegmentIndex(0).cut_after(), CutIndex(0));
    }

    #[test]
    fn a_later_segment_sits_between_two_neighbouring_cuts() {
        assert_eq!(SegmentIndex(3).cut_before(), Some(CutIndex(2)));
        assert_eq!(SegmentIndex(3).cut_after(), CutIndex(3));
    }

    #[test]
    fn a_layout_numbers_its_segments_across_the_whole_plan() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(2, 4));
        let merged = &plan.scene_layouts()[2];

        assert_eq!(merged.segment_index(0), SegmentIndex(2));
        assert_eq!(merged.segment_index(2), SegmentIndex(4));
    }

    #[test]
    fn a_segment_named_by_its_layout_can_be_moved_out() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(2, 4));
        let middle_of_the_merged_scene = plan.scene_layouts()[2].segment_index(1);

        plan.isolate_segment(middle_of_the_merged_scene);
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 4.0, 6.0, 8.0]);
    }

    #[test]
    fn a_merged_cut_named_by_its_layout_can_be_split_again() {
        let mut plan = plan_with_cuts(&[4.0, 8.0]);
        plan.merge_scenes(scenes(0, 1));
        let merged_cut = plan.scene_layouts()[0].merged_cuts[0];
        assert_eq!(plan.cut_time(merged_cut), 4.0);

        plan.set_choice(merged_cut, CutChoice::Split);
        assert!(plan.is_as_detected());
    }

    #[test]
    fn a_time_that_is_not_a_number_is_outside_the_video() {
        let plan = plan_with_cuts(&[]);
        assert_eq!(plan.cut_time_near(f64::NAN), Err(AddCutError::OutsideVideo));
    }

    #[test]
    fn the_plan_covers_the_video_its_detected_cuts_belong_to() {
        assert_eq!(plan_with_cuts(&[2.0]).whole_video(), WHOLE_VIDEO);
    }
}
