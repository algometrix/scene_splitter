//! Whether each scene of a split is written or dropped.
//!
//! A person drops the scenes they do not want as files, such as adverts or
//! credits. The scenes that are kept are numbered from one without gaps, so
//! the files of a split with dropped scenes are named like those of any
//! other split.

use std::fmt;

use crate::scenes::CutTimes;
use crate::timecode::TimeSpan;

/// What a split does with one scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SceneFate {
    /// Written to a file of its own.
    #[default]
    Keep,
    /// Left out. Its part of the video is in no scene file.
    Drop,
}

/// Why a set of scenes cannot be split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NothingToWrite {
    /// Every scene is dropped, so no file would be written at all.
    EverySceneDropped,
    /// The fates are not those of these cuts' scenes.
    WrongNumberOfFates { scenes: usize, fates: usize },
}

impl fmt::Display for NothingToWrite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EverySceneDropped => {
                write!(formatter, "At least one scene has to be written.")
            }
            Self::WrongNumberOfFates { scenes, fates } => write!(
                formatter,
                "there are {scenes} scenes but {fates} choices about them"
            ),
        }
    }
}

/// The scenes of one video as a split takes them: where to cut, and which
/// of the scenes between the cuts to write. At least one scene is written.
#[derive(Debug, Clone, PartialEq)]
pub struct ScenesToWrite {
    cuts: CutTimes,
    /// One per scene, in scene order.
    fates: Vec<SceneFate>,
}

impl ScenesToWrite {
    pub fn new(cuts: CutTimes, fates: Vec<SceneFate>) -> Result<Self, NothingToWrite> {
        let scenes = cuts.len() + 1;
        if fates.len() != scenes {
            return Err(NothingToWrite::WrongNumberOfFates {
                scenes,
                fates: fates.len(),
            });
        }
        if !fates.contains(&SceneFate::Keep) {
            return Err(NothingToWrite::EverySceneDropped);
        }
        Ok(Self { cuts, fates })
    }

    /// Every scene between the cuts is written, which is what a split does
    /// unless a person dropped some.
    pub fn every_scene(cuts: CutTimes) -> Self {
        let fates = vec![SceneFate::Keep; cuts.len() + 1];
        Self { cuts, fates }
    }

    pub fn cuts(&self) -> &CutTimes {
        &self.cuts
    }

    /// What happens to each scene, in scene order.
    pub fn fates(&self) -> &[SceneFate] {
        &self.fates
    }

    pub fn dropped_count(&self) -> usize {
        self.fates
            .iter()
            .filter(|&&fate| fate == SceneFate::Drop)
            .count()
    }

    pub fn has_dropped_scenes(&self) -> bool {
        self.dropped_count() > 0
    }

    /// The scenes that are written, in order. The nth of them goes into
    /// scene file n.
    pub fn kept_scenes(&self) -> Vec<TimeSpan> {
        self.cuts
            .scenes()
            .into_iter()
            .zip(&self.fates)
            .filter(|(_, &fate)| fate == SceneFate::Keep)
            .map(|(scene, _)| scene)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHOLE_VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 30.0,
    };

    fn two_cuts() -> CutTimes {
        CutTimes::inside(WHOLE_VIDEO, vec![10.0, 20.0]).unwrap()
    }

    fn span(start: f64, end: f64) -> TimeSpan {
        TimeSpan { start, end }
    }

    #[test]
    fn a_split_writes_every_scene_unless_some_are_dropped() {
        let scenes = ScenesToWrite::every_scene(two_cuts());
        assert_eq!(scenes.fates().len(), 3);
        assert_eq!(scenes.dropped_count(), 0);
        assert!(!scenes.has_dropped_scenes());
        assert_eq!(scenes.kept_scenes(), two_cuts().scenes());
    }

    #[test]
    fn the_kept_scenes_are_those_not_dropped_in_their_order() {
        let fates = vec![SceneFate::Keep, SceneFate::Drop, SceneFate::Keep];
        let scenes = ScenesToWrite::new(two_cuts(), fates).unwrap();
        assert_eq!(scenes.dropped_count(), 1);
        assert!(scenes.has_dropped_scenes());
        assert_eq!(
            scenes.kept_scenes(),
            vec![span(0.0, 10.0), span(20.0, 30.0)]
        );
    }

    #[test]
    fn dropping_every_scene_is_refused() {
        assert_eq!(
            ScenesToWrite::new(two_cuts(), vec![SceneFate::Drop; 3]),
            Err(NothingToWrite::EverySceneDropped)
        );
    }

    #[test]
    fn fates_for_another_number_of_scenes_are_refused() {
        assert_eq!(
            ScenesToWrite::new(two_cuts(), vec![SceneFate::Keep; 2]),
            Err(NothingToWrite::WrongNumberOfFates {
                scenes: 3,
                fates: 2
            })
        );
    }

    #[test]
    fn a_scene_is_kept_unless_said_otherwise() {
        assert_eq!(SceneFate::default(), SceneFate::Keep);
    }
}
