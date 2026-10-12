//! Finds hard cuts: scene changes with no black screen, where one frame
//! shows the old scene and the very next frame shows the new one.
//!
//! Every frame is compared with the one before it. A hard cut shows up as a
//! single frame that differs sharply from its predecessor while the frames
//! around it barely change. Films cut between camera angles every few
//! seconds, so the strongest changes are kept and the rest dropped until the
//! scenes are at least as long as the person asked for.

use std::path::Path;

use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::probe::VideoPacket;
use crate::scan::{self, FrameProgress, OnProgress, UndecodedFrames};
use crate::scenes;
use crate::timecode::TimeSpan;
use crate::windows::FrameWindow;

/// How many frames on each side a change is compared with.
const NEIGHBOURS_EACH_SIDE: usize = 6;
/// A cut changes the picture this many times more than its neighbours do.
/// Fast action changes every frame, a cut changes one.
const SPIKE_RATIO: f32 = 3.0;
/// The weakest change worth remembering, which is the threshold of the most
/// sensitive setting. Changes are measured from 0 (identical frames) to 255.
const WEAKEST_CANDIDATE: u8 = 12;

const DEFAULT_MIN_SCENE_SECONDS: f64 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Sensitivity {
    Low,
    Medium,
    High,
}

impl Sensitivity {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    /// Named by what gets detected, since "sensitivity" alone does not say
    /// whether more of it means more scenes or fewer.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Only obvious changes",
            Self::Medium => "Most changes",
            Self::High => "Even subtle changes",
        }
    }

    /// How much the picture must change between two frames to count.
    fn weakest_change(self) -> u8 {
        match self {
            Self::Low => 36,
            Self::Medium => 22,
            Self::High => WEAKEST_CANDIDATE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HardCutSettings {
    pub sensitivity: Sensitivity,
    /// Hard cuts closer together than this are thinned out, strongest first.
    pub min_scene_seconds: f64,
}

impl Default for HardCutSettings {
    fn default() -> Self {
        Self {
            sensitivity: Sensitivity::Medium,
            min_scene_seconds: DEFAULT_MIN_SCENE_SECONDS,
        }
    }
}

/// A frame where the picture changed abruptly. Stored with the scan, so the
/// settings can be changed later without decoding the video again.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HardCutCandidate {
    pub time: f64,
    /// How much the picture changed, from 0 to 255.
    pub strength: u8,
}

/// What decoding every frame of a video found.
#[derive(Debug, PartialEq)]
pub struct HardCutScan {
    pub candidates: Vec<HardCutCandidate>,
    pub undecoded: Vec<UndecodedFrames>,
}

/// Decodes every frame of the video and returns the abrupt changes in it,
/// reporting progress as the share of the frames decoded so far.
pub fn find_candidates(
    video: &Path,
    packets: &[VideoPacket],
    windows: &[FrameWindow],
    on_progress: OnProgress,
) -> Result<HardCutScan> {
    let measured = measure_frame_changes(video, packets, windows, on_progress)?;
    let frame_times: Vec<f64> = packets.iter().map(|packet| packet.time).collect();
    Ok(HardCutScan {
        candidates: candidates_among(&measured.changes, &frame_times),
        undecoded: measured.undecoded,
    })
}

/// What decoding the whole video learned.
struct FrameChanges {
    /// For every frame, how much it differs from the frame before it.
    changes: Vec<u8>,
    undecoded: Vec<UndecodedFrames>,
}

/// What one window's decode learned.
struct WindowChanges {
    /// How much each frame differs from the one before it. The first entry
    /// is zero, since its predecessor belongs to the previous window.
    changes: Vec<u8>,
    first_thumbnail: Option<Vec<u8>>,
    last_thumbnail: Option<Vec<u8>>,
    undecoded: Option<UndecodedFrames>,
}

/// Returns, for every frame, how much it differs from the frame before it.
/// `windows` must cover the video without gaps, in order.
fn measure_frame_changes(
    video: &Path,
    packets: &[VideoPacket],
    windows: &[FrameWindow],
    on_progress: OnProgress,
) -> Result<FrameChanges> {
    let progress = FrameProgress::new(windows.iter().map(FrameWindow::len).sum(), on_progress);
    let changes_per_window = windows
        .par_iter()
        .map(|window| measure_window(video, packets, window, &progress))
        .collect::<Result<Vec<_>>>()?;
    progress.finish();

    let mut changes = vec![0u8; packets.len()];
    let mut last_thumbnail_before: Option<&Vec<u8>> = None;
    for (window, measured) in windows.iter().zip(&changes_per_window) {
        let frames = window.start..window.start + measured.changes.len();
        changes[frames].copy_from_slice(&measured.changes);
        // The change into a window's first frame spans two decodes.
        if let (Some(before), Some(first)) = (last_thumbnail_before, &measured.first_thumbnail) {
            changes[window.start] = mean_difference(before, first);
        }
        last_thumbnail_before = measured.last_thumbnail.as_ref();
    }
    let undecoded = changes_per_window
        .iter()
        .filter_map(|measured| measured.undecoded)
        .collect();
    Ok(FrameChanges { changes, undecoded })
}

fn measure_window(
    video: &Path,
    packets: &[VideoPacket],
    window: &FrameWindow,
    progress: &FrameProgress,
) -> Result<WindowChanges> {
    let mut changes = Vec::with_capacity(window.len());
    let mut first_thumbnail = None;
    let mut previous: Option<Vec<u8>> = None;
    let undecoded = scan::decode_thumbnails(video, packets, window, |thumbnail| {
        match &mut previous {
            Some(previous) => {
                changes.push(mean_difference(previous, thumbnail));
                previous.copy_from_slice(thumbnail);
            }
            None => {
                changes.push(0);
                first_thumbnail = Some(thumbnail.to_vec());
                previous = Some(thumbnail.to_vec());
            }
        }
        progress.frame_decoded();
    })?;
    Ok(WindowChanges {
        changes,
        first_thumbnail,
        last_thumbnail: previous,
        undecoded,
    })
}

/// The average brightness difference between two thumbnails, pixel by pixel.
fn mean_difference(one: &[u8], other: &[u8]) -> u8 {
    if one.is_empty() {
        return 0;
    }
    let total: u64 = one
        .iter()
        .zip(other)
        .map(|(&a, &b)| u64::from(a.abs_diff(b)))
        .sum();
    (total / one.len() as u64) as u8
}

/// The frames whose change stands far above that of the frames around them.
fn candidates_among(changes: &[u8], frame_times: &[f64]) -> Vec<HardCutCandidate> {
    changes
        .iter()
        .enumerate()
        .filter(|&(frame, &change)| {
            change >= WEAKEST_CANDIDATE
                && f32::from(change) >= SPIKE_RATIO * typical_change_around(changes, frame)
        })
        .map(|(frame, &change)| HardCutCandidate {
            time: frame_times[frame],
            strength: change,
        })
        .collect()
}

/// The average change of the frames on either side of `frame`, without the
/// frame itself.
fn typical_change_around(changes: &[u8], frame: usize) -> f32 {
    let first = frame.saturating_sub(NEIGHBOURS_EACH_SIDE);
    let last = (frame + NEIGHBOURS_EACH_SIDE).min(changes.len() - 1);
    let neighbours = (first..=last).filter(|&neighbour| neighbour != frame);
    let (total, count) = neighbours.fold((0u32, 0u32), |(total, count), neighbour| {
        (total + u32::from(changes[neighbour]), count + 1)
    });
    if count == 0 {
        return 0.0;
    }
    total as f32 / count as f32
}

/// The cuts of a hard-cut scan: every black-screen cut, plus the strongest
/// hard cuts that leave scenes of at least the wanted length.
///
/// Like every cut, a hard cut is placed on the nearest keyframe. Encoders
/// usually put a keyframe on a scene change, so that is mostly the cut
/// itself.
pub fn cuts_with_hard_cuts(
    black_screen_cuts: &[f64],
    candidates: &[HardCutCandidate],
    keyframe_times: &[f64],
    video: TimeSpan,
    settings: &HardCutSettings,
) -> Vec<f64> {
    let mut strong_enough: Vec<HardCutCandidate> = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.strength >= settings.sensitivity.weakest_change())
        .collect();
    strong_enough.sort_by_key(|candidate| std::cmp::Reverse(candidate.strength));

    let mut cuts = black_screen_cuts.to_vec();
    for candidate in strong_enough {
        let exact_moment = TimeSpan {
            start: candidate.time,
            end: candidate.time,
        };
        let Some(cut) = scenes::nearest_keyframe(&exact_moment, keyframe_times) else {
            continue;
        };
        // Checked on its own, so the result holds even for a shortest scene
        // of zero: no cut twice, and none on the ends of the video.
        let is_new_cut_inside_the_video =
            cut > video.start && cut < video.end && !cuts.contains(&cut);
        let leaves_room = |other: f64| (cut - other).abs() >= settings.min_scene_seconds;
        if is_new_cut_inside_the_video
            && leaves_room(video.start)
            && leaves_room(video.end)
            && cuts.iter().all(|&c| leaves_room(c))
        {
            cuts.push(cut);
        }
    }
    cuts.sort_by(f64::total_cmp);
    cuts
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 1000.0,
    };

    fn candidate(time: f64, strength: u8) -> HardCutCandidate {
        HardCutCandidate { time, strength }
    }

    fn keyframes_every_ten_seconds() -> Vec<f64> {
        (0..100)
            .map(|keyframe| f64::from(keyframe) * 10.0)
            .collect()
    }

    fn settings(sensitivity: Sensitivity, min_scene_seconds: f64) -> HardCutSettings {
        HardCutSettings {
            sensitivity,
            min_scene_seconds,
        }
    }

    /// One frame per second.
    fn times(frame_count: usize) -> Vec<f64> {
        (0..frame_count).map(|frame| frame as f64).collect()
    }

    #[test]
    fn identical_thumbnails_do_not_differ() {
        assert_eq!(mean_difference(&[10, 200, 30], &[10, 200, 30]), 0);
    }

    #[test]
    fn the_difference_is_averaged_over_the_pixels() {
        assert_eq!(mean_difference(&[0, 0, 0, 0], &[100, 0, 0, 20]), 30);
    }

    #[test]
    fn a_single_sharp_change_in_calm_footage_is_a_candidate() {
        let mut changes = vec![2u8; 40];
        changes[20] = 60;
        assert_eq!(
            candidates_among(&changes, &times(40)),
            vec![candidate(20.0, 60)]
        );
    }

    #[test]
    fn fast_action_that_changes_every_frame_is_not_a_cut() {
        let changes = vec![40u8; 40];
        assert!(candidates_among(&changes, &times(40)).is_empty());
    }

    #[test]
    fn a_faint_change_is_ignored_even_in_still_footage() {
        let mut changes = vec![0u8; 40];
        changes[20] = WEAKEST_CANDIDATE - 1;
        assert!(candidates_among(&changes, &times(40)).is_empty());
    }

    #[test]
    fn a_change_on_the_very_first_or_last_frame_is_handled() {
        let mut changes = vec![1u8; 10];
        changes[0] = 80;
        changes[9] = 80;
        let found = candidates_among(&changes, &times(10));
        assert_eq!(found, vec![candidate(0.0, 80), candidate(9.0, 80)]);
    }

    #[test]
    fn hard_cuts_land_on_the_nearest_keyframe() {
        let cuts = cuts_with_hard_cuts(
            &[],
            &[candidate(497.0, 90)],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, 60.0),
        );
        assert_eq!(cuts, vec![500.0]);
    }

    #[test]
    fn close_hard_cuts_are_thinned_out_strongest_first() {
        let cuts = cuts_with_hard_cuts(
            &[],
            &[
                candidate(300.0, 40),
                candidate(330.0, 90),
                candidate(700.0, 50),
            ],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, 120.0),
        );
        // 300 is within two minutes of the stronger cut at 330.
        assert_eq!(cuts, vec![330.0, 700.0]);
    }

    #[test]
    fn black_screen_cuts_are_always_kept() {
        let cuts = cuts_with_hard_cuts(
            &[400.0],
            &[candidate(430.0, 250), candidate(800.0, 60)],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, 120.0),
        );
        // The hard cut at 430 is too close to the black screen at 400.
        assert_eq!(cuts, vec![400.0, 800.0]);
    }

    #[test]
    fn hard_cuts_keep_their_distance_from_the_ends_of_the_video() {
        let cuts = cuts_with_hard_cuts(
            &[],
            &[candidate(50.0, 200), candidate(960.0, 200)],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, 120.0),
        );
        assert!(cuts.is_empty());
    }

    #[test]
    fn with_no_shortest_scene_cuts_are_still_never_repeated_or_on_the_ends() {
        let cuts = cuts_with_hard_cuts(
            &[400.0],
            &[
                candidate(2.0, 200),
                candidate(401.0, 200),
                candidate(603.0, 90),
                candidate(598.0, 80),
            ],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, 0.0),
        );
        // 2 s snaps to the start, 401 s to the existing cut, and both 603 s
        // and 598 s to the keyframe at 600 s.
        assert_eq!(cuts, vec![400.0, 600.0]);
    }

    #[test]
    fn higher_sensitivity_accepts_weaker_changes() {
        let weak = [candidate(500.0, 15)];
        let keyframes = keyframes_every_ten_seconds();
        let with = |sensitivity| {
            cuts_with_hard_cuts(&[], &weak, &keyframes, VIDEO, &settings(sensitivity, 60.0))
        };
        assert!(with(Sensitivity::Low).is_empty());
        assert!(with(Sensitivity::Medium).is_empty());
        assert_eq!(with(Sensitivity::High), vec![500.0]);
    }

    /// A frame far enough from both ends of 40 frames to have a full set of
    /// neighbours.
    const MIDDLE_FRAME: usize = 20;
    const FRAME_COUNT: usize = 40;

    #[test]
    fn empty_thumbnails_do_not_differ() {
        assert_eq!(mean_difference(&[], &[]), 0);
        assert_eq!(mean_difference(&[], &[10, 20]), 0);
    }

    #[test]
    fn the_average_difference_is_rounded_down() {
        // A total of 7 over 4 pixels is 1.75.
        assert_eq!(mean_difference(&[0, 0, 0, 0], &[7, 0, 0, 0]), 1);
    }

    #[test]
    fn black_against_white_is_the_largest_difference_in_either_order() {
        let black = [u8::MIN; 4];
        let white = [u8::MAX; 4];
        assert_eq!(mean_difference(&black, &white), u8::MAX);
        assert_eq!(mean_difference(&white, &black), u8::MAX);
    }

    #[test]
    fn a_video_with_no_frames_has_no_candidates() {
        assert!(candidates_among(&[], &[]).is_empty());
    }

    #[test]
    fn the_typical_change_leaves_out_the_frame_itself() {
        const CALM_CHANGE: u8 = 4;
        let mut changes = vec![CALM_CHANGE; FRAME_COUNT];
        changes[MIDDLE_FRAME] = 200;
        assert_eq!(
            typical_change_around(&changes, MIDDLE_FRAME),
            f32::from(CALM_CHANGE)
        );
    }

    #[test]
    fn the_typical_change_ignores_frames_beyond_the_neighbourhood() {
        let mut changes = vec![0u8; FRAME_COUNT];
        changes[MIDDLE_FRAME - NEIGHBOURS_EACH_SIDE - 1] = 240;
        changes[MIDDLE_FRAME + NEIGHBOURS_EACH_SIDE + 1] = 240;
        assert_eq!(typical_change_around(&changes, MIDDLE_FRAME), 0.0);
    }

    #[test]
    fn the_typical_change_at_the_first_frame_uses_only_the_frames_after_it() {
        const CALM_CHANGE: u8 = 3;
        let mut changes = vec![CALM_CHANGE; FRAME_COUNT];
        changes[0] = 90;
        assert_eq!(typical_change_around(&changes, 0), f32::from(CALM_CHANGE));
    }

    #[test]
    fn a_lone_frame_has_no_typical_change() {
        assert_eq!(typical_change_around(&[50], 0), 0.0);
    }

    #[test]
    fn a_sharp_change_is_found_in_a_video_shorter_than_the_neighbourhood() {
        let changes = [1, 50, 1];
        assert_eq!(
            candidates_among(&changes, &times(changes.len())),
            vec![candidate(1.0, 50)]
        );
    }

    #[test]
    fn a_change_exactly_at_the_spike_ratio_is_a_candidate_and_one_below_is_not() {
        const BUSY_CHANGE: u8 = 5;
        const SPIKE: u8 = SPIKE_RATIO as u8 * BUSY_CHANGE;
        let mut changes = vec![BUSY_CHANGE; FRAME_COUNT];

        changes[MIDDLE_FRAME] = SPIKE;
        assert_eq!(
            candidates_among(&changes, &times(FRAME_COUNT)),
            vec![candidate(MIDDLE_FRAME as f64, SPIKE)]
        );

        changes[MIDDLE_FRAME] = SPIKE - 1;
        assert!(candidates_among(&changes, &times(FRAME_COUNT)).is_empty());
    }

    #[test]
    fn the_weakest_change_worth_remembering_is_a_candidate_in_still_footage() {
        let mut changes = vec![0u8; FRAME_COUNT];
        changes[MIDDLE_FRAME] = WEAKEST_CANDIDATE;
        assert_eq!(
            candidates_among(&changes, &times(FRAME_COUNT)),
            vec![candidate(MIDDLE_FRAME as f64, WEAKEST_CANDIDATE)]
        );
    }

    #[test]
    fn a_flash_is_a_candidate_on_the_way_in_and_on_the_way_out() {
        const FLASH_CHANGE: u8 = 60;
        let mut changes = vec![2u8; FRAME_COUNT];
        // One bright frame differs from the frame before it, and the frame
        // after it differs just as much.
        changes[MIDDLE_FRAME] = FLASH_CHANGE;
        changes[MIDDLE_FRAME + 1] = FLASH_CHANGE;

        assert_eq!(
            candidates_among(&changes, &times(FRAME_COUNT)),
            vec![
                candidate(MIDDLE_FRAME as f64, FLASH_CHANGE),
                candidate((MIDDLE_FRAME + 1) as f64, FLASH_CHANGE)
            ]
        );
    }

    #[test]
    fn the_sensitivities_are_listed_from_the_strictest_to_the_most_accepting() {
        let thresholds = Sensitivity::ALL.map(Sensitivity::weakest_change);
        assert!(thresholds.windows(2).all(|pair| pair[0] > pair[1]));
    }

    #[test]
    fn the_most_sensitive_setting_accepts_every_remembered_candidate() {
        assert_eq!(Sensitivity::High.weakest_change(), WEAKEST_CANDIDATE);
    }

    #[test]
    fn a_change_exactly_at_the_threshold_is_accepted_and_one_below_is_not() {
        let threshold = Sensitivity::Medium.weakest_change();
        let keyframes = keyframes_every_ten_seconds();
        let with_strength = |strength| {
            cuts_with_hard_cuts(
                &[],
                &[candidate(500.0, strength)],
                &keyframes,
                VIDEO,
                &settings(Sensitivity::Medium, 60.0),
            )
        };
        assert_eq!(with_strength(threshold), vec![500.0]);
        assert!(with_strength(threshold - 1).is_empty());
    }

    #[test]
    fn a_video_with_no_keyframes_keeps_only_its_black_screen_cuts() {
        let cuts = cuts_with_hard_cuts(
            &[400.0],
            &[candidate(700.0, 200)],
            &[],
            VIDEO,
            &settings(Sensitivity::Medium, 120.0),
        );
        assert_eq!(cuts, vec![400.0]);
    }

    #[test]
    fn of_two_equally_strong_close_hard_cuts_the_earlier_one_is_kept() {
        let cuts = cuts_with_hard_cuts(
            &[],
            &[candidate(300.0, 90), candidate(330.0, 90)],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, 120.0),
        );
        assert_eq!(cuts, vec![300.0]);
    }

    #[test]
    fn a_strong_hard_cut_beats_a_weaker_one_on_each_side() {
        let cuts = cuts_with_hard_cuts(
            &[],
            &[
                candidate(300.0, 50),
                candidate(400.0, 90),
                candidate(500.0, 50),
            ],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, 120.0),
        );
        // Taken in order of time, 300 and 500 would both fit. The strongest
        // is placed first, and each of them is within two minutes of it.
        assert_eq!(cuts, vec![400.0]);
    }

    #[test]
    fn hard_cuts_exactly_the_shortest_scene_apart_are_both_kept() {
        const MIN_SCENE_SECONDS: f64 = 120.0;
        let cuts = cuts_with_hard_cuts(
            &[],
            &[
                candidate(300.0, 90),
                candidate(300.0 + MIN_SCENE_SECONDS, 80),
            ],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, MIN_SCENE_SECONDS),
        );
        assert_eq!(cuts, vec![300.0, 420.0]);
    }

    #[test]
    fn a_hard_cut_exactly_the_shortest_scene_from_an_end_of_the_video_is_kept() {
        const MIN_SCENE_SECONDS: f64 = 120.0;
        let cuts = cuts_with_hard_cuts(
            &[],
            &[
                candidate(VIDEO.start + MIN_SCENE_SECONDS, 90),
                candidate(VIDEO.end - MIN_SCENE_SECONDS, 90),
            ],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, MIN_SCENE_SECONDS),
        );
        assert_eq!(cuts, vec![120.0, 880.0]);
    }

    #[test]
    fn a_hard_cut_before_a_black_screen_cut_comes_first_in_the_result() {
        let cuts = cuts_with_hard_cuts(
            &[800.0],
            &[candidate(300.0, 90)],
            &keyframes_every_ten_seconds(),
            VIDEO,
            &settings(Sensitivity::Medium, 120.0),
        );
        assert_eq!(cuts, vec![300.0, 800.0]);
    }
}
