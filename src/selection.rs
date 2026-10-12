//! Which scenes are selected in the review list, with the usual click,
//! Ctrl-click and Shift-click rules.

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClickKind {
    /// A plain click: select this scene only.
    Single,
    /// Ctrl-click: add the scene to the selection, or take it out.
    Toggle,
    /// Shift-click: select everything from the last clicked scene to this one.
    ExtendRange,
}

#[derive(Debug, Default)]
pub struct SceneSelection {
    selected: BTreeSet<usize>,
    /// The scene a Shift-click extends from.
    anchor: Option<usize>,
}

impl SceneSelection {
    pub fn only(scene: usize) -> Self {
        Self {
            selected: BTreeSet::from([scene]),
            anchor: Some(scene),
        }
    }

    pub fn click(&mut self, scene: usize, kind: ClickKind) {
        match (kind, self.anchor) {
            (ClickKind::ExtendRange, Some(anchor)) => {
                self.selected = (anchor.min(scene)..=anchor.max(scene)).collect();
            }
            (ClickKind::Toggle, _) => {
                if !self.selected.remove(&scene) {
                    self.selected.insert(scene);
                }
                self.anchor = Some(scene);
            }
            (ClickKind::Single, _) | (ClickKind::ExtendRange, None) => *self = Self::only(scene),
        }
    }

    pub fn select_all(&mut self, scene_count: usize) {
        self.selected = (0..scene_count).collect();
        self.anchor = self.selected.first().copied();
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn contains(&self, scene: usize) -> bool {
        self.selected.contains(&scene)
    }

    pub fn scenes(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    /// From the first selected scene to the last, when at least two scenes
    /// are selected. Merged scenes are always adjacent, so merging a
    /// selection with gaps takes in the scenes between them too.
    pub fn span_to_merge(&self) -> Option<RangeInclusive<usize>> {
        let first = *self.selected.first()?;
        let last = *self.selected.last()?;
        (first != last).then_some(first..=last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(selection: &SceneSelection) -> Vec<usize> {
        selection.scenes().collect()
    }

    #[test]
    fn a_plain_click_selects_only_that_scene() {
        let mut selection = SceneSelection::only(1);
        selection.click(4, ClickKind::Single);
        assert_eq!(selected(&selection), vec![4]);
    }

    #[test]
    fn a_ctrl_click_adds_and_removes_scenes() {
        let mut selection = SceneSelection::only(1);
        selection.click(4, ClickKind::Toggle);
        assert_eq!(selected(&selection), vec![1, 4]);

        selection.click(1, ClickKind::Toggle);
        assert_eq!(selected(&selection), vec![4]);
    }

    #[test]
    fn a_shift_click_selects_the_range_in_either_direction() {
        let mut selection = SceneSelection::only(5);
        selection.click(2, ClickKind::ExtendRange);
        assert_eq!(selected(&selection), vec![2, 3, 4, 5]);

        // The range still extends from the scene clicked first.
        selection.click(7, ClickKind::ExtendRange);
        assert_eq!(selected(&selection), vec![5, 6, 7]);
    }

    #[test]
    fn a_shift_click_with_nothing_selected_acts_as_a_plain_click() {
        let mut selection = SceneSelection::default();
        selection.click(3, ClickKind::ExtendRange);
        assert_eq!(selected(&selection), vec![3]);
    }

    #[test]
    fn one_scene_alone_cannot_be_merged() {
        assert_eq!(SceneSelection::only(3).span_to_merge(), None);
        assert_eq!(SceneSelection::default().span_to_merge(), None);
    }

    #[test]
    fn a_selection_with_gaps_merges_everything_between() {
        let mut selection = SceneSelection::only(1);
        selection.click(4, ClickKind::Toggle);
        assert_eq!(selection.span_to_merge(), Some(1..=4));
    }

    #[test]
    fn select_all_takes_every_scene() {
        let mut selection = SceneSelection::default();
        selection.select_all(3);
        assert_eq!(selected(&selection), vec![0, 1, 2]);
    }
}
