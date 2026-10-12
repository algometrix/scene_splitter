//! Plans a split that lands on the exact picture the person chose.
//!
//! A copy of the compressed video can only start on a keyframe, because
//! every other picture is stored as a change to the pictures before it. To
//! cut anywhere else, the pictures between the cut and the nearest keyframes
//! are encoded again, and everything else is still copied. This module only
//! decides which pictures of each scene are copied and which are encoded
//! again. It runs nothing and reads no files.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::detect::{Detection, ScanKind};
use crate::hardcut;
use crate::scenes::{self, CutTimes};

/// A time less than this after a picture appears still means that picture.
/// Times typed by a person or kept as fractions of a second rarely match a
/// picture's own time to the last digit.
const SAME_PICTURE_SECONDS: f64 = 0.0005;
const MICROSECONDS_PER_SECOND: f64 = 1_000_000.0;

/// Where a split may cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CutPrecision {
    /// On the keyframe nearest to the chosen moment. Nothing is encoded
    /// again. Also what a settings file from before the choice existed means.
    #[default]
    NearestKeyframe,
    /// On the chosen picture itself. The pictures around each cut are
    /// encoded again.
    ExactFrame,
}

/// A time handed to ffmpeg, in whole microseconds, which is the finest step
/// ffmpeg reads. Sums and differences of these are exact, so the end of one
/// scene's sound and the start of the next one's are the same instant and no
/// packet of sound is written twice or lost between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FfmpegTime {
    microseconds: i64,
}

impl FfmpegTime {
    pub const ZERO: Self = Self { microseconds: 0 };

    pub const fn from_whole_seconds(seconds: i64) -> Self {
        Self {
            microseconds: seconds * 1_000_000,
        }
    }

    pub fn from_seconds(seconds: f64) -> Self {
        Self {
            microseconds: (seconds * MICROSECONDS_PER_SECOND).round() as i64,
        }
    }

    pub fn seconds(self) -> f64 {
        self.microseconds as f64 / MICROSECONDS_PER_SECOND
    }

    pub fn is_positive(self) -> bool {
        self.microseconds > 0
    }

    /// How long after `earlier` this is.
    pub fn since(self, earlier: Self) -> Self {
        Self {
            microseconds: self.microseconds - earlier.microseconds,
        }
    }

    pub fn earlier_of(self, other: Self) -> Self {
        self.min(other)
    }
}

impl fmt::Display for FfmpegTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.microseconds < 0 { "-" } else { "" };
        let magnitude = self.microseconds.unsigned_abs();
        write!(
            formatter,
            "{sign}{}.{:06}",
            magnitude / 1_000_000,
            magnitude % 1_000_000
        )
    }
}

/// One picture of the video as the file stores it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoredFrame {
    /// When the picture is shown, in seconds on the clock `ffmpeg -ss` uses.
    pub shown_at: f64,
    /// When the picture is handed to the decoder, on the same clock. Pictures
    /// are stored in this order, which differs from the order they are shown
    /// in. `None` when the file does not say.
    pub decoded_at: Option<f64>,
    /// How much earlier the picture is decoded than shown, in the steps of
    /// the video stream's own clock.
    pub reorder_ticks: Option<i64>,
    pub is_keyframe: bool,
}

/// Every picture of a video, in the order they are shown.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameTable {
    frames: Vec<StoredFrame>,
    /// For each picture, when the picture stored just before it is decoded.
    stored_after: Vec<Option<f64>>,
    /// For each picture, whether a copy can start with it and end before it.
    copy_points: Vec<bool>,
}

/// What is known about one picture besides what the file says of it.
struct TableEntry {
    frame: StoredFrame,
    stored_after: Option<f64>,
    is_copy_point: bool,
}

/// For each stored picture, whether the file can be cut in two just before
/// it by copying: it is a keyframe, every picture stored before it is shown
/// before it, and every picture stored after it is shown after it.
///
/// Many videos have keyframes with a few pictures stored after them that are
/// shown before them, and built from pictures on both sides. A copy that
/// starts on such a keyframe cannot show those pictures, and a copy that
/// ends before it would lack them.
fn copy_points(stored: &[StoredFrame]) -> Vec<bool> {
    let mut latest_shown_before = Vec::with_capacity(stored.len());
    let mut latest = f64::NEG_INFINITY;
    for frame in stored {
        latest_shown_before.push(latest);
        latest = latest.max(frame.shown_at);
    }
    let mut earliest_shown_after = vec![f64::INFINITY; stored.len()];
    let mut earliest = f64::INFINITY;
    for (place, frame) in stored.iter().enumerate().rev() {
        earliest_shown_after[place] = earliest;
        earliest = earliest.min(frame.shown_at);
    }
    stored
        .iter()
        .zip(latest_shown_before.iter().zip(&earliest_shown_after))
        .map(|(frame, (&latest_before, &earliest_after))| {
            frame.is_keyframe && latest_before < frame.shown_at && frame.shown_at < earliest_after
        })
        .collect()
}

impl FrameTable {
    /// Takes the pictures in the order the file stores them.
    pub fn from_stored_order(stored: Vec<StoredFrame>) -> Result<Self, PlanError> {
        if stored.is_empty() {
            return Err(PlanError::NoPictures);
        }
        let decoded_before =
            std::iter::once(None).chain(stored.iter().map(|frame| frame.decoded_at));
        let mut entries: Vec<TableEntry> = stored
            .iter()
            .zip(decoded_before)
            .zip(copy_points(&stored))
            .map(|((&frame, stored_after), is_copy_point)| TableEntry {
                frame,
                stored_after,
                is_copy_point,
            })
            .collect();
        entries.sort_by(|a, b| a.frame.shown_at.total_cmp(&b.frame.shown_at));

        let shares_a_time = entries
            .windows(2)
            .find(|pair| pair[1].frame.shown_at <= pair[0].frame.shown_at);
        if let Some(pair) = shares_a_time {
            return Err(PlanError::PicturesShareATime {
                at: pair[1].frame.shown_at,
            });
        }
        Ok(Self {
            frames: entries.iter().map(|entry| entry.frame).collect(),
            stored_after: entries.iter().map(|entry| entry.stored_after).collect(),
            copy_points: entries.iter().map(|entry| entry.is_copy_point).collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn shown_at(&self, frame: usize) -> f64 {
        self.frames[frame].shown_at
    }

    /// The first picture shown at or after `time`, or `len()` when the video
    /// has ended by then.
    fn first_frame_at_or_after(&self, time: f64) -> usize {
        self.frames
            .partition_point(|frame| frame.shown_at < time - SAME_PICTURE_SECONDS)
    }

    /// Whether a copy can start with this picture. See `copy_points`.
    fn is_copy_point(&self, frame: usize) -> bool {
        self.copy_points[frame]
    }

    /// Whether a copy can end just before this picture: at a copy point, or
    /// at the end of the video.
    fn copy_can_end_before(&self, frame: usize) -> bool {
        frame == self.len() || self.is_copy_point(frame)
    }

    /// Half way between this picture and the one shown before it. A seek to
    /// here cannot be thrown to the wrong picture by rounding.
    fn just_before_shown(&self, frame: usize) -> Option<FfmpegTime> {
        let before = frame.checked_sub(1)?;
        Some(FfmpegTime::from_seconds(
            (self.shown_at(before) + self.shown_at(frame)) / 2.0,
        ))
    }
}

/// A run of pictures, counted in the order they are shown. Never empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRange {
    first: usize,
    end: usize,
}

impl FrameRange {
    fn new(first: usize, end: usize) -> Option<Self> {
        (first < end).then_some(Self { first, end })
    }

    pub fn first(self) -> usize {
        self.first
    }

    pub fn len(self) -> usize {
        self.end - self.first
    }

    pub fn frames(self) -> std::ops::Range<usize> {
        self.first..self.end
    }
}

/// Where the copy of a stretch begins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CopyStart {
    /// With the first picture of the video.
    VideoStart,
    /// With the first keyframe decoded at or after this time. ffmpeg picks
    /// the first packet of a copy by its decode time, so this is half way
    /// between the keyframe's and that of the picture stored before it.
    FirstKeyframeDecodedAfter(FfmpegTime),
}

/// How one part of a scene is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PartKind {
    /// Copied as stored. It starts on a keyframe and ends before one.
    Copied { start: CopyStart },
    /// Encoded again.
    EncodedAgain {
        /// Where to start decoding from, or `None` at the start of the
        /// video.
        seek: Option<FfmpegTime>,
        /// How much earlier than shown the copied pictures next to this part
        /// are decoded. The encoded pictures are given the same lead, so the
        /// decode times run on evenly across the join. Zero when the whole
        /// scene is encoded again.
        reorder_ticks: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScenePart {
    pub frames: FrameRange,
    pub kind: PartKind,
    /// How long the part is shown.
    pub seconds: f64,
}

impl ScenePart {
    pub fn is_copied(&self) -> bool {
        matches!(self.kind, PartKind::Copied { .. })
    }
}

/// One scene of an exact split.
#[derive(Debug, Clone, PartialEq)]
pub struct ScenePlan {
    pub frames: FrameRange,
    /// In the order they are shown, covering `frames` without gap or overlap.
    pub parts: Vec<ScenePart>,
    /// When the scene's first picture is shown in the original.
    pub starts_at: f64,
    /// The sound of the scene is every packet of sound that starts in this
    /// stretch of the original. `None` is the start or the end of the video.
    pub sound_from: Option<FfmpegTime>,
    pub sound_until: Option<FfmpegTime>,
}

/// Which pictures go into which scene, and how each is written.
#[derive(Debug, Clone, PartialEq)]
pub struct ExactCutPlan {
    scenes: Vec<ScenePlan>,
    frame_count: usize,
}

impl ExactCutPlan {
    pub fn scenes(&self) -> &[ScenePlan] {
        &self.scenes
    }

    /// How many pictures the video has, and so the scenes between them.
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// How many pictures are encoded again.
    pub fn frames_encoded_again(&self) -> usize {
        self.scenes
            .iter()
            .flat_map(|scene| &scene.parts)
            .filter(|part| !part.is_copied())
            .map(|part| part.frames.len())
            .sum()
    }
}

/// Why no plan could be made.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    NoPictures,
    /// Two pictures are shown at the same time, so pictures cannot be told
    /// apart by their times.
    PicturesShareATime {
        at: f64,
    },
    /// Two cuts choose the same picture, or a cut chooses the first picture
    /// or none, which would leave a scene with nothing in it.
    SceneWithoutPictures {
        cut_time: f64,
    },
    /// The file does not say when a keyframe is decoded, which a copy that
    /// starts there needs to know.
    DecodeOrderUnknown {
        at: f64,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPictures => write!(formatter, "the video has no pictures"),
            Self::PicturesShareATime { at } => write!(
                formatter,
                "two pictures of the video are shown at the same time, {at} seconds"
            ),
            Self::SceneWithoutPictures { cut_time } => write!(
                formatter,
                "the cut at {cut_time} seconds would leave a scene without pictures"
            ),
            Self::DecodeOrderUnknown { at } => write!(
                formatter,
                "the file does not say when the keyframe at {at} seconds is decoded"
            ),
        }
    }
}

impl std::error::Error for PlanError {}

/// Plans the scenes between `cuts`. Each cut lands on the first picture
/// shown at or after its time, so a scene starts less than one picture after
/// the moment that was chosen.
pub fn plan_exact_cuts(table: &FrameTable, cuts: &CutTimes) -> Result<ExactCutPlan, PlanError> {
    let mut scene_starts = vec![0];
    for &cut_time in cuts.as_slice() {
        let frame = table.first_frame_at_or_after(cut_time);
        let previous_start = scene_starts[scene_starts.len() - 1];
        if frame <= previous_start || frame >= table.len() {
            return Err(PlanError::SceneWithoutPictures { cut_time });
        }
        scene_starts.push(frame);
    }
    let scene_ends = scene_starts
        .iter()
        .skip(1)
        .copied()
        .chain([table.len()])
        .collect::<Vec<_>>();

    let scenes = scene_starts
        .iter()
        .zip(&scene_ends)
        .map(|(&first, &end)| plan_scene(table, FrameRange { first, end }))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ExactCutPlan {
        scenes,
        frame_count: table.len(),
    })
}

/// A scene is copied from the first keyframe a copy can start on to the
/// last, and encoded again before and after. A scene with no stretch between
/// two such keyframes is encoded again as a whole.
fn plan_scene(table: &FrameTable, scene: FrameRange) -> Result<ScenePlan, PlanError> {
    let first_copy_point = scene.frames().find(|&frame| table.is_copy_point(frame));
    let last_place_a_copy_can_end = if table.copy_can_end_before(scene.end) {
        Some(scene.end)
    } else {
        scene
            .frames()
            .rev()
            .find(|&frame| table.is_copy_point(frame))
    };

    let parts = match (first_copy_point, last_place_a_copy_can_end) {
        (Some(copy_first), Some(copy_end)) if copy_first < copy_end => {
            let head = FrameRange::new(scene.first, copy_first)
                .map(|frames| encoded_beside_a_copy(table, frames, copy_first))
                .transpose()?;
            let copied = copied_part(
                table,
                FrameRange {
                    first: copy_first,
                    end: copy_end,
                },
            )?;
            let tail = FrameRange::new(copy_end, scene.end)
                .map(|frames| encoded_beside_a_copy(table, frames, copy_end))
                .transpose()?;
            head.into_iter().chain([copied]).chain(tail).collect()
        }
        _ => vec![encoded_part(table, scene, 0)],
    };

    let is_first_scene = scene.first == 0;
    let is_last_scene = scene.end == table.len();
    Ok(ScenePlan {
        frames: scene,
        parts,
        starts_at: table.shown_at(scene.first),
        sound_from: (!is_first_scene)
            .then(|| FfmpegTime::from_seconds(table.shown_at(scene.first))),
        sound_until: (!is_last_scene).then(|| FfmpegTime::from_seconds(table.shown_at(scene.end))),
    })
}

fn copied_part(table: &FrameTable, frames: FrameRange) -> Result<ScenePart, PlanError> {
    let unknown = PlanError::DecodeOrderUnknown {
        at: table.shown_at(frames.first),
    };
    let start = if frames.first == 0 {
        CopyStart::VideoStart
    } else {
        let decoded_at = table.frames[frames.first].decoded_at;
        let stored_after = table.stored_after[frames.first];
        let (Some(decoded_at), Some(stored_after)) = (decoded_at, stored_after) else {
            return Err(unknown);
        };
        let between = FfmpegTime::from_seconds((decoded_at + stored_after) / 2.0);
        // ffmpeg cannot be asked to start a copy before time zero.
        if !between.is_positive() || stored_after >= decoded_at {
            return Err(unknown);
        }
        CopyStart::FirstKeyframeDecodedAfter(between)
    };
    Ok(ScenePart {
        frames,
        kind: PartKind::Copied { start },
        seconds: seconds_shown(table, frames),
    })
}

/// A part encoded again next to a copied one, which begins or ends at the
/// keyframe `joining_keyframe`.
fn encoded_beside_a_copy(
    table: &FrameTable,
    frames: FrameRange,
    joining_keyframe: usize,
) -> Result<ScenePart, PlanError> {
    let reorder_ticks =
        table.frames[joining_keyframe]
            .reorder_ticks
            .ok_or(PlanError::DecodeOrderUnknown {
                at: table.shown_at(joining_keyframe),
            })?;
    Ok(encoded_part(table, frames, reorder_ticks))
}

fn encoded_part(table: &FrameTable, frames: FrameRange, reorder_ticks: i64) -> ScenePart {
    ScenePart {
        frames,
        kind: PartKind::EncodedAgain {
            seek: table.just_before_shown(frames.first),
            reorder_ticks,
        },
        seconds: seconds_shown(table, frames),
    }
}

/// From the first picture of the run appearing to the picture after the run
/// appearing. The video's last picture is taken to last as long as the one
/// before it.
fn seconds_shown(table: &FrameTable, frames: FrameRange) -> f64 {
    let end = if frames.end < table.len() {
        table.shown_at(frames.end)
    } else {
        let last = table.len() - 1;
        let last_frame_seconds = match last.checked_sub(1) {
            Some(before) => table.shown_at(last) - table.shown_at(before),
            None => 0.0,
        };
        table.shown_at(last) + last_frame_seconds
    };
    end - table.shown_at(frames.first)
}

/// The cuts of a scan placed where the scene changes are, and not on the
/// keyframes nearest to them: in the middle of each black screen, and on
/// the first picture after each sudden change that the settings choose.
///
/// This is for a split without a review, where nobody chooses the pictures.
pub fn exact_cut_times(detection: &Detection, scan_kind: ScanKind) -> Vec<f64> {
    // The rules that choose among black screens and sudden changes place a
    // cut on the nearest point they are told a cut can go. Told that it can
    // go exactly where each change is, they choose the same changes and
    // leave them where they are.
    let black_screen_middles: Vec<f64> = detection
        .black_screens
        .iter()
        .map(|screen| (screen.start + screen.end) / 2.0)
        .collect();
    let black_screen_cuts = scenes::cut_points(
        &detection.black_screens,
        &black_screen_middles,
        detection.video,
    );
    let (ScanKind::HardCuts(settings), Some(candidates)) =
        (scan_kind, &detection.hard_cut_candidates)
    else {
        return black_screen_cuts;
    };
    let mut change_times: Vec<f64> = candidates.iter().map(|candidate| candidate.time).collect();
    change_times.sort_by(f64::total_cmp);
    hardcut::cuts_with_hard_cuts(
        &black_screen_cuts,
        candidates,
        &change_times,
        detection.video,
        &settings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardcut::{HardCutCandidate, HardCutSettings, Sensitivity};
    use crate::timecode::TimeSpan;

    const FRAME_SECONDS: f64 = 0.04;
    /// The pictures of the test video are decoded two pictures before they
    /// are shown, as happens when pictures are stored out of order.
    const REORDER_TICKS: i64 = 1024;
    const REORDER_SECONDS: f64 = 2.0 * FRAME_SECONDS;

    /// A video of `frame_count` pictures, 25 a second, with a keyframe every
    /// `keyframe_every` pictures.
    fn table(frame_count: usize, keyframe_every: usize) -> FrameTable {
        table_with_keyframes(frame_count, |frame| frame % keyframe_every == 0)
    }

    fn table_with_keyframes(frame_count: usize, is_keyframe: impl Fn(usize) -> bool) -> FrameTable {
        let stored = (0..frame_count)
            .map(|frame| StoredFrame {
                shown_at: frame as f64 * FRAME_SECONDS,
                decoded_at: Some(frame as f64 * FRAME_SECONDS - REORDER_SECONDS),
                reorder_ticks: Some(REORDER_TICKS),
                is_keyframe: is_keyframe(frame),
            })
            .collect();
        FrameTable::from_stored_order(stored).unwrap()
    }

    fn cuts(table: &FrameTable, times: &[f64]) -> CutTimes {
        let whole_video = TimeSpan {
            start: 0.0,
            end: table.len() as f64 * FRAME_SECONDS,
        };
        CutTimes::inside(whole_video, times.to_vec()).unwrap()
    }

    fn time_of(frame: usize) -> f64 {
        frame as f64 * FRAME_SECONDS
    }

    /// What each part of a scene does with which pictures, in few words.
    fn parts_in_short(scene: &ScenePlan) -> Vec<(&'static str, usize, usize)> {
        scene
            .parts
            .iter()
            .map(|part| {
                let what = if part.is_copied() { "copy" } else { "encode" };
                (what, part.frames.first(), part.frames.end)
            })
            .collect()
    }

    #[test]
    fn a_cut_between_keyframes_encodes_only_the_pictures_around_it() {
        // Keyframes at 0, 50, 100, 150, 200. The cut is on picture 73.
        let table = table(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(73)])).unwrap();

        assert_eq!(plan.scenes().len(), 2);
        assert_eq!(
            parts_in_short(&plan.scenes()[0]),
            vec![("copy", 0, 50), ("encode", 50, 73)]
        );
        assert_eq!(
            parts_in_short(&plan.scenes()[1]),
            vec![("encode", 73, 100), ("copy", 100, 225)]
        );
        assert_eq!(plan.frames_encoded_again(), 50);
        assert_eq!(plan.frame_count(), 225);
    }

    #[test]
    fn a_cut_on_a_keyframe_encodes_nothing() {
        let table = table(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(100)])).unwrap();

        assert_eq!(parts_in_short(&plan.scenes()[0]), vec![("copy", 0, 100)]);
        assert_eq!(parts_in_short(&plan.scenes()[1]), vec![("copy", 100, 225)]);
        assert_eq!(plan.frames_encoded_again(), 0);
    }

    #[test]
    fn a_scene_between_two_cuts_is_encoded_at_both_ends() {
        let table = table(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(30), time_of(170)])).unwrap();

        assert_eq!(
            parts_in_short(&plan.scenes()[1]),
            vec![("encode", 30, 50), ("copy", 50, 150), ("encode", 150, 170)]
        );
    }

    #[test]
    fn a_scene_with_no_stretch_between_two_keyframes_is_encoded_as_a_whole() {
        let table = table(225, 50);
        // Inside one stretch between keyframes, and across a single keyframe.
        let plan = plan_exact_cuts(
            &table,
            &cuts(&table, &[time_of(10), time_of(30), time_of(70)]),
        )
        .unwrap();

        assert_eq!(parts_in_short(&plan.scenes()[1]), vec![("encode", 10, 30)]);
        assert_eq!(parts_in_short(&plan.scenes()[2]), vec![("encode", 30, 70)]);
        assert_eq!(
            plan.scenes()[2].parts[0].kind,
            PartKind::EncodedAgain {
                seek: Some(FfmpegTime::from_seconds(time_of(30) - FRAME_SECONDS / 2.0)),
                reorder_ticks: 0,
            }
        );
    }

    #[test]
    fn a_scene_that_starts_on_a_keyframe_and_ends_before_the_next_is_encoded() {
        let table = table(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(50), time_of(70)])).unwrap();
        // A copy could start on picture 50 but has no keyframe to end before.
        assert_eq!(parts_in_short(&plan.scenes()[1]), vec![("encode", 50, 70)]);
    }

    #[test]
    fn the_scenes_hold_every_picture_once() {
        let table = table(225, 50);
        let plan = plan_exact_cuts(
            &table,
            &cuts(
                &table,
                &[time_of(10), time_of(73), time_of(150), time_of(224)],
            ),
        )
        .unwrap();

        let mut next_frame = 0;
        for scene in plan.scenes() {
            assert_eq!(scene.frames.first(), next_frame);
            let mut next_in_scene = scene.frames.first();
            for part in &scene.parts {
                assert_eq!(part.frames.first(), next_in_scene);
                next_in_scene = part.frames.end;
            }
            assert_eq!(next_in_scene, scene.frames.end);
            next_frame = scene.frames.end;
        }
        assert_eq!(next_frame, table.len());
    }

    #[test]
    fn a_cut_lands_on_the_first_picture_at_or_after_its_time() {
        let table = table(225, 50);
        let between_pictures = time_of(73) - FRAME_SECONDS / 4.0;
        let plan = plan_exact_cuts(&table, &cuts(&table, &[between_pictures])).unwrap();

        assert_eq!(plan.scenes()[1].frames.first(), 73);
        assert_eq!(plan.scenes()[1].starts_at, time_of(73));
    }

    #[test]
    fn a_time_a_hair_after_a_picture_appears_still_means_that_picture() {
        let table = table(225, 50);
        let a_hair_after = time_of(73) + SAME_PICTURE_SECONDS / 2.0;
        let plan = plan_exact_cuts(&table, &cuts(&table, &[a_hair_after])).unwrap();
        assert_eq!(plan.scenes()[1].frames.first(), 73);
    }

    #[test]
    fn a_copy_starts_at_the_keyframe_by_its_decode_time() {
        let table = table(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(73)])).unwrap();

        assert_eq!(
            plan.scenes()[0].parts[0].kind,
            PartKind::Copied {
                start: CopyStart::VideoStart
            }
        );
        // Picture 100 is decoded at 3.92 s and the one stored before it at
        // 3.88 s.
        assert_eq!(
            plan.scenes()[1].parts[1].kind,
            PartKind::Copied {
                start: CopyStart::FirstKeyframeDecodedAfter(FfmpegTime::from_seconds(3.90))
            }
        );
    }

    #[test]
    fn an_encoded_part_beside_a_copy_takes_over_its_decode_lead() {
        let table = table(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(73)])).unwrap();

        assert_eq!(
            plan.scenes()[0].parts[1].kind,
            PartKind::EncodedAgain {
                seek: Some(FfmpegTime::from_seconds(time_of(50) - FRAME_SECONDS / 2.0)),
                reorder_ticks: REORDER_TICKS,
            }
        );
    }

    #[test]
    fn each_part_is_as_long_as_its_pictures_are_shown() {
        let table = table(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(73)])).unwrap();
        let seconds: Vec<f64> = plan
            .scenes()
            .iter()
            .flat_map(|scene| &scene.parts)
            .map(|part| part.seconds)
            .collect();

        for (seconds, pictures) in seconds.iter().zip([50, 23, 27, 125]) {
            assert!(
                (seconds - pictures as f64 * FRAME_SECONDS).abs() < 1e-9,
                "{seconds} for {pictures} pictures"
            );
        }
    }

    #[test]
    fn the_sound_of_a_scene_runs_from_its_first_picture_to_the_next_scene() {
        let table = table(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(73)])).unwrap();
        let cut = Some(FfmpegTime::from_seconds(time_of(73)));

        assert_eq!(plan.scenes()[0].sound_from, None);
        assert_eq!(plan.scenes()[0].sound_until, cut);
        assert_eq!(plan.scenes()[1].sound_from, cut);
        assert_eq!(plan.scenes()[1].sound_until, None);
    }

    #[test]
    fn a_video_that_does_not_start_on_a_keyframe_has_its_start_encoded() {
        let table = table_with_keyframes(100, |frame| frame == 20 || frame == 60);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[])).unwrap();
        assert_eq!(
            parts_in_short(&plan.scenes()[0]),
            vec![("encode", 0, 20), ("copy", 20, 100)]
        );
        assert_eq!(
            plan.scenes()[0].parts[0].kind,
            PartKind::EncodedAgain {
                seek: None,
                reorder_ticks: REORDER_TICKS,
            }
        );
    }

    #[test]
    fn two_cuts_on_one_picture_are_refused() {
        let table = table(225, 50);
        let same_picture = [time_of(73) - 0.01, time_of(73)];
        assert_eq!(
            plan_exact_cuts(&table, &cuts(&table, &same_picture)),
            Err(PlanError::SceneWithoutPictures {
                cut_time: time_of(73)
            })
        );
    }

    #[test]
    fn a_cut_after_the_last_picture_is_refused() {
        let table = table(225, 50);
        let after_the_last_picture = time_of(224) + FRAME_SECONDS / 2.0;
        assert_eq!(
            plan_exact_cuts(&table, &cuts(&table, &[after_the_last_picture])),
            Err(PlanError::SceneWithoutPictures {
                cut_time: after_the_last_picture
            })
        );
    }

    #[test]
    fn a_copy_cannot_start_where_the_decode_time_is_unknown() {
        let stored = (0..100)
            .map(|frame| StoredFrame {
                shown_at: time_of(frame),
                decoded_at: None,
                reorder_ticks: None,
                is_keyframe: frame % 50 == 0,
            })
            .collect();
        let table = FrameTable::from_stored_order(stored).unwrap();

        assert_eq!(
            plan_exact_cuts(&table, &cuts(&table, &[time_of(20)])),
            Err(PlanError::DecodeOrderUnknown { at: time_of(50) })
        );
        // Copied from the start of the video, nothing needs to be known.
        assert!(plan_exact_cuts(&table, &cuts(&table, &[])).is_ok());
    }

    #[test]
    fn pictures_stored_out_of_order_are_put_in_the_order_they_are_shown() {
        let stored_at = |shown: usize, decoded: f64| StoredFrame {
            shown_at: time_of(shown),
            decoded_at: Some(decoded),
            reorder_ticks: None,
            is_keyframe: shown == 0,
        };
        let table = FrameTable::from_stored_order(vec![
            stored_at(0, -0.08),
            stored_at(3, -0.04),
            stored_at(1, 0.0),
            stored_at(2, 0.04),
        ])
        .unwrap();

        let shown: Vec<f64> = (0..table.len())
            .map(|frame| table.shown_at(frame))
            .collect();
        assert_eq!(shown, vec![time_of(0), time_of(1), time_of(2), time_of(3)]);
        // Picture 1 is stored right after picture 3, which is decoded at -0.04.
        assert_eq!(table.stored_after[1], Some(-0.04));
        assert_eq!(table.stored_after[0], None);
    }

    /// Pictures stored the way an encoder with open groups stores them:
    /// each keyframe after the first is followed by the two pictures that
    /// are shown just before it.
    fn table_with_open_groups(frame_count: usize, keyframe_every: usize) -> FrameTable {
        let mut shown_order: Vec<usize> = (0..frame_count).collect();
        for keyframe in (keyframe_every..frame_count).step_by(keyframe_every) {
            shown_order[keyframe - 2..=keyframe].rotate_right(1);
        }
        let stored = shown_order
            .into_iter()
            .enumerate()
            .map(|(place, shown)| StoredFrame {
                shown_at: time_of(shown),
                decoded_at: Some(time_of(place) - REORDER_SECONDS),
                reorder_ticks: Some(REORDER_TICKS),
                is_keyframe: shown % keyframe_every == 0,
            })
            .collect();
        FrameTable::from_stored_order(stored).unwrap()
    }

    #[test]
    fn a_keyframe_followed_by_pictures_shown_before_it_is_no_place_to_copy_from() {
        let table = table_with_open_groups(225, 50);
        assert!(table.is_copy_point(0));
        for keyframe in [50, 100, 150, 200] {
            assert!(!table.is_copy_point(keyframe), "keyframe {keyframe}");
        }

        // With nowhere to start or end a copy, both scenes are encoded again.
        let plan = plan_exact_cuts(&table, &cuts(&table, &[time_of(73)])).unwrap();
        assert_eq!(parts_in_short(&plan.scenes()[0]), vec![("encode", 0, 73)]);
        assert_eq!(parts_in_short(&plan.scenes()[1]), vec![("encode", 73, 225)]);
    }

    #[test]
    fn a_video_cut_at_no_point_is_copied_whole_even_with_open_groups() {
        let table = table_with_open_groups(225, 50);
        let plan = plan_exact_cuts(&table, &cuts(&table, &[])).unwrap();
        assert_eq!(parts_in_short(&plan.scenes()[0]), vec![("copy", 0, 225)]);
    }

    #[test]
    fn a_picture_that_is_no_keyframe_is_no_place_to_copy_from() {
        let table = table(225, 50);
        assert!(table.is_copy_point(50));
        assert!(!table.is_copy_point(49));
        assert!(!table.is_copy_point(51));
    }

    #[test]
    fn a_table_needs_pictures_with_times_of_their_own() {
        assert_eq!(
            FrameTable::from_stored_order(Vec::new()),
            Err(PlanError::NoPictures)
        );
        let twice = StoredFrame {
            shown_at: 1.0,
            decoded_at: None,
            reorder_ticks: None,
            is_keyframe: true,
        };
        assert_eq!(
            FrameTable::from_stored_order(vec![twice, twice]),
            Err(PlanError::PicturesShareATime { at: 1.0 })
        );
    }

    #[test]
    fn ffmpeg_times_are_written_to_the_microsecond() {
        assert_eq!(FfmpegTime::from_seconds(2.92).to_string(), "2.920000");
        assert_eq!(FfmpegTime::from_seconds(3723.5).to_string(), "3723.500000");
        assert_eq!(FfmpegTime::from_seconds(-0.02).to_string(), "-0.020000");
        assert_eq!(FfmpegTime::ZERO.to_string(), "0.000000");
    }

    #[test]
    fn differences_of_ffmpeg_times_are_exact() {
        let from = FfmpegTime::from_seconds(2.92);
        let until = FfmpegTime::from_seconds(7.133333);
        assert_eq!(until.since(from).to_string(), "4.213333");
        assert_eq!(from.earlier_of(until), from);
    }

    fn detection_with_changes() -> Detection {
        Detection {
            video: TimeSpan {
                start: 0.0,
                end: 100.0,
            },
            keyframe_times: vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0],
            frame_count: 2500,
            black_screens: vec![TimeSpan {
                start: 31.0,
                end: 33.0,
            }],
            cuts: vec![30.0],
            decoded_frames: 2500,
            decoded_windows: 1,
            hard_cut_candidates: Some(vec![HardCutCandidate {
                time: 64.36,
                strength: 255,
            }]),
            chapters: None,
        }
    }

    #[test]
    fn an_exact_cut_for_a_black_screen_is_in_its_middle() {
        let detection = detection_with_changes();
        assert_eq!(
            exact_cut_times(&detection, ScanKind::BlackScreens),
            vec![32.0]
        );
    }

    #[test]
    fn an_exact_cut_for_a_sudden_change_is_on_the_change() {
        let detection = detection_with_changes();
        let settings = HardCutSettings {
            sensitivity: Sensitivity::Medium,
            min_scene_seconds: 5.0,
        };
        assert_eq!(
            exact_cut_times(&detection, ScanKind::HardCuts(settings)),
            vec![32.0, 64.36]
        );
    }

    #[test]
    fn a_settings_file_from_before_the_choice_cuts_on_keyframes() {
        assert_eq!(CutPrecision::default(), CutPrecision::NearestKeyframe);
    }
}
