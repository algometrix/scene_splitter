//! Limits on how long a scene may be, and what they do to the splits a scan
//! suggests.
//!
//! A film that fades its title cards in and out has a black screen every few
//! seconds, and a split at each one gives scenes nobody wants. With a
//! shortest scene set, splits are left out until no scene is shorter. With a
//! longest scene set, a scene that is longer is cut into equal parts. Both
//! are off unless the person sets them, and each works without the other.
//!
//! The limits shape the splits a review or a split starts from. They are
//! applied after the scan, to what it found, so changing them needs no new
//! scan. What a person merges or splits by hand in a review is theirs: the
//! limits are not applied to it again.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::scenes;
use crate::timecode::{format_length, parse_timestamp, TimeSpan};

/// How long a scene is, in seconds: a number above zero.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct SceneLength(f64);

impl SceneLength {
    pub fn from_seconds(seconds: f64) -> Result<Self, LimitsRefused> {
        if !seconds.is_finite() || seconds <= 0.0 {
            return Err(LimitsRefused::NotALength);
        }
        Ok(Self(seconds))
    }

    pub fn seconds(self) -> f64 {
        self.0
    }
}

impl fmt::Display for SceneLength {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&format_length(self.0))
    }
}

/// Why two lengths cannot be the limits of a scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LimitsRefused {
    /// Zero, less than zero, or not a number.
    NotALength,
    /// No scene could be within both limits.
    LongestBelowShortest {
        shortest: SceneLength,
        longest: SceneLength,
    },
}

impl fmt::Display for LimitsRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotALength => write!(formatter, "a scene length must be longer than zero"),
            Self::LongestBelowShortest { shortest, longest } => write!(
                formatter,
                "the longest scene ({longest}) must not be shorter than the shortest scene \
                 ({shortest})"
            ),
        }
    }
}

impl std::error::Error for LimitsRefused {}

/// The shortest and the longest a scene may be. Either can be left off, and
/// a longest scene is never shorter than the shortest one, so there is
/// always a length that both allow.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "StoredLimits", into = "StoredLimits")]
pub struct SceneLengthLimits {
    shortest: Option<SceneLength>,
    longest: Option<SceneLength>,
}

impl SceneLengthLimits {
    /// Scenes may be as short and as long as they come.
    pub const NONE: Self = Self {
        shortest: None,
        longest: None,
    };

    pub fn new(
        shortest: Option<SceneLength>,
        longest: Option<SceneLength>,
    ) -> Result<Self, LimitsRefused> {
        if let (Some(shortest), Some(longest)) = (shortest, longest) {
            if longest < shortest {
                return Err(LimitsRefused::LongestBelowShortest { shortest, longest });
            }
        }
        Ok(Self { shortest, longest })
    }

    /// `None` while scenes may be as short as they come.
    pub fn shortest(self) -> Option<SceneLength> {
        self.shortest
    }

    /// `None` while scenes may be as long as they come.
    pub fn longest(self) -> Option<SceneLength> {
        self.longest
    }

    pub fn with_shortest(self, shortest: Option<SceneLength>) -> Result<Self, LimitsRefused> {
        Self::new(shortest, self.longest)
    }

    pub fn with_longest(self, longest: Option<SceneLength>) -> Result<Self, LimitsRefused> {
        Self::new(self.shortest, longest)
    }

    /// The limits with what one run of the command line says in their place.
    pub fn overridden_by(
        self,
        shortest: Option<SceneLengthOverride>,
        longest: Option<SceneLengthOverride>,
    ) -> Result<Self, LimitsRefused> {
        Self::new(
            shortest.map_or(self.shortest, SceneLengthOverride::limit),
            longest.map_or(self.longest, SceneLengthOverride::limit),
        )
    }
}

/// The limits as the settings file holds them. A file from before there
/// were limits has neither entry, which means no limit.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredLimits {
    shortest_seconds: Option<f64>,
    longest_seconds: Option<f64>,
}

impl TryFrom<StoredLimits> for SceneLengthLimits {
    type Error = LimitsRefused;

    fn try_from(stored: StoredLimits) -> Result<Self, LimitsRefused> {
        let length = |seconds: Option<f64>| seconds.map(SceneLength::from_seconds).transpose();
        Self::new(
            length(stored.shortest_seconds)?,
            length(stored.longest_seconds)?,
        )
    }
}

impl From<SceneLengthLimits> for StoredLimits {
    fn from(limits: SceneLengthLimits) -> Self {
        Self {
            shortest_seconds: limits.shortest.map(SceneLength::seconds),
            longest_seconds: limits.longest.map(SceneLength::seconds),
        }
    }
}

/// What the command line says about one limit for one run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SceneLengthOverride {
    /// Ignore the saved limit.
    NoLimit,
    Limit(SceneLength),
}

/// What is typed on the command line for "no limit in this run".
pub const NO_LIMIT_ARGUMENT: &str = "off";

impl SceneLengthOverride {
    fn limit(self) -> Option<SceneLength> {
        match self {
            Self::NoLimit => None,
            Self::Limit(length) => Some(length),
        }
    }
}

/// What was typed for a scene length is neither a time nor "off".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotASceneLength;

impl fmt::Display for NotASceneLength {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "give a length above zero like 90, 1:30 or 1:02:03, or {NO_LIMIT_ARGUMENT} for no limit"
        )
    }
}

impl std::error::Error for NotASceneLength {}

impl FromStr for SceneLengthOverride {
    type Err = NotASceneLength;

    fn from_str(typed: &str) -> Result<Self, NotASceneLength> {
        if typed.trim().eq_ignore_ascii_case(NO_LIMIT_ARGUMENT) {
            return Ok(Self::NoLimit);
        }
        parse_timestamp(typed)
            .and_then(|seconds| SceneLength::from_seconds(seconds).ok())
            .map(Self::Limit)
            .ok_or(NotASceneLength)
    }
}

/// Why a split is suggested where it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitReason {
    /// A black screen that lasted this many seconds.
    BlackScreen { seconds: f64 },
    /// A chapter mark of the file.
    ChapterMark,
    /// A sudden change of picture, this strong (0 to 255).
    SuddenChange { strength: u8 },
}

/// How much a kind of reason is trusted, from the least to the most.
///
/// A black screen is what this app splits at, so it comes first. A chapter
/// mark was set by whoever made the file, which is a statement and not a
/// guess. A sudden change of picture is a guess: films also cut between
/// camera angles inside one scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Trust {
    SuddenChange,
    ChapterMark,
    BlackScreen,
}

impl SplitReason {
    fn trust(self) -> Trust {
        match self {
            Self::BlackScreen { .. } => Trust::BlackScreen,
            Self::ChapterMark => Trust::ChapterMark,
            Self::SuddenChange { .. } => Trust::SuddenChange,
        }
    }

    /// How two reasons of the same kind compare: a longer black screen and
    /// a stronger change count for more. Chapter marks are all alike.
    fn weight(self) -> f64 {
        match self {
            Self::BlackScreen { seconds } => seconds,
            Self::ChapterMark => 0.0,
            Self::SuddenChange { strength } => f64::from(strength),
        }
    }
}

/// One split a scan or the file suggests, before any limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SuggestedSplit {
    pub time: f64,
    pub reason: SplitReason,
}

/// Orders splits from the one most worth keeping to the one least worth
/// keeping. Never equal for two splits at different times, so the order
/// does not depend on how the splits were listed.
fn most_worth_keeping_first(one: &SuggestedSplit, other: &SuggestedSplit) -> Ordering {
    other
        .reason
        .trust()
        .cmp(&one.reason.trust())
        .then(other.reason.weight().total_cmp(&one.reason.weight()))
        .then(one.time.total_cmp(&other.time))
}

/// The suggested splits once the limits are applied.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LimitedSplits {
    /// The suggested splits that stay, in ascending order of time.
    pub kept: Vec<SuggestedSplit>,
    /// The suggested splits the shortest scene left out, in ascending order
    /// of time.
    pub left_out: Vec<SuggestedSplit>,
    /// The splits the longest scene added, in ascending order of time. None
    /// is at the time of a split that stays.
    pub added: Vec<f64>,
}

/// Applies the limits to the splits suggested for one video.
///
/// `suggested` holds each time once, strictly inside `whole_video`.
/// `keyframe_times` are the points where the video can be cut, in ascending
/// order.
///
/// The shortest scene is seen to first. Splits are taken in the order of
/// `most_worth_keeping_first`: those at black screens before those at
/// chapter marks before those at sudden picture changes, a longer black
/// screen before a shorter one, a stronger change before a weaker one, and
/// of two alike the earlier one. A split stays when it is at least the
/// shortest scene away from both ends of the video and from every split
/// that already stays. So a split is only left out for the sake of an end
/// of the video or of a split worth more than it, and the first and the
/// last scene are as long as any other.
///
/// Then each scene longer than the longest scene is cut into equal parts,
/// at the points nearest to them where the video can be cut. Where the two
/// limits pull apart, the shortest scene wins: no part is made shorter than
/// it, even if that leaves a scene longer than the longest.
pub fn splits_within(
    limits: SceneLengthLimits,
    suggested: &[SuggestedSplit],
    keyframe_times: &[f64],
    whole_video: TimeSpan,
) -> LimitedSplits {
    let (kept, left_out) = match limits.shortest {
        Some(shortest) => without_short_scenes(suggested, shortest, whole_video),
        None => (in_order_of_time(suggested.to_vec()), Vec::new()),
    };
    let added = match limits.longest {
        Some(longest) => {
            let kept_times: Vec<f64> = kept.iter().map(|split| split.time).collect();
            scenes::scenes_between(&kept_times, whole_video)
                .into_iter()
                .filter(|scene| scene.seconds() > longest.seconds())
                .flat_map(|scene| {
                    cuts_into_equal_parts(scene, longest, limits.shortest, keyframe_times)
                })
                .collect()
        }
        None => Vec::new(),
    };
    LimitedSplits {
        kept,
        left_out,
        added,
    }
}

fn in_order_of_time(mut splits: Vec<SuggestedSplit>) -> Vec<SuggestedSplit> {
    splits.sort_by(|one, other| one.time.total_cmp(&other.time));
    splits
}

/// The splits that stay and those that are left out, so that no scene is
/// shorter than `shortest`.
fn without_short_scenes(
    suggested: &[SuggestedSplit],
    shortest: SceneLength,
    whole_video: TimeSpan,
) -> (Vec<SuggestedSplit>, Vec<SuggestedSplit>) {
    let mut by_worth = suggested.to_vec();
    by_worth.sort_by(most_worth_keeping_first);

    let mut kept: Vec<SuggestedSplit> = Vec::new();
    let mut left_out = Vec::new();
    for split in by_worth {
        let leaves_room = |other: f64| (split.time - other).abs() >= shortest.seconds();
        let leaves_no_short_scene = leaves_room(whole_video.start)
            && leaves_room(whole_video.end)
            && kept.iter().all(|stays| leaves_room(stays.time));
        if leaves_no_short_scene {
            kept.push(split);
        } else {
            left_out.push(split);
        }
    }
    (in_order_of_time(kept), in_order_of_time(left_out))
}

/// The cuts that divide a scene longer than `longest` into equal parts.
///
/// The scene gets the fewest parts that are each within the limit. A point
/// where the video can be cut is rarely where a part should end, so a part
/// can come out a little too long. Then the scene is cut into one part more,
/// if that brings every part within the limit. Where the video can only be
/// cut far apart and even that does not help, the cuts of the fewest parts
/// stand: as close to the limit as the video allows.
fn cuts_into_equal_parts(
    scene: TimeSpan,
    longest: SceneLength,
    shortest: Option<SceneLength>,
    keyframe_times: &[f64],
) -> Vec<f64> {
    // More parts than this could not all be as long as the shortest scene.
    let most_parts = match shortest {
        Some(shortest) => (scene.seconds() / shortest.seconds()).floor() as usize,
        None => usize::MAX,
    };
    let parts_within_the_limit = (scene.seconds() / longest.seconds()).ceil() as usize;
    let fewest_parts = parts_within_the_limit.min(most_parts);
    if fewest_parts < 2 {
        return Vec::new();
    }
    let shortest_part = shortest.map_or(0.0, SceneLength::seconds);

    let cuts = cuts_at_equal_parts(scene, fewest_parts, shortest_part, keyframe_times);
    if every_part_is_within(scene, &cuts, longest) || fewest_parts == most_parts {
        return cuts;
    }
    let finer_cuts = cuts_at_equal_parts(scene, fewest_parts + 1, shortest_part, keyframe_times);
    if every_part_is_within(scene, &finer_cuts, longest) {
        return finer_cuts;
    }
    cuts
}

fn every_part_is_within(scene: TimeSpan, cuts: &[f64], longest: SceneLength) -> bool {
    scenes::scenes_between(cuts, scene)
        .iter()
        .all(|part| part.seconds() <= longest.seconds())
}

/// One cut where each of `part_count` equal parts of the scene should end,
/// moved to the nearest point where the video can be cut that leaves at
/// least `shortest_part` seconds on both sides. A part with no such point
/// gets no cut, and is then one with the part after it.
fn cuts_at_equal_parts(
    scene: TimeSpan,
    part_count: usize,
    shortest_part: f64,
    keyframe_times: &[f64],
) -> Vec<f64> {
    let part_seconds = scene.seconds() / part_count as f64;
    let mut cuts = Vec::new();
    let mut last_edge = scene.start;
    for part in 1..part_count {
        let part_end = scene.start + part as f64 * part_seconds;
        // Keyframe times ascend, so those far enough from the edge before
        // and from the end of the scene are one run of them.
        let first_far_enough = keyframe_times
            .partition_point(|&time| time <= last_edge || time - last_edge < shortest_part);
        let first_too_late = keyframe_times
            .partition_point(|&time| time < scene.end && scene.end - time >= shortest_part);
        let Some(usable_keyframes) = keyframe_times.get(first_far_enough..first_too_late) else {
            continue;
        };
        let moment = TimeSpan {
            start: part_end,
            end: part_end,
        };
        if let Some(cut) = scenes::nearest_keyframe(&moment, usable_keyframes) {
            cuts.push(cut);
            last_edge = cut;
        }
    }
    cuts
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 1000.0,
    };

    fn length(seconds: f64) -> SceneLength {
        SceneLength::from_seconds(seconds).unwrap()
    }

    fn shortest(seconds: f64) -> SceneLengthLimits {
        SceneLengthLimits::new(Some(length(seconds)), None).unwrap()
    }

    fn longest(seconds: f64) -> SceneLengthLimits {
        SceneLengthLimits::new(None, Some(length(seconds))).unwrap()
    }

    fn between(shortest: f64, longest: f64) -> SceneLengthLimits {
        SceneLengthLimits::new(Some(length(shortest)), Some(length(longest))).unwrap()
    }

    fn black(time: f64, seconds: f64) -> SuggestedSplit {
        SuggestedSplit {
            time,
            reason: SplitReason::BlackScreen { seconds },
        }
    }

    fn chapter(time: f64) -> SuggestedSplit {
        SuggestedSplit {
            time,
            reason: SplitReason::ChapterMark,
        }
    }

    fn sudden(time: f64, strength: u8) -> SuggestedSplit {
        SuggestedSplit {
            time,
            reason: SplitReason::SuddenChange { strength },
        }
    }

    fn keyframes_every(seconds: usize) -> Vec<f64> {
        (0..1000).step_by(seconds).map(|time| time as f64).collect()
    }

    fn times(splits: &[SuggestedSplit]) -> Vec<f64> {
        splits.iter().map(|split| split.time).collect()
    }

    fn limited(limits: SceneLengthLimits, suggested: &[SuggestedSplit]) -> LimitedSplits {
        splits_within(limits, suggested, &keyframes_every(10), VIDEO)
    }

    /// The scenes a split with these limits would write.
    fn scene_lengths(limits: SceneLengthLimits, suggested: &[SuggestedSplit]) -> Vec<f64> {
        let limited = limited(limits, suggested);
        let mut cuts = times(&limited.kept);
        cuts.extend(&limited.added);
        cuts.sort_by(f64::total_cmp);
        scenes::scenes_between(&cuts, VIDEO)
            .iter()
            .map(TimeSpan::seconds)
            .collect()
    }

    #[test]
    fn a_scene_length_is_above_zero() {
        assert_eq!(length(90.0).seconds(), 90.0);
        for not_a_length in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                SceneLength::from_seconds(not_a_length),
                Err(LimitsRefused::NotALength)
            );
        }
    }

    #[test]
    fn limits_are_off_unless_set_and_each_can_be_set_alone() {
        let off = SceneLengthLimits::default();
        assert_eq!(off.shortest(), None);
        assert_eq!(off.longest(), None);
        assert_eq!(shortest(60.0).longest(), None);
        assert_eq!(longest(60.0).shortest(), None);
    }

    #[test]
    fn a_longest_scene_below_the_shortest_is_refused_in_plain_words() {
        let refused = SceneLengthLimits::new(Some(length(120.0)), Some(length(60.0)));
        assert_eq!(
            refused.unwrap_err().to_string(),
            "the longest scene (1 min 00 s) must not be shorter than the shortest scene \
             (2 min 00 s)"
        );
        // A scene may be exactly as long as both limits say.
        assert!(SceneLengthLimits::new(Some(length(60.0)), Some(length(60.0))).is_ok());
        assert!(shortest(120.0).with_longest(Some(length(60.0))).is_err());
        assert!(longest(60.0).with_shortest(Some(length(120.0))).is_err());
        assert_eq!(shortest(120.0).with_shortest(None), Ok(Default::default()));
    }

    #[test]
    fn limits_are_stored_in_seconds_and_no_limit_as_nothing() {
        let stored = serde_json::to_string(&shortest(90.0)).unwrap();
        assert_eq!(
            stored,
            r#"{"shortest_seconds":90.0,"longest_seconds":null}"#
        );
        assert_eq!(
            serde_json::from_str::<SceneLengthLimits>(&stored).unwrap(),
            shortest(90.0)
        );
        assert_eq!(
            serde_json::from_str::<SceneLengthLimits>("{}").unwrap(),
            SceneLengthLimits::default()
        );
    }

    #[test]
    fn stored_limits_no_scene_could_be_within_do_not_load() {
        let upside_down = r#"{"shortest_seconds":120.0,"longest_seconds":60.0}"#;
        let problem = serde_json::from_str::<SceneLengthLimits>(upside_down).unwrap_err();
        assert!(
            problem.to_string().contains("must not be shorter than"),
            "{problem}"
        );
        let nothing = r#"{"shortest_seconds":0.0}"#;
        assert!(serde_json::from_str::<SceneLengthLimits>(nothing).is_err());
    }

    #[test]
    fn a_length_is_typed_as_a_time_and_no_limit_as_off() {
        let typed = |text: &str| text.parse::<SceneLengthOverride>();
        assert_eq!(typed("90"), Ok(SceneLengthOverride::Limit(length(90.0))));
        assert_eq!(typed("1:30"), Ok(SceneLengthOverride::Limit(length(90.0))));
        assert_eq!(
            typed("1:02:03"),
            Ok(SceneLengthOverride::Limit(length(3723.0)))
        );
        assert_eq!(typed("off"), Ok(SceneLengthOverride::NoLimit));
        assert_eq!(typed("Off"), Ok(SceneLengthOverride::NoLimit));
        for not_a_length in ["", "0", "soon", "-5", "1:2:3:4"] {
            assert_eq!(typed(not_a_length), Err(NotASceneLength), "{not_a_length}");
        }
    }

    #[test]
    fn one_run_can_set_a_limit_or_ignore_a_saved_one() {
        let saved = between(60.0, 600.0);
        assert_eq!(saved.overridden_by(None, None), Ok(saved));
        assert_eq!(
            saved.overridden_by(Some(SceneLengthOverride::NoLimit), None),
            Ok(longest(600.0))
        );
        assert_eq!(
            saved.overridden_by(None, Some(SceneLengthOverride::Limit(length(300.0)))),
            Ok(between(60.0, 300.0))
        );
        // The saved longest scene still has a say.
        assert!(saved
            .overridden_by(Some(SceneLengthOverride::Limit(length(900.0))), None)
            .is_err());
    }

    #[test]
    fn without_limits_every_suggested_split_stays() {
        let suggested = [black(400.0, 1.0), black(5.0, 1.0), sudden(402.0, 20)];
        let limited = limited(SceneLengthLimits::default(), &suggested);
        assert_eq!(times(&limited.kept), [5.0, 400.0, 402.0]);
        assert!(limited.left_out.is_empty());
        assert!(limited.added.is_empty());
    }

    #[test]
    fn of_splits_too_close_together_the_one_at_the_longest_black_screen_stays() {
        let suggested = [
            black(300.0, 0.5),
            black(310.0, 2.0),
            black(320.0, 1.0),
            black(700.0, 0.2),
        ];
        let limited = limited(shortest(60.0), &suggested);
        assert_eq!(times(&limited.kept), [310.0, 700.0]);
        assert_eq!(times(&limited.left_out), [300.0, 320.0]);
    }

    #[test]
    fn a_split_stays_when_only_weaker_splits_are_close_to_it() {
        // Taken in order of time, 300 would push out 350 and then 400 would
        // stay. The strongest are placed first, so 350 stays and both of its
        // neighbours go.
        let suggested = [black(300.0, 1.0), black(350.0, 3.0), black(400.0, 2.0)];
        assert_eq!(times(&limited(shortest(60.0), &suggested).kept), [350.0]);
    }

    #[test]
    fn as_many_splits_stay_as_the_stronger_ones_leave_room_for() {
        let suggested = [
            black(100.0, 1.0),
            black(150.0, 2.0),
            black(200.0, 3.0),
            black(250.0, 4.0),
        ];
        // 250 stays, 200 is too close to it, 150 is far enough from 250.
        assert_eq!(
            times(&limited(shortest(60.0), &suggested).kept),
            [150.0, 250.0]
        );
    }

    #[test]
    fn the_first_and_the_last_scene_are_as_long_as_any_other() {
        let suggested = [black(30.0, 9.0), black(500.0, 0.1), black(970.0, 9.0)];
        let limited = limited(shortest(60.0), &suggested);
        assert_eq!(times(&limited.kept), [500.0]);
        assert_eq!(times(&limited.left_out), [30.0, 970.0]);
    }

    #[test]
    fn splits_exactly_the_shortest_scene_apart_all_stay() {
        let suggested = [black(60.0, 1.0), black(120.0, 1.0), black(940.0, 1.0)];
        assert_eq!(
            times(&limited(shortest(60.0), &suggested).kept),
            [60.0, 120.0, 940.0]
        );
    }

    #[test]
    fn of_two_equally_long_black_screens_the_earlier_split_stays() {
        let suggested = [black(330.0, 1.0), black(300.0, 1.0)];
        assert_eq!(times(&limited(shortest(60.0), &suggested).kept), [300.0]);
    }

    #[test]
    fn a_black_screen_outranks_a_chapter_mark_which_outranks_a_sudden_change() {
        let around_a_black_screen = [sudden(290.0, 255), black(300.0, 0.1), chapter(310.0)];
        assert_eq!(
            times(&limited(shortest(60.0), &around_a_black_screen).kept),
            [300.0]
        );
        let without_one = [sudden(290.0, 255), chapter(310.0)];
        assert_eq!(times(&limited(shortest(60.0), &without_one).kept), [310.0]);
    }

    #[test]
    fn of_sudden_changes_the_strongest_stays_and_of_chapter_marks_the_earliest() {
        let changes = [sudden(300.0, 40), sudden(320.0, 90), sudden(340.0, 60)];
        assert_eq!(times(&limited(shortest(60.0), &changes).kept), [320.0]);
        let marks = [chapter(340.0), chapter(300.0), chapter(320.0)];
        assert_eq!(times(&limited(shortest(60.0), &marks).kept), [300.0]);
    }

    #[test]
    fn the_result_does_not_depend_on_the_order_the_splits_are_listed_in() {
        let mut suggested = vec![
            black(100.0, 1.0),
            chapter(130.0),
            sudden(160.0, 80),
            black(190.0, 1.0),
            black(400.0, 2.0),
            sudden(420.0, 80),
        ];
        let in_order = limited(shortest(60.0), &suggested);
        suggested.reverse();
        assert_eq!(limited(shortest(60.0), &suggested), in_order);
        assert_eq!(times(&in_order.kept), [100.0, 190.0, 400.0]);
    }

    #[test]
    fn a_video_shorter_than_two_shortest_scenes_keeps_no_split() {
        let short_video = TimeSpan {
            start: 0.0,
            end: 100.0,
        };
        let limited = splits_within(
            shortest(60.0),
            &[black(50.0, 5.0)],
            &keyframes_every(10),
            short_video,
        );
        assert!(limited.kept.is_empty());
        assert_eq!(times(&limited.left_out), [50.0]);
    }

    #[test]
    fn a_scene_longer_than_the_longest_is_cut_into_equal_parts() {
        // Fifty minutes with a limit of twenty become three scenes of about
        // seventeen, where the video can be cut every ten seconds.
        let video = TimeSpan {
            start: 0.0,
            end: 3000.0,
        };
        let keyframes: Vec<f64> = (0..300)
            .map(|keyframe| f64::from(keyframe) * 10.0)
            .collect();
        let limited = splits_within(longest(1200.0), &[], &keyframes, video);
        assert_eq!(limited.added, [1000.0, 2000.0]);
    }

    #[test]
    fn only_the_scenes_that_are_too_long_are_cut() {
        let suggested = [black(100.0, 1.0), black(700.0, 1.0)];
        let limited = limited(longest(300.0), &suggested);
        assert_eq!(times(&limited.kept), [100.0, 700.0]);
        // 100 to 700 is cut in two. 700 to 1000 is exactly as long as the
        // limit allows.
        assert_eq!(limited.added, [400.0]);
        assert_eq!(
            scene_lengths(longest(300.0), &suggested),
            [100.0, 300.0, 300.0, 300.0]
        );
    }

    #[test]
    fn a_scene_exactly_as_long_as_the_longest_is_left_whole() {
        let suggested = [black(500.0, 1.0)];
        assert!(limited(longest(500.0), &suggested).added.is_empty());
    }

    #[test]
    fn parts_end_at_the_nearest_points_where_the_video_can_be_cut() {
        let keyframes = [0.0, 310.0, 480.0, 690.0, 990.0];
        let limited = splits_within(longest(400.0), &[], &keyframes, VIDEO);
        // Three parts would end at 333 and 667.
        assert_eq!(limited.added, [310.0, 690.0]);
    }

    #[test]
    fn a_part_that_comes_out_too_long_gives_the_scene_one_part_more() {
        // Two parts of 500 are within the limit, but the video can only be
        // cut at 520, which leaves a part of 520.
        let keyframes = [0.0, 330.0, 520.0, 670.0];
        let limited = splits_within(longest(500.0), &[], &keyframes, VIDEO);
        assert_eq!(limited.added, [330.0, 670.0]);
    }

    #[test]
    fn where_the_video_can_rarely_be_cut_the_cuts_come_as_close_as_it_allows() {
        // Neither two parts nor three can stay within 500 here.
        let keyframes = [0.0, 100.0, 900.0];
        let limited = splits_within(longest(500.0), &[], &keyframes, VIDEO);
        assert_eq!(limited.added, [100.0]);

        let nowhere_to_cut = splits_within(longest(500.0), &[], &[0.0], VIDEO);
        assert!(nowhere_to_cut.added.is_empty());
        let no_keyframes = splits_within(longest(500.0), &[], &[], VIDEO);
        assert!(no_keyframes.added.is_empty());
    }

    #[test]
    fn two_parts_never_get_the_same_cut() {
        // Both of the two cuts of three parts are nearest to 500.
        let keyframes = [0.0, 500.0];
        let limited = splits_within(longest(400.0), &[], &keyframes, VIDEO);
        assert_eq!(limited.added, [500.0]);
    }

    #[test]
    fn an_added_cut_is_never_on_an_edge_of_its_scene() {
        let suggested = [black(400.0, 1.0)];
        let keyframes = [0.0, 400.0, 990.0];
        let limited = splits_within(longest(350.0), &suggested, &keyframes, VIDEO);
        // The first scene has no point to cut on. The second has one.
        assert_eq!(limited.added, [990.0]);
    }

    #[test]
    fn the_shortest_scene_wins_where_the_limits_pull_apart() {
        // 1000 seconds within 600 would be two parts of 500, which are
        // shorter than the shortest scene allows.
        let limits = between(550.0, 600.0);
        let limited = limited(limits, &[]);
        assert!(limited.added.is_empty());
    }

    #[test]
    fn an_added_cut_leaves_no_scene_shorter_than_the_shortest() {
        // Two parts would be cut at 500. The video can only be cut where
        // that leaves a part under 450, so it is not cut.
        let keyframes = [0.0, 300.0, 700.0];
        let limited = splits_within(between(450.0, 600.0), &[], &keyframes, VIDEO);
        assert!(limited.added.is_empty());

        // With a point further in, the cut goes there.
        let keyframes = [0.0, 300.0, 460.0, 700.0];
        let limited = splits_within(between(450.0, 600.0), &[], &keyframes, VIDEO);
        assert_eq!(limited.added, [460.0]);
    }

    #[test]
    fn long_scenes_are_cut_after_short_ones_are_merged() {
        // The title cards at the start go, and the scene they leave is cut.
        let suggested = [
            black(10.0, 0.5),
            black(20.0, 0.5),
            black(30.0, 0.5),
            black(600.0, 2.0),
        ];
        let limits = between(60.0, 300.0);
        let limited = limited(limits, &suggested);
        assert_eq!(times(&limited.kept), [600.0]);
        assert_eq!(times(&limited.left_out), [10.0, 20.0, 30.0]);
        assert_eq!(limited.added, [300.0, 800.0]);
        assert_eq!(
            scene_lengths(limits, &suggested),
            [300.0, 300.0, 200.0, 200.0]
        );
    }

    #[test]
    fn with_both_limits_every_scene_is_within_them_where_the_video_allows() {
        let suggested: Vec<SuggestedSplit> = [3.0, 9.0, 14.0, 250.0, 255.0, 260.0, 900.0]
            .into_iter()
            .enumerate()
            .map(|(position, time)| black(time, 0.1 + position as f64))
            .collect();
        let limits = between(30.0, 200.0);
        for scene_seconds in scene_lengths(limits, &suggested) {
            assert!((30.0..=200.0).contains(&scene_seconds), "{scene_seconds}");
        }
    }
}
