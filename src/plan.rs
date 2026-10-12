//! The split plan a person edits during review: every proposed cut, and
//! whether to split there or merge the footage on either side.
//!
//! A plan counts three different things from zero, and each has its own
//! index type so one cannot be passed for another: scenes (what the split
//! would write now), segments (the stretches between all proposed cuts,
//! merged or not) and the cuts themselves.

use std::fmt;
use std::ops::RangeInclusive;

use crate::exact_cut::CutPrecision;
use crate::fates::{NothingToWrite, SceneFate, ScenesToWrite};
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
    /// Suggested by the scan at `detected_at`, and moved by the person
    /// since. The time is kept so that a reset can put the cut back.
    DetectedThenMoved {
        detected_at: f64,
    },
    /// A chapter mark of the file, where the scan itself found nothing.
    Chapter,
    /// A chapter cut that stood at `marked_at`, and was moved by the person
    /// since. It still comes from the chapter mark, and a reset puts it
    /// back.
    ChapterThenMoved {
        marked_at: f64,
    },
    AddedByUser,
}

impl CutSource {
    /// Where a cut of this source comes from once it has been moved from
    /// `from` to `to`. A suggested cut moved back to where the review began
    /// with it is as suggested again.
    fn after_a_move(self, from: f64, to: f64) -> Self {
        match self {
            Self::Detected => Self::DetectedThenMoved { detected_at: from },
            Self::DetectedThenMoved { detected_at } if detected_at == to => Self::Detected,
            Self::Chapter => Self::ChapterThenMoved { marked_at: from },
            Self::ChapterThenMoved { marked_at } if marked_at == to => Self::Chapter,
            Self::DetectedThenMoved { .. } | Self::ChapterThenMoved { .. } | Self::AddedByUser => {
                self
            }
        }
    }

    /// Whether the cut was there when the review began, moved since or not,
    /// as opposed to one the person added.
    fn is_suggested(self) -> bool {
        match self {
            Self::Detected
            | Self::DetectedThenMoved { .. }
            | Self::Chapter
            | Self::ChapterThenMoved { .. } => true,
            Self::AddedByUser => false,
        }
    }

    /// Whether the cut is one the review began with, still where it was.
    fn is_untouched_suggestion(self) -> bool {
        match self {
            Self::Detected | Self::Chapter => true,
            Self::DetectedThenMoved { .. } | Self::ChapterThenMoved { .. } | Self::AddedByUser => {
                false
            }
        }
    }

    /// Whether the cut exists only because of a chapter mark of the file.
    fn is_from_a_chapter_mark(self) -> bool {
        match self {
            Self::Chapter | Self::ChapterThenMoved { .. } => true,
            Self::Detected | Self::DetectedThenMoved { .. } | Self::AddedByUser => false,
        }
    }

    /// The source and the time the review began with, for a suggested cut
    /// the person has moved.
    fn before_it_was_moved(self) -> Option<(Self, f64)> {
        match self {
            Self::DetectedThenMoved { detected_at } => Some((Self::Detected, detected_at)),
            Self::ChapterThenMoved { marked_at } => Some((Self::Chapter, marked_at)),
            Self::Detected | Self::Chapter | Self::AddedByUser => None,
        }
    }
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

/// Which way a split is moved along the video.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NudgeDirection {
    Earlier,
    Later,
}

/// A split that was moved to the next point where the video can be cut.
/// How far one nudge moves a split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NudgeStep {
    /// To the keyframe beside it, the nearest place where a split that
    /// copies can cut.
    ToNextCutPoint,
    /// By the time one picture is shown, for a split that cuts on the exact
    /// picture.
    OnePicture { seconds: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovedCut {
    pub from: f64,
    pub to: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NudgeCutError {
    /// The first scene starts where the video starts, not at a split, and
    /// a scene past the last one does not exist.
    NoSplitBeforeScene,
    /// The video has no keyframe further that way to cut on.
    NoCutPoint(NudgeDirection),
    /// The next keyframe that way is at or beyond the neighbouring split.
    SplitInTheWay { at: f64 },
    /// The next keyframe that way is at or beyond a cut the person merged
    /// away, where two parts of a merged scene meet.
    MergedPartsInTheWay { at: f64 },
}

impl fmt::Display for NudgeCutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        use crate::timecode::format_timestamp;
        match self {
            Self::NoSplitBeforeScene => write!(
                formatter,
                "This scene starts where the video starts, so there is no split to move."
            ),
            Self::NoCutPoint(NudgeDirection::Earlier) => write!(
                formatter,
                "There is no earlier point where the video can be cut."
            ),
            Self::NoCutPoint(NudgeDirection::Later) => write!(
                formatter,
                "There is no later point where the video can be cut."
            ),
            Self::SplitInTheWay { at } => write!(
                formatter,
                "The split cannot move further. The split at {} is in the way.",
                format_timestamp(*at)
            ),
            Self::MergedPartsInTheWay { at } => write!(
                formatter,
                "The split cannot move further. Two merged parts meet at {}.",
                format_timestamp(*at)
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
    fates: Vec<SceneFate>,
}

#[derive(Debug)]
pub struct SplitPlan {
    whole_video: TimeSpan,
    keyframe_times: Vec<f64>,
    /// In ascending order of time, with no duplicates, each strictly inside
    /// `whole_video`.
    cuts: Vec<ProposedCut>,
    /// Whether each segment is written or dropped, one per segment. The
    /// segments of one scene always share a fate, and at least one segment
    /// is kept.
    fates: Vec<SceneFate>,
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
            fates: vec![SceneFate::Keep; detected_cuts.len() + 1],
            cuts,
        }
    }

    /// Adds the cuts that come from the file's chapter marks, each set to
    /// split. Meant for a plan nobody has edited yet. A time outside the
    /// video, or one that is proposed already, is left out.
    pub fn propose_chapter_cuts(&mut self, chapter_cut_times: &[f64]) {
        for &time in chapter_cut_times {
            if !self.whole_video.has_strictly_inside(time) {
                continue;
            }
            let position = self.cuts.partition_point(|cut| cut.time < time);
            if self.cuts.get(position).is_some_and(|cut| cut.time == time) {
                continue;
            }
            self.cuts.insert(
                position,
                ProposedCut {
                    time,
                    source: CutSource::Chapter,
                    choice: CutChoice::Split,
                },
            );
            // The new cut divides segment `position` in two. Both halves
            // are written or dropped as the whole was.
            self.fates.insert(position, self.fates[position]);
        }
    }

    /// How many splits the plan has now only because of a chapter mark.
    pub fn chapter_split_count(&self) -> usize {
        self.cuts
            .iter()
            .filter(|cut| cut.source.is_from_a_chapter_mark() && cut.choice == CutChoice::Split)
            .count()
    }

    /// Merges the footage on both sides of every split that comes from a
    /// chapter mark, for a person who wants only what the scan found.
    pub fn merge_chapter_cuts(&mut self) {
        for cut in &mut self.cuts {
            if cut.source.is_from_a_chapter_mark() {
                cut.choice = CutChoice::Merge;
            }
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
        let accepted_cut_positions = self.accepted_cut_positions();
        for boundary in scenes.start().0..scenes.end().0 {
            if let Some(&cut_position) = accepted_cut_positions.get(boundary) {
                self.cuts[cut_position].choice = CutChoice::Merge;
            }
        }
        self.keep_scenes_with_a_kept_part();
    }

    /// What the split does with each of the current scenes, in scene order.
    pub fn scene_fates(&self) -> Vec<SceneFate> {
        self.scene_layouts()
            .iter()
            .map(|layout| self.fates[layout.first_segment.0])
            .collect()
    }

    /// Marks one of the current scenes as written or dropped. Dropping the
    /// last scene that is still written is refused, since a split has to
    /// write something.
    pub fn set_scene_fate(
        &mut self,
        scene: SceneIndex,
        fate: SceneFate,
    ) -> Result<(), NothingToWrite> {
        let layouts = self.scene_layouts();
        let Some(layout) = layouts.get(scene.position()) else {
            return Ok(());
        };
        let segments = layout.first_segment.0..layout.first_segment.0 + layout.segments.len();
        let kept_elsewhere = self
            .fates
            .iter()
            .enumerate()
            .any(|(segment, &fate)| fate == SceneFate::Keep && !segments.contains(&segment));
        if fate == SceneFate::Drop && !kept_elsewhere {
            return Err(NothingToWrite::EverySceneDropped);
        }
        self.fates[segments].fill(fate);
        Ok(())
    }

    /// The cuts and the scenes to write, as a split takes them.
    pub fn scenes_to_write(&self) -> ScenesToWrite {
        // `set_scene_fate` refuses to drop the last kept scene, and no other
        // edit drops anything.
        ScenesToWrite::new(self.accepted_cuts(), self.scene_fates())
            .expect("a plan has one fate per scene and keeps at least one scene")
    }

    /// A scene made by merging a dropped scene with a kept one is kept as a
    /// whole. Merging must never be a way to lose footage unnoticed, and a
    /// scene that is half dropped could not be shown or written.
    fn keep_scenes_with_a_kept_part(&mut self) {
        for layout in self.scene_layouts() {
            let segments = layout.first_segment.0..layout.first_segment.0 + layout.segments.len();
            if self.fates[segments.clone()].contains(&SceneFate::Keep) {
                self.fates[segments].fill(SceneFate::Keep);
            }
        }
    }

    /// Where in `cuts` each cut that splits sits. The nth of them is the
    /// one between scene n and scene n + 1.
    fn accepted_cut_positions(&self) -> Vec<usize> {
        self.cuts
            .iter()
            .enumerate()
            .filter(|(_, cut)| cut.choice == CutChoice::Split)
            .map(|(position, _)| position)
            .collect()
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

    /// Goes back to what the review began with: every detected cut and
    /// every chapter cut splits again where it stood, the cuts the person
    /// added are gone, and no scene is dropped.
    pub fn reset_to_detected(&mut self) {
        self.cuts.retain(|cut| cut.source.is_suggested());
        for cut in &mut self.cuts {
            // A moved cut never passes another cut, and suggested cuts are
            // never removed, so putting the times back keeps the order.
            if let Some((source, time)) = cut.source.before_it_was_moved() {
                cut.time = time;
                cut.source = source;
            }
            cut.choice = CutChoice::Split;
        }
        self.fates = vec![SceneFate::Keep; self.cuts.len() + 1];
    }

    /// The plan as it is now, to come back to with `restore`.
    pub fn snapshot(&self) -> PlanSnapshot {
        PlanSnapshot {
            cuts: self.cuts.clone(),
            fates: self.fates.clone(),
        }
    }

    /// Puts the plan back to how it was when `snapshot` was taken of it.
    pub fn restore(&mut self, snapshot: PlanSnapshot) {
        self.cuts = snapshot.cuts;
        self.fates = snapshot.fates;
    }

    /// Whether nothing has changed since `snapshot` was taken.
    pub fn is_at(&self, snapshot: &PlanSnapshot) -> bool {
        self.cuts == snapshot.cuts && self.fates == snapshot.fates
    }

    /// Whether the plan is still exactly what the review began with.
    pub fn is_as_detected(&self) -> bool {
        let no_scene_dropped = !self.fates.contains(&SceneFate::Drop);
        no_scene_dropped
            && self
                .cuts
                .iter()
                .all(|cut| cut.source.is_untouched_suggestion() && cut.choice == CutChoice::Split)
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
        self.keep_scenes_with_a_kept_part();
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

    /// Where a cut asked for at `requested_time` would land with the given
    /// precision: on the nearest keyframe, or at the very time asked for.
    pub fn cut_time_for(
        &self,
        requested_time: f64,
        precision: CutPrecision,
    ) -> Result<f64, AddCutError> {
        match precision {
            CutPrecision::NearestKeyframe => self.cut_time_near(requested_time),
            CutPrecision::ExactFrame if self.whole_video.has_strictly_inside(requested_time) => {
                Ok(requested_time)
            }
            CutPrecision::ExactFrame => Err(AddCutError::OutsideVideo),
        }
    }

    /// Adds a cut on the keyframe nearest to `requested_time` and returns the
    /// time it landed on.
    pub fn add_cut_near(&mut self, requested_time: f64) -> Result<f64, AddCutError> {
        let time = self.cut_time_for(requested_time, CutPrecision::NearestKeyframe)?;
        self.add_cut_at(time)
    }

    /// Adds a cut at the very time asked for, which only a split that
    /// encodes the pictures around its cuts again can cut at.
    pub fn add_cut_exactly_at(&mut self, requested_time: f64) -> Result<f64, AddCutError> {
        let time = self.cut_time_for(requested_time, CutPrecision::ExactFrame)?;
        self.add_cut_at(time)
    }

    /// `time` is one that `cut_time_for` gave.
    fn add_cut_at(&mut self, time: f64) -> Result<f64, AddCutError> {
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
        // The new cut divides segment `position` in two. Both halves are
        // written or dropped as the whole was.
        self.fates.insert(position, self.fates[position]);
        Ok(time)
    }

    /// Moves the split at which `scene` starts one `step` in the given
    /// direction, and returns where it was and where it is now.
    ///
    /// The split stays between the cuts on either side of it, merged ones
    /// included. Past a merged cut it would take a part of one merged scene
    /// into another, which nobody asked for.
    pub fn nudge_cut_before(
        &mut self,
        scene: SceneIndex,
        direction: NudgeDirection,
        step: NudgeStep,
    ) -> Result<MovedCut, NudgeCutError> {
        let position = self
            .accepted_cut_position_before(scene)
            .ok_or(NudgeCutError::NoSplitBeforeScene)?;
        let from = self.cuts[position].time;
        let to = self
            .one_step_from(from, direction, step)
            .ok_or(NudgeCutError::NoCutPoint(direction))?;
        self.refuse_a_move_onto_or_past_the_neighbour(position, to, direction)?;

        let cut = &mut self.cuts[position];
        cut.source = cut.source.after_a_move(from, to);
        cut.time = to;
        Ok(MovedCut { from, to })
    }

    /// `None` for the first scene, which starts where the video starts, and
    /// for a scene the plan does not have.
    fn accepted_cut_position_before(&self, scene: SceneIndex) -> Option<usize> {
        let boundary = scene.0.checked_sub(1)?;
        self.accepted_cut_positions().get(boundary).copied()
    }

    /// Where a split at `time` is after one step, or `None` when that would
    /// take it out of the video.
    fn one_step_from(&self, time: f64, direction: NudgeDirection, step: NudgeStep) -> Option<f64> {
        let seconds = match step {
            NudgeStep::ToNextCutPoint => return self.cut_point_beside(time, direction),
            NudgeStep::OnePicture { seconds } => seconds,
        };
        // A video whose picture length is not known has no picture to step
        // by, and a step of nothing would look like a move.
        if !(seconds.is_finite() && seconds > 0.0) {
            return None;
        }
        let stepped = match direction {
            NudgeDirection::Earlier => time - seconds,
            NudgeDirection::Later => time + seconds,
        };
        Some(stepped).filter(|&stepped| self.whole_video.has_strictly_inside(stepped))
    }

    /// Where the review began with the cut that is at `cut_time` now. That
    /// is `cut_time` itself unless the person moved a suggested cut there.
    /// What the file says about the footage from a cut on, such as a
    /// chapter title, is found by this time.
    pub fn time_the_review_began_with(&self, cut_time: f64) -> f64 {
        self.cuts
            .iter()
            .find(|cut| cut.time == cut_time)
            .and_then(|cut| cut.source.before_it_was_moved())
            .map_or(cut_time, |(_source, time)| time)
    }

    /// The keyframe next to `time` on the given side that a cut can be on.
    fn cut_point_beside(&self, time: f64, direction: NudgeDirection) -> Option<f64> {
        let keyframes = &self.keyframe_times;
        let beside = match direction {
            NudgeDirection::Earlier => keyframes
                .partition_point(|&keyframe| keyframe < time)
                .checked_sub(1)
                .map(|before| keyframes[before]),
            NudgeDirection::Later => keyframes
                .get(keyframes.partition_point(|&keyframe| keyframe <= time))
                .copied(),
        };
        beside.filter(|&keyframe| self.whole_video.has_strictly_inside(keyframe))
    }

    fn refuse_a_move_onto_or_past_the_neighbour(
        &self,
        position: usize,
        to: f64,
        direction: NudgeDirection,
    ) -> Result<(), NudgeCutError> {
        let neighbour = match direction {
            NudgeDirection::Earlier => position.checked_sub(1).map(|before| self.cuts[before]),
            NudgeDirection::Later => self.cuts.get(position + 1).copied(),
        };
        // With no cut on that side the end of the video is the limit, and
        // `cut_point_beside` has kept the split inside the video already.
        let Some(neighbour) = neighbour else {
            return Ok(());
        };
        let reaches_the_neighbour = match direction {
            NudgeDirection::Earlier => to <= neighbour.time,
            NudgeDirection::Later => to >= neighbour.time,
        };
        if !reaches_the_neighbour {
            return Ok(());
        }
        Err(match neighbour.choice {
            CutChoice::Split => NudgeCutError::SplitInTheWay { at: neighbour.time },
            CutChoice::Merge => NudgeCutError::MergedPartsInTheWay { at: neighbour.time },
        })
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
    fn an_exact_cut_stays_at_the_time_asked_for() {
        let mut plan = plan_with_cuts(&[4.0]);
        assert_eq!(plan.cut_time_for(5.3, CutPrecision::ExactFrame), Ok(5.3));
        assert_eq!(plan.add_cut_exactly_at(5.3), Ok(5.3));
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0, 5.3]);
        // The same request on keyframes lands on the keyframe at 6 s.
        assert_eq!(plan.add_cut_near(5.3), Ok(6.0));
    }

    #[test]
    fn an_exact_cut_outside_the_video_is_refused() {
        let mut plan = plan_with_cuts(&[]);
        for outside in [WHOLE_VIDEO.start, WHOLE_VIDEO.end, 12.0, -1.0] {
            assert_eq!(
                plan.add_cut_exactly_at(outside),
                Err(AddCutError::OutsideVideo)
            );
        }
    }

    #[test]
    fn an_exact_cut_given_twice_is_refused_the_second_time() {
        let mut plan = plan_with_cuts(&[]);
        plan.add_cut_exactly_at(5.3).unwrap();
        assert_eq!(
            plan.add_cut_exactly_at(5.3),
            Err(AddCutError::AlreadyProposed { at: 5.3 })
        );
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

    const EARLIER: NudgeDirection = NudgeDirection::Earlier;
    const LATER: NudgeDirection = NudgeDirection::Later;
    const TO_CUT_POINT: NudgeStep = NudgeStep::ToNextCutPoint;
    /// One picture of a video with 25 pictures a second.
    const ONE_PICTURE: NudgeStep = NudgeStep::OnePicture { seconds: 0.04 };

    #[test]
    fn with_exact_cuts_a_split_moves_by_one_picture() {
        let mut plan = plan_with_cuts(&[4.0]);
        let moved = plan
            .nudge_cut_before(second_scene(), LATER, ONE_PICTURE)
            .unwrap();
        assert_eq!(
            moved,
            MovedCut {
                from: 4.0,
                to: 4.04
            }
        );
        assert_eq!(plan.accepted_cuts().as_slice(), [4.04]);

        let back = plan
            .nudge_cut_before(second_scene(), EARLIER, ONE_PICTURE)
            .unwrap();
        assert_eq!(back.from, 4.04);
        assert!((back.to - 4.0).abs() < 1e-9);
    }

    #[test]
    fn a_step_of_one_picture_stops_at_the_neighbour_and_inside_the_video() {
        let mut plan = plan_with_keyframes(&[], vec![0.0]);
        plan.add_cut_exactly_at(0.03).unwrap();
        plan.add_cut_exactly_at(0.06).unwrap();
        plan.add_cut_exactly_at(9.98).unwrap();
        assert_eq!(
            plan.nudge_cut_before(SceneIndex::new(1), EARLIER, ONE_PICTURE),
            Err(NudgeCutError::NoCutPoint(EARLIER))
        );
        assert_eq!(
            plan.nudge_cut_before(SceneIndex::new(1), LATER, ONE_PICTURE),
            Err(NudgeCutError::SplitInTheWay { at: 0.06 })
        );
        assert_eq!(
            plan.nudge_cut_before(SceneIndex::new(3), LATER, ONE_PICTURE),
            Err(NudgeCutError::NoCutPoint(LATER))
        );
    }

    #[test]
    fn a_picture_of_no_length_is_no_step() {
        let mut plan = plan_with_cuts(&[4.0]);
        for seconds in [0.0, -0.04, f64::NAN, f64::INFINITY] {
            assert_eq!(
                plan.nudge_cut_before(second_scene(), LATER, NudgeStep::OnePicture { seconds }),
                Err(NudgeCutError::NoCutPoint(LATER))
            );
        }
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0]);
    }

    #[test]
    fn a_moved_chapter_cut_still_comes_from_its_chapter_and_a_reset_puts_it_back() {
        let mut plan = plan_with_cuts(&[]);
        plan.propose_chapter_cuts(&[4.0]);
        plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT)
            .unwrap();
        assert_eq!(plan.accepted_cuts().as_slice(), [6.0]);
        assert_eq!(plan.chapter_split_count(), 1);
        assert_eq!(plan.time_the_review_began_with(6.0), 4.0);
        assert!(!plan.is_as_detected());

        plan.merge_chapter_cuts();
        assert!(plan.accepted_cuts().is_empty());

        plan.reset_to_detected();
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0]);
        assert!(plan.is_as_detected());
    }

    #[test]
    fn a_chapter_cut_moved_back_to_its_mark_counts_as_untouched() {
        let mut plan = plan_with_cuts(&[]);
        plan.propose_chapter_cuts(&[4.0]);
        plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT)
            .unwrap();
        plan.nudge_cut_before(second_scene(), EARLIER, TO_CUT_POINT)
            .unwrap();
        assert!(plan.is_as_detected());
        assert_eq!(plan.time_the_review_began_with(4.0), 4.0);
    }

    #[test]
    fn a_cut_nobody_moved_began_where_it_is() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.add_cut_near(8.0).unwrap();
        for time in [4.0, 8.0, 5.0] {
            assert_eq!(plan.time_the_review_began_with(time), time);
        }
        plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT)
            .unwrap();
        assert_eq!(plan.time_the_review_began_with(6.0), 4.0);
    }

    #[test]
    fn a_chapter_cut_added_to_a_plan_keeps_one_fate_per_segment() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.propose_chapter_cuts(&[2.0, 8.0]);
        assert_eq!(plan.scene_fates().len(), plan.scenes().len());
        plan.set_scene_fate(SceneIndex::new(3), SceneFate::Drop)
            .unwrap();
        assert_eq!(plan.scenes_to_write().dropped_count(), 1);
    }

    fn second_scene() -> SceneIndex {
        SceneIndex::new(1)
    }

    #[test]
    fn a_split_moves_to_the_next_point_where_the_video_can_be_cut() {
        let mut plan = plan_with_cuts(&[4.0]);

        assert_eq!(
            plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT),
            Ok(MovedCut { from: 4.0, to: 6.0 })
        );
        assert_eq!(plan.accepted_cuts().as_slice(), [6.0]);

        assert_eq!(
            plan.nudge_cut_before(second_scene(), EARLIER, TO_CUT_POINT),
            Ok(MovedCut { from: 6.0, to: 4.0 })
        );
        assert_eq!(
            plan.nudge_cut_before(second_scene(), EARLIER, TO_CUT_POINT),
            Ok(MovedCut { from: 4.0, to: 2.0 })
        );
        assert_eq!(plan.scenes(), vec![span(0.0, 2.0), span(2.0, 10.0)]);
    }

    #[test]
    fn a_split_between_two_keyframes_moves_to_the_one_on_the_side_asked_for() {
        let mut plan = plan_with_cuts(&[5.0]);
        assert_eq!(
            plan.nudge_cut_before(second_scene(), EARLIER, TO_CUT_POINT),
            Ok(MovedCut { from: 5.0, to: 4.0 })
        );

        let mut plan = plan_with_cuts(&[5.0]);
        assert_eq!(
            plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT),
            Ok(MovedCut { from: 5.0, to: 6.0 })
        );
    }

    #[test]
    fn a_split_does_not_move_onto_or_past_the_split_beside_it() {
        let mut plan = plan_with_cuts(&[4.0, 6.0]);
        let untouched = plan.snapshot();

        assert_eq!(
            plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT),
            Err(NudgeCutError::SplitInTheWay { at: 6.0 })
        );
        assert_eq!(
            plan.nudge_cut_before(SceneIndex::new(2), EARLIER, TO_CUT_POINT),
            Err(NudgeCutError::SplitInTheWay { at: 4.0 })
        );
        assert!(plan.is_at(&untouched));
    }

    #[test]
    fn a_split_does_not_move_past_a_split_that_sits_between_two_keyframes() {
        let mut plan = plan_with_cuts(&[4.0, 5.0]);
        assert_eq!(
            plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT),
            Err(NudgeCutError::SplitInTheWay { at: 5.0 })
        );
    }

    #[test]
    fn a_split_does_not_move_into_the_parts_of_a_merged_scene() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0]);
        // The scenes are now 0-2, 2-4 and 4-10, the last made of two parts.
        plan.merge_scenes(scenes(2, 3));

        assert_eq!(
            plan.nudge_cut_before(SceneIndex::new(2), LATER, TO_CUT_POINT),
            Err(NudgeCutError::MergedPartsInTheWay { at: 6.0 })
        );
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 4.0]);
    }

    #[test]
    fn a_split_does_not_move_to_the_ends_of_the_video() {
        let mut plan = plan_with_cuts(&[2.0]);
        // The keyframe before 2 is the first frame of the video.
        assert_eq!(
            plan.nudge_cut_before(second_scene(), EARLIER, TO_CUT_POINT),
            Err(NudgeCutError::NoCutPoint(EARLIER))
        );

        let mut plan = plan_with_cuts(&[8.0]);
        assert_eq!(
            plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT),
            Err(NudgeCutError::NoCutPoint(LATER))
        );
        assert_eq!(plan.accepted_cuts().as_slice(), [8.0]);
    }

    #[test]
    fn the_first_scene_and_a_scene_that_does_not_exist_have_no_split_to_move() {
        let mut plan = plan_with_cuts(&[4.0]);
        assert_eq!(
            plan.nudge_cut_before(SceneIndex::new(0), LATER, TO_CUT_POINT),
            Err(NudgeCutError::NoSplitBeforeScene)
        );
        assert_eq!(
            plan.nudge_cut_before(SceneIndex::new(2), LATER, TO_CUT_POINT),
            Err(NudgeCutError::NoSplitBeforeScene)
        );
    }

    #[test]
    fn the_split_that_is_moved_is_the_one_the_scene_starts_at_now() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 8.0]);
        // The scenes are now 0-4, 4-8 and 8-10, so the third starts at 8.
        plan.merge_scenes(scenes(0, 1));

        assert_eq!(
            plan.nudge_cut_before(SceneIndex::new(2), EARLIER, TO_CUT_POINT),
            Ok(MovedCut { from: 8.0, to: 6.0 })
        );
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0, 6.0]);
    }

    #[test]
    fn a_reset_puts_a_moved_split_back_where_the_scan_saw_it() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT)
            .unwrap();
        assert!(!plan.is_as_detected());

        plan.reset_to_detected();
        assert!(plan.is_as_detected());
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0]);
    }

    #[test]
    fn a_reset_puts_a_split_moved_twice_back_where_the_scan_saw_it() {
        let mut plan = plan_with_cuts(&[2.0]);
        plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT)
            .unwrap();
        plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT)
            .unwrap();
        // A split added where the moved one was is the person's own.
        plan.add_cut_near(2.25).unwrap();

        plan.reset_to_detected();
        assert!(plan.is_as_detected());
        assert_eq!(plan.accepted_cuts().as_slice(), [2.0]);
    }

    #[test]
    fn a_split_moved_back_to_where_the_scan_saw_it_is_as_detected_again() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT)
            .unwrap();
        plan.nudge_cut_before(second_scene(), EARLIER, TO_CUT_POINT)
            .unwrap();
        assert!(plan.is_as_detected());
    }

    #[test]
    fn a_split_the_person_added_can_be_moved_and_a_reset_still_removes_it() {
        let mut plan = plan_with_cuts(&[]);
        plan.add_cut_near(4.0).unwrap();

        assert_eq!(
            plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT),
            Ok(MovedCut { from: 4.0, to: 6.0 })
        );
        plan.reset_to_detected();
        assert!(plan.accepted_cuts().is_empty());
    }

    #[test]
    fn a_snapshot_taken_before_a_move_puts_the_split_back() {
        let mut plan = plan_with_cuts(&[4.0]);
        let before_the_move = plan.snapshot();

        plan.nudge_cut_before(second_scene(), LATER, TO_CUT_POINT)
            .unwrap();
        assert!(!plan.is_at(&before_the_move));
        plan.restore(before_the_move);
        assert!(plan.is_as_detected());
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0]);
    }

    #[test]
    fn every_reason_a_split_cannot_move_is_explained() {
        assert_eq!(
            NudgeCutError::NoSplitBeforeScene.to_string(),
            "This scene starts where the video starts, so there is no split to move."
        );
        assert_eq!(
            NudgeCutError::NoCutPoint(EARLIER).to_string(),
            "There is no earlier point where the video can be cut."
        );
        assert_eq!(
            NudgeCutError::NoCutPoint(LATER).to_string(),
            "There is no later point where the video can be cut."
        );
        assert_eq!(
            NudgeCutError::SplitInTheWay { at: 6.0 }.to_string(),
            "The split cannot move further. The split at 00:00:06.000 is in the way."
        );
        assert_eq!(
            NudgeCutError::MergedPartsInTheWay { at: 6.0 }.to_string(),
            "The split cannot move further. Two merged parts meet at 00:00:06.000."
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
        // The added cut at 6 moves to 4.
        plan.nudge_cut_before(SceneIndex::new(2), NudgeDirection::Earlier, TO_CUT_POINT)
            .unwrap();
        check(&plan);
        plan.set_scene_fate(SceneIndex::new(1), SceneFate::Drop)
            .unwrap();
        check(&plan);
        plan.merge_scenes(scenes(0, 2));
        check(&plan);
        plan.isolate_segment(SegmentIndex(1));
        check(&plan);
        plan.set_scene_fate(SceneIndex::new(2), SceneFate::Drop)
            .unwrap();
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

    fn fates(plan: &SplitPlan) -> Vec<SceneFate> {
        plan.scene_fates()
    }

    const KEEP: SceneFate = SceneFate::Keep;
    const DROP: SceneFate = SceneFate::Drop;

    #[test]
    fn every_scene_is_written_until_one_is_dropped() {
        let mut plan = plan_with_cuts(&[4.0, 8.0]);
        assert_eq!(fates(&plan), [KEEP, KEEP, KEEP]);

        plan.set_scene_fate(SceneIndex::new(1), DROP).unwrap();
        assert_eq!(fates(&plan), [KEEP, DROP, KEEP]);
        assert_eq!(
            plan.scenes_to_write().kept_scenes(),
            vec![span(0.0, 4.0), span(8.0, 10.0)]
        );
        // The scenes themselves are as they were.
        assert_eq!(plan.scenes().len(), 3);
    }

    #[test]
    fn a_dropped_scene_can_be_kept_again() {
        let mut plan = plan_with_cuts(&[4.0, 8.0]);
        plan.set_scene_fate(SceneIndex::new(1), DROP).unwrap();
        assert!(!plan.is_as_detected());

        plan.set_scene_fate(SceneIndex::new(1), KEEP).unwrap();
        assert!(plan.is_as_detected());
    }

    #[test]
    fn dropping_the_last_scene_that_is_written_is_refused() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.set_scene_fate(SceneIndex::new(0), DROP).unwrap();

        assert_eq!(
            plan.set_scene_fate(SceneIndex::new(1), DROP),
            Err(NothingToWrite::EverySceneDropped)
        );
        assert_eq!(fates(&plan), [DROP, KEEP]);
    }

    #[test]
    fn the_only_scene_of_a_video_cannot_be_dropped() {
        let mut plan = plan_with_cuts(&[]);
        assert_eq!(
            plan.set_scene_fate(SceneIndex::new(0), DROP),
            Err(NothingToWrite::EverySceneDropped)
        );
    }

    #[test]
    fn a_fate_for_a_scene_that_does_not_exist_changes_nothing() {
        let mut plan = plan_with_cuts(&[4.0]);
        let before = plan.snapshot();
        plan.set_scene_fate(SceneIndex::new(7), DROP).unwrap();
        assert!(plan.is_at(&before));
    }

    #[test]
    fn merging_a_dropped_scene_with_a_kept_one_keeps_all_of_it() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.set_scene_fate(SceneIndex::new(1), DROP).unwrap();

        plan.merge_scenes(scenes(0, 1));
        assert_eq!(fates(&plan), [KEEP, KEEP, KEEP, KEEP]);
        // Taken apart again, both parts are written.
        plan.restore_splits_within(span(0.0, 4.0));
        assert_eq!(fates(&plan), [KEEP, KEEP, KEEP, KEEP, KEEP]);
    }

    #[test]
    fn merging_only_dropped_scenes_gives_one_dropped_scene() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.set_scene_fate(SceneIndex::new(1), DROP).unwrap();
        plan.set_scene_fate(SceneIndex::new(2), DROP).unwrap();

        plan.merge_scenes(scenes(1, 2));
        assert_eq!(fates(&plan), [KEEP, DROP, KEEP, KEEP]);
        assert_eq!(plan.scenes()[1], span(2.0, 6.0));
    }

    #[test]
    fn merging_other_scenes_leaves_a_dropped_scene_dropped() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.set_scene_fate(SceneIndex::new(4), DROP).unwrap();

        plan.merge_scenes(scenes(0, 2));
        assert_eq!(fates(&plan), [KEEP, KEEP, DROP]);
    }

    #[test]
    fn dropping_a_merged_scene_drops_every_part_of_it() {
        let mut plan = plan_with_cuts(&[2.0, 4.0, 6.0, 8.0]);
        plan.merge_scenes(scenes(1, 3));
        plan.set_scene_fate(SceneIndex::new(1), DROP).unwrap();
        assert_eq!(fates(&plan), [KEEP, DROP, KEEP]);

        // Each part taken out of it is still dropped.
        plan.isolate_segment(SegmentIndex(2));
        assert_eq!(fates(&plan), [KEEP, DROP, DROP, DROP, KEEP]);
    }

    #[test]
    fn a_split_added_inside_a_dropped_scene_leaves_both_halves_dropped() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        plan.set_scene_fate(SceneIndex::new(1), DROP).unwrap();

        plan.add_cut_near(5.75).unwrap();
        assert_eq!(fates(&plan), [KEEP, DROP, DROP, KEEP]);
    }

    #[test]
    fn a_reset_writes_every_scene_again() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        plan.add_cut_near(5.75).unwrap();
        plan.set_scene_fate(SceneIndex::new(3), DROP).unwrap();

        plan.reset_to_detected();
        assert_eq!(fates(&plan), [KEEP, KEEP, KEEP]);
        assert!(plan.is_as_detected());
    }

    #[test]
    fn a_snapshot_remembers_which_scenes_were_dropped() {
        let mut plan = plan_with_cuts(&[2.0, 8.0]);
        let nothing_dropped = plan.snapshot();
        plan.set_scene_fate(SceneIndex::new(0), DROP).unwrap();
        assert!(!plan.is_at(&nothing_dropped));
        let first_dropped = plan.snapshot();

        plan.restore(nothing_dropped);
        assert_eq!(fates(&plan), [KEEP, KEEP, KEEP]);
        plan.restore(first_dropped);
        assert_eq!(fates(&plan), [DROP, KEEP, KEEP]);
    }

    #[test]
    fn there_is_always_one_fate_per_scene_and_one_scene_to_write() {
        check_after_every_edit(|plan| {
            assert_eq!(plan.fates.len(), plan.segments().len());
            let scenes_to_write = plan.scenes_to_write();
            assert_eq!(scenes_to_write.fates().len(), plan.scenes().len());
            assert!(!scenes_to_write.kept_scenes().is_empty());
        });
    }

    #[test]
    fn chapter_cuts_split_beside_the_detected_ones_and_are_marked_as_such() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.propose_chapter_cuts(&[2.0, 8.0]);

        assert_eq!(plan.accepted_cuts().as_slice(), [2.0, 4.0, 8.0]);
        let sources: Vec<CutSource> = plan.cuts.iter().map(|cut| cut.source).collect();
        assert_eq!(
            sources,
            [CutSource::Chapter, CutSource::Detected, CutSource::Chapter]
        );
        assert_eq!(plan.chapter_split_count(), 2);
        // Nothing the person did, so there is nothing to reset.
        assert!(plan.is_as_detected());
    }

    #[test]
    fn a_chapter_cut_where_a_cut_exists_or_outside_the_video_is_left_out() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.propose_chapter_cuts(&[4.0, 0.0, 10.0, 25.0, f64::NAN]);

        assert_eq!(plan.accepted_cuts().as_slice(), [4.0]);
        assert_eq!(plan.cuts[0].source, CutSource::Detected);
        assert_eq!(plan.chapter_split_count(), 0);
    }

    #[test]
    fn a_reset_brings_chapter_cuts_back_and_keeps_them() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.propose_chapter_cuts(&[8.0]);
        plan.merge_scenes(scenes(0, 2));
        plan.add_cut_near(2.25).unwrap();

        plan.reset_to_detected();
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0, 8.0]);
        assert!(plan.is_as_detected());
    }

    #[test]
    fn leaving_out_the_chapter_cuts_keeps_the_detected_and_added_splits() {
        let mut plan = plan_with_cuts(&[4.0]);
        plan.propose_chapter_cuts(&[2.0, 8.0]);
        plan.add_cut_near(6.25).unwrap();

        plan.merge_chapter_cuts();
        assert_eq!(plan.accepted_cuts().as_slice(), [4.0, 6.0]);
        assert_eq!(plan.chapter_split_count(), 0);
        // The marks are still there to be separated at again.
        assert_eq!(plan.segments().len(), 5);
        assert!(!plan.is_as_detected());
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
