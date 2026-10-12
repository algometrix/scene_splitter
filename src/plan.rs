//! The split plan a person edits during review: every proposed cut, and
//! whether to split there or merge the footage on either side.

use std::fmt;
use std::ops::RangeInclusive;

use crate::black::TimeSpan;
use crate::scenes;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CutSource {
    BlackScreen,
    AddedByUser,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CutChoice {
    Split,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProposedCut {
    pub time: f64,
    pub source: CutSource,
    pub choice: CutChoice,
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
                write!(formatter, "There is no keyframe near that time to cut on.")
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
    pub first_segment: usize,
    /// The merged cuts between its segments, as indices into the plan's
    /// cuts. Always one fewer than the segments.
    pub merged_cuts: Vec<usize>,
}

#[derive(Debug)]
pub struct SplitPlan {
    video: TimeSpan,
    keyframe_times: Vec<f64>,
    /// In ascending order of time, with no duplicates.
    cuts: Vec<ProposedCut>,
}

impl SplitPlan {
    /// Starts with every detected cut set to split.
    pub fn from_detected_cuts(
        detected_cuts: &[f64],
        keyframe_times: Vec<f64>,
        video: TimeSpan,
    ) -> Self {
        let cuts = detected_cuts
            .iter()
            .map(|&time| ProposedCut {
                time,
                source: CutSource::BlackScreen,
                choice: CutChoice::Split,
            })
            .collect();
        Self {
            video,
            keyframe_times,
            cuts,
        }
    }

    pub fn video(&self) -> TimeSpan {
        self.video
    }

    pub fn cuts(&self) -> &[ProposedCut] {
        &self.cuts
    }

    /// The stretches of footage between all proposed cuts, merged or not.
    /// There is always one more segment than there are cuts.
    pub fn segments(&self) -> Vec<TimeSpan> {
        let every_cut: Vec<f64> = self.cuts.iter().map(|cut| cut.time).collect();
        scenes::scenes_between(&every_cut, self.video)
    }

    /// The scenes that splitting would produce right now.
    pub fn scenes(&self) -> Vec<TimeSpan> {
        scenes::scenes_between(&self.accepted_cuts(), self.video)
    }

    /// The current scenes in order, each with the segments it is made of.
    pub fn scene_layouts(&self) -> Vec<SceneLayout> {
        let mut layouts: Vec<SceneLayout> = Vec::new();
        for (segment_index, segment) in self.segments().into_iter().enumerate() {
            let cut_before = segment_index.checked_sub(1);
            let merged_into_previous =
                cut_before.is_some_and(|cut| self.cuts[cut].choice == CutChoice::Merge);
            match (layouts.last_mut(), cut_before) {
                (Some(layout), Some(cut)) if merged_into_previous => {
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
    pub fn merge_scenes(&mut self, scenes: RangeInclusive<usize>) {
        // The nth accepted cut is the one between scene n and scene n + 1.
        let accepted_cut_indices: Vec<usize> = self
            .cuts
            .iter()
            .enumerate()
            .filter(|(_, cut)| cut.choice == CutChoice::Split)
            .map(|(index, _)| index)
            .collect();
        for boundary in *scenes.start()..*scenes.end() {
            if let Some(&cut_index) = accepted_cut_indices.get(boundary) {
                self.cuts[cut_index].choice = CutChoice::Merge;
            }
        }
    }

    /// Takes one segment out of a merged scene and makes it a scene of its
    /// own, by restoring the split on each side of it. The segments before
    /// and after it keep whatever else they were merged with.
    pub fn isolate_segment(&mut self, segment_index: usize) {
        // Cut n sits between segment n and segment n + 1.
        let cut_before = segment_index.checked_sub(1);
        let cut_after = segment_index;
        for cut_index in cut_before.into_iter().chain([cut_after]) {
            if let Some(cut) = self.cuts.get_mut(cut_index) {
                cut.choice = CutChoice::Split;
            }
        }
    }

    /// Goes back to the scan's own suggestion: every detected cut splits
    /// again, and the cuts the person added are gone.
    pub fn reset_to_detected(&mut self) {
        self.cuts.retain(|cut| cut.source == CutSource::BlackScreen);
        for cut in &mut self.cuts {
            cut.choice = CutChoice::Split;
        }
    }

    /// Puts back cuts saved earlier from `cuts`, to undo a change.
    pub fn restore_cuts(&mut self, cuts: Vec<ProposedCut>) {
        self.cuts = cuts;
    }

    /// Whether the plan is still exactly what the scan suggested.
    pub fn is_as_detected(&self) -> bool {
        self.cuts
            .iter()
            .all(|cut| cut.source == CutSource::BlackScreen && cut.choice == CutChoice::Split)
    }

    /// Undoes every merge strictly inside `span`.
    pub fn restore_splits_within(&mut self, span: TimeSpan) {
        for cut in &mut self.cuts {
            if cut.time > span.start && cut.time < span.end {
                cut.choice = CutChoice::Split;
            }
        }
    }

    pub fn accepted_cuts(&self) -> Vec<f64> {
        self.cuts
            .iter()
            .filter(|cut| cut.choice == CutChoice::Split)
            .map(|cut| cut.time)
            .collect()
    }

    pub fn set_choice(&mut self, cut_index: usize, choice: CutChoice) {
        self.cuts[cut_index].choice = choice;
    }

    /// Where a cut asked for at `requested_time` would land: on the nearest
    /// keyframe, since a split without re-encoding can cut nowhere else.
    pub fn cut_time_near(&self, requested_time: f64) -> Result<f64, AddCutError> {
        if requested_time <= self.video.start || requested_time >= self.video.end {
            return Err(AddCutError::OutsideVideo);
        }
        let exact_moment = TimeSpan {
            start: requested_time,
            end: requested_time,
        };
        scenes::nearest_keyframe(&exact_moment, &self.keyframe_times)
            .filter(|&keyframe| keyframe > self.video.start && keyframe < self.video.end)
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

    const VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 10.0,
    };

    fn span(start: f64, end: f64) -> TimeSpan {
        TimeSpan { start, end }
    }

    fn plan_with_cuts(detected_cuts: &[f64]) -> SplitPlan {
        SplitPlan::from_detected_cuts(detected_cuts, vec![0.0, 2.0, 4.0, 6.0, 8.0], VIDEO)
    }

    #[test]
    fn every_detected_cut_starts_as_a_split() {
        let plan = plan_with_cuts(&[4.0, 8.0]);
        assert_eq!(plan.accepted_cuts(), vec![4.0, 8.0]);
        assert_eq!(plan.scenes().len(), 3);
    }

    #[test]
    fn merging_a_cut_joins_its_scenes_but_keeps_the_segments() {
        let mut plan = plan_with_cuts(&[4.0, 8.0]);
        plan.set_choice(0, CutChoice::Merge);

        assert_eq!(plan.scenes(), vec![span(0.0, 8.0), span(8.0, 10.0)]);
        assert_eq!(plan.segments().len(), 3);
    }

    #[test]
    fn merging_a_run_of_scenes_leaves_the_others_alone() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(1..=3);
        assert_eq!(
            plan.scenes(),
            vec![span(0.0, 2.0), span(2.0, 8.0), span(8.0, 10.0)]
        );
    }

    #[test]
    fn scene_numbers_count_the_scenes_as_they_are_now() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(0..=1);
        // After the first merge the scenes are 0-4, 4-6, 6-8 and 8-10.
        plan.merge_scenes(1..=2);
        assert_eq!(
            plan.scenes(),
            vec![span(0.0, 4.0), span(4.0, 8.0), span(8.0, 10.0)]
        );
    }

    #[test]
    fn a_scene_layout_lists_its_merged_segments_and_cuts() {
        let mut plan = plan_with_cuts(&[4.0, 8.0]);
        plan.merge_scenes(0..=1);
        assert_eq!(
            plan.scene_layouts(),
            vec![
                SceneLayout {
                    span: span(0.0, 8.0),
                    segments: vec![span(0.0, 4.0), span(4.0, 8.0)],
                    first_segment: 0,
                    merged_cuts: vec![0],
                },
                SceneLayout {
                    span: span(8.0, 10.0),
                    segments: vec![span(8.0, 10.0)],
                    first_segment: 2,
                    merged_cuts: vec![],
                },
            ]
        );
    }

    #[test]
    fn a_middle_segment_moved_out_leaves_a_scene_on_each_side() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(0..=4);

        plan.isolate_segment(2);
        assert_eq!(
            plan.scenes(),
            vec![span(0.0, 4.0), span(4.0, 6.0), span(6.0, 10.0)]
        );
    }

    #[test]
    fn the_first_segment_moved_out_leaves_the_rest_merged() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(0..=4);

        plan.isolate_segment(0);
        assert_eq!(plan.scenes(), vec![span(0.0, 2.0), span(2.0, 10.0)]);
    }

    #[test]
    fn the_last_segment_moved_out_leaves_the_rest_merged() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(0..=4);

        plan.isolate_segment(4);
        assert_eq!(plan.scenes(), vec![span(0.0, 8.0), span(8.0, 10.0)]);
    }

    #[test]
    fn moving_a_segment_out_does_not_touch_other_merged_scenes() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(0..=1);
        // The scenes are now 0-4, 4-6, 6-8 and 8-10.
        plan.merge_scenes(2..=3);

        plan.isolate_segment(0);
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

        plan.merge_scenes(0..=1);
        plan.add_cut_near(5.75).unwrap();
        assert!(!plan.is_as_detected());

        plan.reset_to_detected();
        assert!(plan.is_as_detected());
        assert_eq!(plan.accepted_cuts(), vec![2.0, 8.0]);
    }

    #[test]
    fn saved_cuts_can_be_put_back() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        let saved = plan.cuts().to_vec();

        plan.merge_scenes(0..=2);
        plan.restore_cuts(saved);
        assert_eq!(plan.accepted_cuts(), vec![2.0, 8.0]);
    }

    #[test]
    fn restoring_splits_only_touches_the_given_scene() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(0..=1);
        plan.merge_scenes(1..=2);

        plan.restore_splits_within(span(0.0, 4.0));
        assert_eq!(plan.accepted_cuts(), vec![2.0, 4.0, 8.0]);
    }

    #[test]
    fn a_merged_cut_can_be_split_again() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.set_choice(0, CutChoice::Merge);
        plan.set_choice(0, CutChoice::Split);
        assert_eq!(plan.accepted_cuts(), vec![4.0]);
    }

    #[test]
    fn an_added_cut_snaps_to_the_nearest_keyframe_and_stays_in_order() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);

        assert_eq!(plan.add_cut_near(5.75), Ok(6.0));
        assert_eq!(plan.accepted_cuts(), vec![2.0, 6.0, 8.0]);
        assert_eq!(plan.cuts()[1].source, CutSource::AddedByUser);
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
        plan.set_choice(0, CutChoice::Merge);

        assert_eq!(plan.add_cut_near(4.25), Ok(4.0));
        assert_eq!(plan.accepted_cuts(), vec![4.0]);
        assert!(plan.is_as_detected());
    }

    #[test]
    fn asking_where_a_cut_would_land_changes_nothing() {
        let plan = plan_with_cuts(&[2.0]);
        assert_eq!(plan.cut_time_near(5.75), Ok(6.0));
        assert_eq!(plan.accepted_cuts(), vec![2.0]);
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
}
