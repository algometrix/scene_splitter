//! Where the keys act in the review list: one scene and, within it, the
//! picture to play from.
//!
//! The selection says which scenes a merge takes in. The focus is the one
//! scene the arrow keys stand on, which moves through the list and which
//! the keys for a single scene (play, join, move its split) work on.

use crate::plan::SceneIndex;
use crate::thumbnails::PREVIEWS_PER_SEGMENT;
use crate::timecode::TimeSpan;

/// Where a picture sits among all the pictures of one scene, counted from
/// zero across its parts.
pub type PicturePosition = usize;

/// How many pictures a scene of `part_count` parts shows.
pub fn picture_count(part_count: usize) -> usize {
    part_count * PREVIEWS_PER_SEGMENT
}

/// The picture at `place_in_part` among the pictures of one part, counted
/// across the whole scene.
pub fn picture_position(part: usize, place_in_part: usize) -> PicturePosition {
    part * PREVIEWS_PER_SEGMENT + place_in_part
}

/// Which part of its scene a picture belongs to.
pub fn part_of(picture: PicturePosition) -> usize {
    picture / PREVIEWS_PER_SEGMENT
}

/// Where a picture sits among the pictures of its own part.
pub fn place_in_part(picture: PicturePosition) -> usize {
    picture % PREVIEWS_PER_SEGMENT
}

/// The scene the keys act on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReviewFocus {
    scene: SceneIndex,
    picture: Option<PicturePosition>,
}

impl ReviewFocus {
    /// A scene that was just stepped onto or clicked, with no picture chosen
    /// in it yet.
    pub fn on(scene: SceneIndex) -> Self {
        Self {
            scene,
            picture: None,
        }
    }

    pub fn scene(self) -> SceneIndex {
        self.scene
    }

    /// The picture chosen to play from. `None` plays the scene from its
    /// start.
    pub fn picture(self) -> Option<PicturePosition> {
        self.picture
    }

    pub fn with_picture(self, picture: Option<PicturePosition>) -> Self {
        Self { picture, ..self }
    }
}

/// A move of the focus from one scene to another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SceneStep {
    Previous,
    Next,
    First,
    Last,
}

impl SceneStep {
    /// The scene this step leads to in a list of `scene_count` scenes, or
    /// `None` in an empty list. A step off either end stays on the scene at
    /// that end. With nothing focused yet, a step forward starts at the top
    /// of the list and a step back at its bottom.
    pub fn from(self, focused: Option<SceneIndex>, scene_count: usize) -> Option<SceneIndex> {
        let last = scene_count.checked_sub(1)?;
        let position = match (self, focused) {
            (Self::First, _) | (Self::Next, None) => 0,
            (Self::Last, _) | (Self::Previous, None) => last,
            (Self::Previous, Some(scene)) => scene.position().saturating_sub(1),
            (Self::Next, Some(scene)) => scene.position() + 1,
        };
        Some(SceneIndex::new(position.min(last)))
    }
}

/// A move of the focus between the pictures of the focused scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PictureStep {
    Previous,
    Next,
}

impl PictureStep {
    /// The picture this step leads to in a scene of `picture_count`
    /// pictures. Before the first picture comes "no picture", which stands
    /// for the start of the scene. A step past the last picture stays on it.
    pub fn from(
        self,
        chosen: Option<PicturePosition>,
        picture_count: usize,
    ) -> Option<PicturePosition> {
        let last = picture_count.checked_sub(1)?;
        match (self, chosen) {
            (Self::Previous, None) => None,
            (Self::Previous, Some(picture)) => picture.checked_sub(1),
            (Self::Next, None) => Some(0),
            (Self::Next, Some(picture)) => Some((picture + 1).min(last)),
        }
    }
}

/// The scene that shows the video at `time`. Scenes renumber when the plan
/// changes, so the focus is carried over by the moment it was on.
pub fn scene_showing(scenes: &[TimeSpan], time: f64) -> Option<SceneIndex> {
    let last = scenes.len().checked_sub(1)?;
    let scenes_over = scenes.partition_point(|scene| scene.end <= time);
    Some(SceneIndex::new(scenes_over.min(last)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIVE_SCENES: usize = 5;

    fn scene(position: usize) -> SceneIndex {
        SceneIndex::new(position)
    }

    fn after(step: SceneStep, focused: Option<usize>) -> Option<usize> {
        step.from(focused.map(scene), FIVE_SCENES)
            .map(SceneIndex::position)
    }

    #[test]
    fn a_step_moves_one_scene_up_or_down() {
        assert_eq!(after(SceneStep::Next, Some(1)), Some(2));
        assert_eq!(after(SceneStep::Previous, Some(1)), Some(0));
    }

    #[test]
    fn a_step_off_either_end_of_the_list_stays_at_that_end() {
        assert_eq!(after(SceneStep::Previous, Some(0)), Some(0));
        assert_eq!(after(SceneStep::Next, Some(4)), Some(4));
    }

    #[test]
    fn home_and_end_go_to_the_first_and_last_scene() {
        assert_eq!(after(SceneStep::First, Some(3)), Some(0));
        assert_eq!(after(SceneStep::Last, Some(1)), Some(4));
        assert_eq!(after(SceneStep::First, None), Some(0));
        assert_eq!(after(SceneStep::Last, None), Some(4));
    }

    #[test]
    fn the_first_step_starts_at_the_end_of_the_list_it_comes_from() {
        assert_eq!(after(SceneStep::Next, None), Some(0));
        assert_eq!(after(SceneStep::Previous, None), Some(4));
    }

    #[test]
    fn a_focus_left_on_a_scene_that_is_gone_steps_back_into_the_list() {
        assert_eq!(after(SceneStep::Next, Some(9)), Some(4));
        assert_eq!(after(SceneStep::Previous, Some(9)), Some(4));
    }

    #[test]
    fn a_list_without_scenes_has_nothing_to_step_onto() {
        assert_eq!(SceneStep::Next.from(None, 0), None);
        assert_eq!(SceneStep::Last.from(Some(scene(2)), 0), None);
    }

    #[test]
    fn the_pictures_of_a_scene_are_stepped_through_from_its_start() {
        const FOUR_PICTURES: usize = 4;
        assert_eq!(PictureStep::Next.from(None, FOUR_PICTURES), Some(0));
        assert_eq!(PictureStep::Next.from(Some(0), FOUR_PICTURES), Some(1));
        assert_eq!(PictureStep::Next.from(Some(3), FOUR_PICTURES), Some(3));
        assert_eq!(PictureStep::Previous.from(Some(2), FOUR_PICTURES), Some(1));
        // Before the first picture comes the start of the scene.
        assert_eq!(PictureStep::Previous.from(Some(0), FOUR_PICTURES), None);
        assert_eq!(PictureStep::Previous.from(None, FOUR_PICTURES), None);
    }

    #[test]
    fn a_scene_without_pictures_has_none_to_choose() {
        assert_eq!(PictureStep::Next.from(None, 0), None);
    }

    #[test]
    fn a_new_focus_has_no_picture_chosen() {
        let focus = ReviewFocus::on(scene(2));
        assert_eq!(focus.scene(), scene(2));
        assert_eq!(focus.picture(), None);
        assert_eq!(focus.with_picture(Some(3)).picture(), Some(3));
        assert_eq!(focus.with_picture(Some(3)).scene(), scene(2));
    }

    #[test]
    fn pictures_are_counted_across_the_parts_of_a_scene() {
        assert_eq!(picture_count(3), 3 * PREVIEWS_PER_SEGMENT);
        for part in 0..3 {
            for place in 0..PREVIEWS_PER_SEGMENT {
                let picture = picture_position(part, place);
                assert!(picture < picture_count(3));
                assert_eq!(part_of(picture), part);
                assert_eq!(place_in_part(picture), place);
            }
        }
        assert_eq!(picture_position(1, 0), PREVIEWS_PER_SEGMENT);
    }

    fn scenes_ending_at(ends: &[f64]) -> Vec<TimeSpan> {
        let starts = [0.0].into_iter().chain(ends.iter().copied());
        starts
            .zip(ends)
            .map(|(start, &end)| TimeSpan { start, end })
            .collect()
    }

    #[test]
    fn a_moment_belongs_to_the_scene_that_shows_it() {
        let scenes = scenes_ending_at(&[20.0, 60.0, 100.0]);
        assert_eq!(scene_showing(&scenes, 0.0), Some(scene(0)));
        assert_eq!(scene_showing(&scenes, 19.9), Some(scene(0)));
        // A scene starts at the moment the one before it ends.
        assert_eq!(scene_showing(&scenes, 20.0), Some(scene(1)));
        assert_eq!(scene_showing(&scenes, 40.0), Some(scene(1)));
        assert_eq!(scene_showing(&scenes, 60.0), Some(scene(2)));
    }

    #[test]
    fn a_moment_at_or_past_the_end_belongs_to_the_last_scene() {
        let scenes = scenes_ending_at(&[20.0, 60.0, 100.0]);
        assert_eq!(scene_showing(&scenes, 100.0), Some(scene(2)));
        assert_eq!(scene_showing(&scenes, 500.0), Some(scene(2)));
        assert_eq!(scene_showing(&[], 5.0), None);
    }
}
