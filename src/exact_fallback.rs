//! What becomes of a split that was to land on exact pictures when the
//! video cannot be cut that way.
//!
//! Such a video is cut the normal way instead: copied, at the points where
//! it can be cut that are nearest to the splits that were chosen. This
//! module says why, moves the splits, and words what the person is told. It
//! runs nothing and reads no files.

use std::fmt;

use crate::exact_cut::PlanError;
use crate::exact_split::{ExactCutError, Unsupported};
use crate::scenes::{self, CutTimes};
use crate::timecode::{format_timestamp, TimeSpan};

/// A split this close to a point where the video can be cut is on it. Times
/// kept as fractions of a second rarely match to the last digit.
const ON_THE_CUT_POINT_SECONDS: f64 = 0.0005;

/// What follows every reason a video is not cut on the exact picture, before
/// the split and after it.
const WILL_BE_CUT_AT_THE_NEAREST_POINTS: &str =
    "It will be cut at the nearest points where it can be cut instead.";
const WAS_CUT_AT_THE_NEAREST_POINTS: &str =
    "It was cut at the nearest points where it can be cut instead.";

/// Why a video is not cut on the exact pictures that were chosen.
#[derive(Debug, Clone, PartialEq)]
pub enum WhyNotExact {
    /// The video is of a kind exact cuts are not made for. Known before
    /// anything is written.
    KindOfVideo(Unsupported),
    /// The ffmpeg in use was built without the encoder. Known before
    /// anything is written.
    EncoderMissing,
    /// The ffmpeg in use is from before it could set the decode times of
    /// the pictures it encodes. Known before anything is written.
    FfmpegTooOld,
    /// The pictures of the video, or where the splits are among them, leave
    /// no way to plan the cut. Known before anything is written.
    Pictures(PlanError),
    /// The scenes could not be written on the exact pictures. Whatever was
    /// written was thrown away.
    CouldNotBeWritten { details: String },
    /// The scenes were written on the exact pictures, did not pass their
    /// check, and were thrown away.
    DidNotPassTheCheck { details: String },
}

impl WhyNotExact {
    /// The reason as the end of a sentence that begins "This video could not
    /// be cut on the exact picture because".
    fn reason_in_plain_words(&self) -> String {
        match self {
            Self::KindOfVideo(what) => kind_of_video_in_plain_words(what),
            Self::EncoderMissing => {
                "this copy of ffmpeg lacks the encoder that exact cuts need".to_owned()
            }
            Self::FfmpegTooOld => "this copy of ffmpeg is too old for exact cuts".to_owned(),
            Self::Pictures(problem) => pictures_in_plain_words(problem),
            Self::CouldNotBeWritten { .. } => {
                "writing the scenes that way failed, and what was written was thrown away"
                    .to_owned()
            }
            Self::DidNotPassTheCheck { .. } => {
                "the scenes that were cut that way did not pass the check and were thrown away"
                    .to_owned()
            }
        }
    }

    /// What the person is told before the split, where the reason is known
    /// by then.
    pub fn before_the_split_in_plain_words(&self) -> String {
        format!(
            "This video cannot be cut on the exact picture because {}. \
             {WILL_BE_CUT_AT_THE_NEAREST_POINTS}",
            self.reason_in_plain_words()
        )
    }

    /// What the person is told in the result of the split.
    pub fn after_the_split_in_plain_words(&self) -> String {
        format!(
            "{} {WAS_CUT_AT_THE_NEAREST_POINTS}",
            self.could_not_be_cut_in_plain_words()
        )
    }

    /// That the video was not cut on the exact picture and why, without
    /// what was done instead.
    fn could_not_be_cut_in_plain_words(&self) -> String {
        format!(
            "This video could not be cut on the exact picture because {}.",
            self.reason_in_plain_words()
        )
    }

    /// The technical reason, for the details a person can copy.
    pub fn details(&self) -> String {
        match self {
            Self::KindOfVideo(what) => what.to_string(),
            Self::EncoderMissing => ExactCutError::EncoderMissing.to_string(),
            Self::FfmpegTooOld => ExactCutError::TimeFilterMissing.to_string(),
            Self::Pictures(problem) => problem.to_string(),
            Self::CouldNotBeWritten { details } | Self::DidNotPassTheCheck { details } => {
                details.clone()
            }
        }
    }
}

fn kind_of_video_in_plain_words(what: &Unsupported) -> String {
    match what {
        Unsupported::Container { extension } if extension.is_empty() => {
            "its name does not say what kind of file it is".to_owned()
        }
        Unsupported::Container { extension } => {
            format!("exact cuts do not work for .{extension} files")
        }
        Unsupported::Codec { name } => format!(
            "its pictures are stored as {name}, and exact cuts only work for pictures stored as H.264"
        ),
        Unsupported::PixelFormat { name } => {
            format!("its pictures are stored with colours ({name}) that exact cuts do not work for")
        }
        Unsupported::Profile { name } => format!(
            "its pictures are stored in a kind of H.264 ({name}) that exact cuts do not work for"
        ),
        Unsupported::Interlaced { .. } => {
            "each of its pictures is stored as two half pictures (it is interlaced)".to_owned()
        }
        Unsupported::Clock { .. } => "the times of its pictures cannot be kept exactly".to_owned(),
        Unsupported::PicturesNeverShown { .. } => {
            "the file holds pictures that are never shown".to_owned()
        }
    }
}

fn pictures_in_plain_words(problem: &PlanError) -> String {
    match problem {
        PlanError::NoPictures => "it has no pictures".to_owned(),
        PlanError::PicturesShareATime { at } => format!(
            "two of its pictures are shown at the same moment, {}",
            format_timestamp(*at)
        ),
        PlanError::SceneWithoutPictures { cut_time } => format!(
            "the split at {} would leave a scene without a single picture",
            format_timestamp(*cut_time)
        ),
        PlanError::DecodeOrderUnknown { .. } => {
            "the file does not say in which order its pictures are stored".to_owned()
        }
    }
}

/// What a look at a video's file says about cutting it on exact pictures,
/// for telling the person before the split.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ExactCutLook {
    /// Nobody has looked yet.
    #[default]
    NotLooked,
    /// A look is under way.
    Looking,
    /// Nothing that shows at a glance speaks against an exact cut. It can
    /// still turn out not to work once every picture is listed, or fail its
    /// check, which only the split finds out.
    CanBeTried,
    CannotBeCut(WhyNotExact),
}

impl ExactCutLook {
    /// What to tell the person before the split, when the look found that
    /// the video will be cut another way.
    pub fn notice_before_the_split(&self) -> Option<String> {
        match self {
            Self::CannotBeCut(why_not_exact) => {
                Some(why_not_exact.before_the_split_in_plain_words())
            }
            Self::NotLooked | Self::Looking | Self::CanBeTried => None,
        }
    }
}

/// One split of a video that was cut at the nearest points instead: where
/// it was chosen, and where the video was cut.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovedSplit {
    pub asked_for: f64,
    pub cut_at: f64,
}

impl MovedSplit {
    /// How far the cut is from where it was asked for, in seconds.
    pub fn seconds_moved(&self) -> f64 {
        (self.cut_at - self.asked_for).abs()
    }

    pub fn has_moved(&self) -> bool {
        self.seconds_moved() > ON_THE_CUT_POINT_SECONDS
    }

    fn direction(&self) -> &'static str {
        if self.cut_at < self.asked_for {
            "earlier"
        } else {
            "later"
        }
    }

    /// "from 00:00:02.920 to 00:00:02.000, 0.920 seconds earlier".
    fn move_in_words(&self) -> String {
        format!(
            "from {} to {}, {:.3} seconds {}",
            format_timestamp(self.asked_for),
            format_timestamp(self.cut_at),
            self.seconds_moved(),
            self.direction()
        )
    }
}

/// A video that was to be cut on exact pictures and was cut at the nearest
/// points where it can be cut instead: why, and what that did to each split.
#[derive(Debug, Clone, PartialEq)]
pub struct ExactCutFallback {
    pub why: WhyNotExact,
    /// Every split of the video, in order, moved or not.
    pub splits: Vec<MovedSplit>,
}

impl ExactCutFallback {
    /// The split that is farthest from where it was asked for, if any moved.
    pub fn farthest_move(&self) -> Option<MovedSplit> {
        self.splits
            .iter()
            .copied()
            .filter(MovedSplit::has_moved)
            .max_by(|a, b| a.seconds_moved().total_cmp(&b.seconds_moved()))
    }

    fn moved_count(&self) -> usize {
        self.splits.iter().filter(|split| split.has_moved()).count()
    }

    /// What happened and what it did to the splits, for a person who chose
    /// exact times and did not get them.
    pub fn in_plain_words(&self) -> String {
        format!(
            "{} {}",
            self.why.after_the_split_in_plain_words(),
            self.moves_in_plain_words()
        )
    }

    fn moves_in_plain_words(&self) -> String {
        let Some(farthest) = self.farthest_move() else {
            return "Every split was at such a point already, so none of them moved.".to_owned();
        };
        match (self.moved_count(), self.splits.len()) {
            (1, 1) => format!("The split moved {}.", farthest.move_in_words()),
            (1, split_count) => format!(
                "1 of the {split_count} splits moved, {}.",
                farthest.move_in_words()
            ),
            (moved_count, split_count) => format!(
                "{moved_count} of the {split_count} splits moved. The one that moved farthest \
                 went {}.",
                farthest.move_in_words()
            ),
        }
    }

    /// Where each split was asked for and where the video was cut, one line
    /// per split, for a run from the command line.
    pub fn each_split_in_words(&self) -> String {
        let lines: Vec<String> = self
            .splits
            .iter()
            .enumerate()
            .map(|(position, split)| {
                let number = position + 1;
                if split.has_moved() {
                    format!("Split {number}: moved {}.", split.move_in_words())
                } else {
                    format!(
                        "Split {number}: at {}, where it was asked for.",
                        format_timestamp(split.cut_at)
                    )
                }
            })
            .collect();
        lines.join("\n")
    }
}

/// Why the splits of a video cannot all go to points where it can be cut.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CannotMoveSplits {
    /// The video has no point inside it where it can be cut.
    NoCutPoint { split: f64 },
    /// Two splits have the same nearest point, so the scene between them
    /// would be left with nothing.
    SplitsMeet {
        first: f64,
        second: f64,
        cut_point: f64,
    },
}

impl CannotMoveSplits {
    /// What stands in the way and what to do about it.
    pub fn in_plain_words(&self) -> String {
        match self {
            Self::NoCutPoint { split } => format!(
                "The video has no point near the split at {} where it can be cut.",
                format_timestamp(*split)
            ),
            Self::SplitsMeet { first, second, .. } => format!(
                "The splits at {} and {} have the same nearest point where the video can be \
                 cut. Merge the scenes on both sides of one of them and split again.",
                format_timestamp(*first),
                format_timestamp(*second)
            ),
        }
    }
}

impl fmt::Display for CannotMoveSplits {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCutPoint { split } => write!(
                formatter,
                "no point where the video can be cut is near the split at {split} seconds"
            ),
            Self::SplitsMeet {
                first,
                second,
                cut_point,
            } => write!(
                formatter,
                "the splits at {first} and {second} seconds would both be cut at {cut_point} seconds"
            ),
        }
    }
}

/// A video that could be cut neither on the exact pictures nor at the
/// nearest points where it can be cut. Nothing was written, and the
/// original was not changed.
#[derive(Debug, Clone, PartialEq)]
pub struct NoWayToCut {
    pub why_not_exact: WhyNotExact,
    pub obstacle: CannotMoveSplits,
}

const ORIGINAL_UNTOUCHED: &str = "Nothing was written, and the original was not changed.";

impl NoWayToCut {
    /// What happened, what was kept and what to try, for a person who knows
    /// nothing of video. `Display` gives the technical reasons.
    pub fn in_plain_words(&self) -> String {
        format!(
            "{} It could not be cut at the nearest points where it can be cut either. \
             {ORIGINAL_UNTOUCHED} {}",
            self.why_not_exact.could_not_be_cut_in_plain_words(),
            self.obstacle.in_plain_words()
        )
    }
}

impl fmt::Display for NoWayToCut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the video was not cut on the exact pictures ({}), and not at the nearest points \
             where it can be cut either: {}",
            self.why_not_exact.details(),
            self.obstacle
        )
    }
}

impl std::error::Error for NoWayToCut {}

/// The splits of `cuts`, each moved to the point nearest to it where the
/// video can be cut. `cut_points` are those points, in ascending order.
///
/// The video keeps as many scenes as were chosen, in the same order, or the
/// choices made about each scene would no longer fit. So two splits that
/// would meet on one point are refused, and so is a split with no point to
/// go to.
pub fn splits_at_the_nearest_cut_points(
    cuts: &CutTimes,
    cut_points: &[f64],
) -> Result<(CutTimes, Vec<MovedSplit>), CannotMoveSplits> {
    let whole_video = cuts.whole_video();
    let mut moved: Vec<MovedSplit> = Vec::with_capacity(cuts.len());
    for &asked_for in cuts.as_slice() {
        let cut_at = nearest_cut_point(asked_for, cut_points, whole_video)
            .ok_or(CannotMoveSplits::NoCutPoint { split: asked_for })?;
        if let Some(before) = moved.last().filter(|before| before.cut_at >= cut_at) {
            return Err(CannotMoveSplits::SplitsMeet {
                first: before.asked_for,
                second: asked_for,
                cut_point: cut_at,
            });
        }
        moved.push(MovedSplit { asked_for, cut_at });
    }
    let cut_times = moved.iter().map(|split| split.cut_at).collect();
    let cuts_at_cut_points = CutTimes::inside(whole_video, cut_times)
        // Every time was just checked to be inside the video.
        .map_err(|outside| CannotMoveSplits::NoCutPoint {
            split: outside.time,
        })?;
    Ok((cuts_at_cut_points, moved))
}

/// The point nearest to `time` where the video can be cut, if it is inside
/// the video. A cut on an edge of the video would leave an empty scene.
fn nearest_cut_point(time: f64, cut_points: &[f64], whole_video: TimeSpan) -> Option<f64> {
    let moment = TimeSpan {
        start: time,
        end: time,
    };
    scenes::nearest_keyframe(&moment, cut_points)
        .filter(|&cut_point| whole_video.has_strictly_inside(cut_point))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHOLE_VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 9.0,
    };
    /// A point where the video can be cut every two seconds.
    const CUT_POINTS: [f64; 5] = [0.0, 2.0, 4.0, 6.0, 8.0];

    fn cuts(times: &[f64]) -> CutTimes {
        CutTimes::inside(WHOLE_VIDEO, times.to_vec()).unwrap()
    }

    fn not_h264() -> WhyNotExact {
        WhyNotExact::KindOfVideo(Unsupported::Codec {
            name: "mpeg4".to_owned(),
        })
    }

    fn fallback(splits: &[(f64, f64)]) -> ExactCutFallback {
        ExactCutFallback {
            why: not_h264(),
            splits: splits
                .iter()
                .map(|&(asked_for, cut_at)| MovedSplit { asked_for, cut_at })
                .collect(),
        }
    }

    #[test]
    fn each_split_goes_to_the_nearest_point_where_the_video_can_be_cut() {
        let (moved_cuts, moved) =
            splits_at_the_nearest_cut_points(&cuts(&[2.92, 5.1]), &CUT_POINTS).unwrap();

        assert_eq!(moved_cuts.as_slice(), &[2.0, 6.0]);
        assert_eq!(
            moved,
            vec![
                MovedSplit {
                    asked_for: 2.92,
                    cut_at: 2.0
                },
                MovedSplit {
                    asked_for: 5.1,
                    cut_at: 6.0
                },
            ]
        );
        assert_eq!(moved_cuts.whole_video(), WHOLE_VIDEO);
    }

    #[test]
    fn a_split_on_such_a_point_stays_and_has_not_moved() {
        let (moved_cuts, moved) =
            splits_at_the_nearest_cut_points(&cuts(&[4.0]), &CUT_POINTS).unwrap();
        assert_eq!(moved_cuts.as_slice(), &[4.0]);
        assert!(!moved[0].has_moved());
    }

    #[test]
    fn two_splits_with_the_same_nearest_point_are_refused() {
        assert_eq!(
            splits_at_the_nearest_cut_points(&cuts(&[3.6, 4.4]), &CUT_POINTS),
            Err(CannotMoveSplits::SplitsMeet {
                first: 3.6,
                second: 4.4,
                cut_point: 4.0,
            })
        );
    }

    #[test]
    fn a_split_whose_nearest_point_is_the_start_of_the_video_is_refused() {
        // The video can only be cut where it begins, which is no cut.
        assert_eq!(
            splits_at_the_nearest_cut_points(&cuts(&[0.4]), &[0.0]),
            Err(CannotMoveSplits::NoCutPoint { split: 0.4 })
        );
        assert_eq!(
            splits_at_the_nearest_cut_points(&cuts(&[0.4]), &[]),
            Err(CannotMoveSplits::NoCutPoint { split: 0.4 })
        );
    }

    #[test]
    fn a_video_without_splits_has_none_to_move() {
        let (moved_cuts, moved) =
            splits_at_the_nearest_cut_points(&cuts(&[]), &CUT_POINTS).unwrap();
        assert!(moved_cuts.is_empty());
        assert!(moved.is_empty());
    }

    #[test]
    fn the_result_says_why_and_how_far_the_one_split_moved() {
        assert_eq!(
            fallback(&[(2.92, 2.0)]).in_plain_words(),
            "This video could not be cut on the exact picture because its pictures are stored \
             as mpeg4, and exact cuts only work for pictures stored as H.264. It was cut at the \
             nearest points where it can be cut instead. The split moved from 00:00:02.920 to \
             00:00:02.000, 0.920 seconds earlier."
        );
    }

    #[test]
    fn of_several_moved_splits_the_farthest_is_named() {
        let several = fallback(&[(2.92, 2.0), (4.0, 4.0), (5.1, 6.0), (7.5, 8.0)]);
        assert_eq!(
            several.farthest_move(),
            Some(MovedSplit {
                asked_for: 2.92,
                cut_at: 2.0
            })
        );
        assert!(
            several.in_plain_words().ends_with(
                "3 of the 4 splits moved. The one that moved farthest went from 00:00:02.920 \
                 to 00:00:02.000, 0.920 seconds earlier."
            ),
            "{}",
            several.in_plain_words()
        );

        let one_of_two = fallback(&[(4.0, 4.0), (5.1, 6.0)]);
        assert!(
            one_of_two.in_plain_words().ends_with(
                "1 of the 2 splits moved, from 00:00:05.100 to 00:00:06.000, 0.900 seconds later."
            ),
            "{}",
            one_of_two.in_plain_words()
        );
    }

    #[test]
    fn splits_that_were_on_such_points_already_are_said_not_to_have_moved() {
        let none_moved = fallback(&[(4.0, 4.0)]);
        assert_eq!(none_moved.farthest_move(), None);
        assert!(none_moved
            .in_plain_words()
            .ends_with("Every split was at such a point already, so none of them moved."));
    }

    #[test]
    fn the_command_line_gets_a_line_for_every_split() {
        assert_eq!(
            fallback(&[(2.92, 2.0), (4.0, 4.0)]).each_split_in_words(),
            "Split 1: moved from 00:00:02.920 to 00:00:02.000, 0.920 seconds earlier.\n\
             Split 2: at 00:00:04.000, where it was asked for."
        );
    }

    #[test]
    fn before_the_split_the_same_reason_says_what_will_happen() {
        assert_eq!(
            WhyNotExact::EncoderMissing.before_the_split_in_plain_words(),
            "This video cannot be cut on the exact picture because this copy of ffmpeg lacks \
             the encoder that exact cuts need. It will be cut at the nearest points where it \
             can be cut instead."
        );
    }

    #[test]
    fn only_a_look_that_found_a_reason_has_something_to_say_before_the_split() {
        for nothing_to_say in [
            ExactCutLook::NotLooked,
            ExactCutLook::Looking,
            ExactCutLook::CanBeTried,
        ] {
            assert_eq!(nothing_to_say.notice_before_the_split(), None);
        }
        assert_eq!(
            ExactCutLook::CannotBeCut(not_h264()).notice_before_the_split(),
            Some(
                "This video cannot be cut on the exact picture because its pictures are stored \
                 as mpeg4, and exact cuts only work for pictures stored as H.264. It will be \
                 cut at the nearest points where it can be cut instead."
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_failed_try_is_named_as_one_with_its_reason_as_details() {
        let failed = WhyNotExact::DidNotPassTheCheck {
            details: "scene 2 should have 152 pictures but decodes to 151".to_owned(),
        };
        assert_eq!(
            failed.after_the_split_in_plain_words(),
            "This video could not be cut on the exact picture because the scenes that were cut \
             that way did not pass the check and were thrown away. It was cut at the nearest \
             points where it can be cut instead."
        );
        assert_eq!(
            failed.details(),
            "scene 2 should have 152 pictures but decodes to 151"
        );
    }

    #[test]
    fn every_reason_is_in_words_without_the_word_keyframe() {
        let reasons = [
            not_h264(),
            WhyNotExact::KindOfVideo(Unsupported::Container {
                extension: "avi".to_owned(),
            }),
            WhyNotExact::KindOfVideo(Unsupported::Container {
                extension: String::new(),
            }),
            WhyNotExact::KindOfVideo(Unsupported::PixelFormat {
                name: "yuv420p10le".to_owned(),
            }),
            WhyNotExact::KindOfVideo(Unsupported::Profile {
                name: "High 4:4:4 Predictive".to_owned(),
            }),
            WhyNotExact::KindOfVideo(Unsupported::Interlaced {
                field_order: "tt".to_owned(),
            }),
            WhyNotExact::KindOfVideo(Unsupported::Clock {
                time_base: "1001/30000".to_owned(),
            }),
            WhyNotExact::KindOfVideo(Unsupported::PicturesNeverShown { count: 2 }),
            WhyNotExact::EncoderMissing,
            WhyNotExact::FfmpegTooOld,
            WhyNotExact::Pictures(PlanError::NoPictures),
            WhyNotExact::Pictures(PlanError::PicturesShareATime { at: 1.0 }),
            WhyNotExact::Pictures(PlanError::SceneWithoutPictures { cut_time: 2.92 }),
            WhyNotExact::Pictures(PlanError::DecodeOrderUnknown { at: 4.0 }),
            WhyNotExact::CouldNotBeWritten {
                details: "ffmpeg failed".to_owned(),
            },
        ];
        for reason in reasons {
            let words = reason.after_the_split_in_plain_words();
            assert!(!words.to_lowercase().contains("keyframe"), "{words}");
            assert!(words.ends_with(WAS_CUT_AT_THE_NEAREST_POINTS), "{words}");
            assert!(!reason.details().is_empty());
        }
    }

    #[test]
    fn a_video_that_can_be_cut_neither_way_says_what_was_kept_and_what_to_try() {
        let no_way = NoWayToCut {
            why_not_exact: not_h264(),
            obstacle: CannotMoveSplits::SplitsMeet {
                first: 3.6,
                second: 4.4,
                cut_point: 4.0,
            },
        };
        assert_eq!(
            no_way.in_plain_words(),
            "This video could not be cut on the exact picture because its pictures are stored \
             as mpeg4, and exact cuts only work for pictures stored as H.264. It could not be \
             cut at the nearest points where it can be cut either. Nothing was written, and \
             the original was not changed. The splits at 00:00:03.600 and 00:00:04.400 have the \
             same nearest point where the video can be cut. Merge the scenes on both sides of \
             one of them and split again."
        );
        assert_eq!(
            no_way.to_string(),
            "the video was not cut on the exact pictures (exact cuts work on H.264 video, this \
             video is mpeg4), and not at the nearest points where it can be cut either: the \
             splits at 3.6 and 4.4 seconds would both be cut at 4 seconds"
        );
    }

    #[test]
    fn why_the_splits_cannot_move_says_what_to_do() {
        let meet = CannotMoveSplits::SplitsMeet {
            first: 3.6,
            second: 4.4,
            cut_point: 4.0,
        };
        assert_eq!(
            meet.in_plain_words(),
            "The splits at 00:00:03.600 and 00:00:04.400 have the same nearest point where the \
             video can be cut. Merge the scenes on both sides of one of them and split again."
        );
    }
}
