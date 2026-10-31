//! Chooses which frames to decode.
//!
//! A black frame compresses to almost nothing (about 25 bytes against several
//! kilobytes for a normal frame), so the packet sizes alone point at the few
//! places worth decoding. Decoding then confirms what is really black.

use std::ops::Range;

use crate::probe::VideoPacket;

/// A run of consecutive frames, as indices into the packet index.
pub type FrameWindow = Range<usize>;

/// A packet this many times smaller than the median one may be a black frame.
///
/// Measured on a low bitrate 480p H.264 file, where the margin is thinnest:
/// black frames were about 60 times smaller than the median. A divisor of 8
/// kept every black screen and gave 24 windows to decode. A divisor of 4 let
/// in the small B-frames of ordinary quiet shots and gave 271.
const TINY_PACKET_DIVISOR: u32 = 8;

/// A packet this small is suspicious whatever the rest of the video looks
/// like. Without it a video that is mostly black, or mostly still, has so
/// small a median that nothing stands out against it and its black screens
/// go unseen. Such a video is simply decoded in full.
const ALWAYS_TINY_BYTES: u32 = 64;

/// Tiny runs this close together are treated as one stretch. Black screens
/// often contain a few larger packets (a keyframe, a flicker of noise).
const MAX_GAP_INSIDE_STRETCH_SECONDS: f64 = 0.5;

/// A stretch must be tiny packets for at least one frame in this many.
///
/// A video converted from 24 to 30 pictures a second repeats every fifth
/// picture, and each repeat is a tiny packet. Measured on such a film, the
/// repeats alone chained into one stretch over the whole two hours. A real
/// black screen is tiny packets almost throughout.
const MIN_TINY_SHARE_DIVISOR: usize = 2;

/// Extra footage decoded on each side of a suspicious stretch, so the exact
/// first and last black frames are found even when they are not tiny.
const PADDING_SECONDS: f64 = 0.5;

/// The pixels one thread may decode so that a scan can look at every frame
/// without being asked to.
///
/// Measured on a 16 thread desktop: 67 minutes of 640x360 H.264, 22 billion
/// pixels, were decoded in full in about 2.5 seconds. Three billion pixels a
/// thread is then about five seconds of decoding.
const CHEAP_PIXELS_PER_THREAD: u64 = 3_000_000_000;

/// How many pixels a scan may decode to look at every frame when the setting
/// only asks for the suspicious stretches.
///
/// Packet sizes miss a black screen that does not compress to almost
/// nothing: one with a logo in a corner, or with grain. In a low bitrate
/// video such a frame is only a few times smaller than an ordinary one.
/// Where decoding everything costs a few seconds, that risk buys nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FullDecodeBudget {
    pub pixels: u64,
}

impl FullDecodeBudget {
    /// The budget that keeps a full decode to a few seconds on a machine
    /// that decodes on `thread_count` threads.
    pub fn for_threads(thread_count: usize) -> Self {
        Self {
            pixels: CHEAP_PIXELS_PER_THREAD.saturating_mul(thread_count as u64),
        }
    }

    /// Whether decoding every one of `frame_count` frames stays within it.
    pub fn covers(self, frame_count: usize, pixels_per_frame: u64) -> bool {
        (frame_count as u64).saturating_mul(pixels_per_frame) <= self.pixels
    }
}

/// Windows around stretches of unusually small packets.
///
/// Still footage (title cards, frozen frames) also produces small packets.
/// Those windows cost a little decoding time and are then rejected.
pub fn suspicious_windows(packets: &[VideoPacket], min_black_seconds: f64) -> Vec<FrameWindow> {
    let tiny_limit = (median_packet_size(packets) / TINY_PACKET_DIVISOR).max(ALWAYS_TINY_BYTES);
    let tiny_runs = runs_of_tiny_packets(packets, tiny_limit);
    let tiny_before = tiny_packets_before_each_frame(packets, tiny_limit);

    let stretches = merge_windows(tiny_runs, |earlier, later| {
        let gap_seconds = packets[later.start].time - packets[earlier.end - 1].time;
        let joined = earlier.start..later.end;
        let tiny_packets = tiny_before[joined.end] - tiny_before[joined.start];
        gap_seconds <= MAX_GAP_INSIDE_STRETCH_SECONDS
            && tiny_packets * MIN_TINY_SHARE_DIVISOR >= joined.len()
    });

    // Half the shortest black screen, because the frames at either edge of one
    // can be larger transition frames that the tiny run does not cover.
    let padded = stretches
        .into_iter()
        .filter(|stretch| window_seconds(packets, stretch) >= min_black_seconds / 2.0)
        .map(|stretch| pad_window(packets, stretch))
        .collect();

    merge_windows(padded, |earlier, later| later.start <= earlier.end)
}

/// Windows that together cover every frame, for videos whose black screens
/// are too noisy to compress into tiny packets.
pub fn whole_video_windows(packets: &[VideoPacket], window_count: usize) -> Vec<FrameWindow> {
    let frames_per_window = packets.len().div_ceil(window_count.max(1)).max(1);
    let mut windows = Vec::new();
    let mut start = 0;
    while start < packets.len() {
        let target_end = (start + frames_per_window).min(packets.len());
        // Ending on a keyframe lets the next window start decoding at once
        // instead of first decoding its way up from an earlier keyframe.
        let end = packets[target_end..]
            .iter()
            .position(|packet| packet.is_keyframe)
            .map_or(packets.len(), |offset| target_end + offset);
        windows.push(start..end);
        start = end;
    }
    windows
}

fn median_packet_size(packets: &[VideoPacket]) -> u32 {
    let mut sizes: Vec<u32> = packets.iter().map(|packet| packet.size_bytes).collect();
    if sizes.is_empty() {
        return 0;
    }
    let middle = sizes.len() / 2;
    *sizes.select_nth_unstable(middle).1
}

/// How many tiny packets come before each frame, with one more entry for the
/// end of the video, so that a subtraction counts those in any window.
fn tiny_packets_before_each_frame(packets: &[VideoPacket], tiny_limit: u32) -> Vec<usize> {
    let mut tiny_so_far = 0;
    let mut tiny_before = Vec::with_capacity(packets.len() + 1);
    tiny_before.push(tiny_so_far);
    for packet in packets {
        if packet.size_bytes <= tiny_limit {
            tiny_so_far += 1;
        }
        tiny_before.push(tiny_so_far);
    }
    tiny_before
}

fn runs_of_tiny_packets(packets: &[VideoPacket], tiny_limit: u32) -> Vec<FrameWindow> {
    let mut runs = Vec::new();
    let mut run_start = None;
    for (frame, packet) in packets.iter().enumerate() {
        let is_tiny = packet.size_bytes <= tiny_limit;
        match (is_tiny, run_start) {
            (true, None) => run_start = Some(frame),
            (false, Some(start)) => {
                runs.push(start..frame);
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(start) = run_start {
        runs.push(start..packets.len());
    }
    runs
}

/// Joins neighbouring windows wherever `should_join(earlier, later)` holds.
/// Expects the windows in ascending order.
fn merge_windows(
    windows: Vec<FrameWindow>,
    should_join: impl Fn(&FrameWindow, &FrameWindow) -> bool,
) -> Vec<FrameWindow> {
    let mut merged: Vec<FrameWindow> = Vec::new();
    for window in windows {
        match merged.last_mut() {
            Some(earlier) if should_join(earlier, &window) => {
                earlier.end = earlier.end.max(window.end)
            }
            _ => merged.push(window),
        }
    }
    merged
}

fn window_seconds(packets: &[VideoPacket], window: &FrameWindow) -> f64 {
    packets[window.end - 1].time - packets[window.start].time
}

fn pad_window(packets: &[VideoPacket], window: FrameWindow) -> FrameWindow {
    let earliest = packets[window.start].time - PADDING_SECONDS;
    let latest = packets[window.end - 1].time + PADDING_SECONDS;
    let start = packets[..window.start].partition_point(|packet| packet.time < earliest);
    let end = window.end + packets[window.end..].partition_point(|packet| packet.time <= latest);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    // A power of two, so frame times are exact and the padding maths has no rounding.
    const FRAME_SECONDS: f64 = 0.125;

    /// Eight frames per second, a keyframe every ten frames.
    fn packets_with_sizes(sizes: &[u32]) -> Vec<VideoPacket> {
        sizes
            .iter()
            .enumerate()
            .map(|(frame, &size_bytes)| VideoPacket {
                time: frame as f64 * FRAME_SECONDS,
                size_bytes,
                is_keyframe: frame % 10 == 0,
            })
            .collect()
    }

    fn footage_with_tiny_frames(frame_count: usize, tiny_frames: &[usize]) -> Vec<VideoPacket> {
        let mut sizes = vec![4000; frame_count];
        for &frame in tiny_frames {
            sizes[frame] = 25;
        }
        packets_with_sizes(&sizes)
    }

    #[test]
    fn normal_footage_has_no_suspicious_windows() {
        let packets = footage_with_tiny_frames(100, &[]);
        assert!(suspicious_windows(&packets, 0.3).is_empty());
    }

    #[test]
    fn a_tiny_run_becomes_a_padded_window() {
        let tiny: Vec<usize> = (40..50).collect();
        let packets = footage_with_tiny_frames(100, &tiny);
        // Half a second is four frames of padding on each side.
        assert_eq!(suspicious_windows(&packets, 0.3), vec![36..54]);
    }

    #[test]
    fn a_larger_packet_inside_a_black_screen_does_not_split_it() {
        let tiny: Vec<usize> = (40..60).filter(|&frame| frame != 50).collect();
        let packets = footage_with_tiny_frames(100, &tiny);
        assert_eq!(suspicious_windows(&packets, 0.3), vec![36..64]);
    }

    #[test]
    fn a_single_tiny_frame_is_too_short_to_matter() {
        let packets = footage_with_tiny_frames(100, &[40]);
        assert!(suspicious_windows(&packets, 0.3).is_empty());
    }

    #[test]
    fn padding_stops_at_the_ends_of_the_video() {
        let tiny: Vec<usize> = (0..10).chain(95..100).collect();
        let packets = footage_with_tiny_frames(100, &tiny);
        assert_eq!(suspicious_windows(&packets, 0.3), vec![0..14, 91..100]);
    }

    #[test]
    fn whole_video_windows_cover_every_frame_once_and_start_on_keyframes() {
        let packets = footage_with_tiny_frames(95, &[]);
        let windows = whole_video_windows(&packets, 4);

        assert_eq!(windows.first().unwrap().start, 0);
        assert_eq!(windows.last().unwrap().end, packets.len());
        for pair in windows.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        for window in &windows {
            assert!(packets[window.start].is_keyframe);
        }
    }

    const NORMAL_BYTES: u32 = 4000;
    const BLACK_BYTES: u32 = 25;
    const MIN_BLACK_SECONDS: f64 = 0.3;
    /// Half of it is one second, which is eight frame intervals.
    const LONG_MIN_BLACK_SECONDS: f64 = 2.0;

    fn without_keyframes(mut packets: Vec<VideoPacket>) -> Vec<VideoPacket> {
        for packet in &mut packets {
            packet.is_keyframe = false;
        }
        packets
    }

    const PIXELS_OF_360P: u64 = 640 * 360;
    const PIXELS_OF_1080P: u64 = 1920 * 1080;
    const FRAMES_IN_AN_HOUR: usize = 24 * 3600;

    #[test]
    fn an_hour_of_360p_is_cheap_enough_to_decode_in_full_on_a_few_threads() {
        let budget = FullDecodeBudget::for_threads(8);
        assert!(budget.covers(FRAMES_IN_AN_HOUR, PIXELS_OF_360P));
    }

    #[test]
    fn an_hour_of_1080p_is_left_to_the_packet_sizes() {
        let budget = FullDecodeBudget::for_threads(8);
        assert!(!budget.covers(FRAMES_IN_AN_HOUR, PIXELS_OF_1080P));
    }

    #[test]
    fn more_threads_afford_a_longer_video() {
        let two_hours = 2 * FRAMES_IN_AN_HOUR;
        assert!(!FullDecodeBudget::for_threads(4).covers(two_hours, PIXELS_OF_360P));
        assert!(FullDecodeBudget::for_threads(16).covers(two_hours, PIXELS_OF_360P));
    }

    #[test]
    fn a_video_exactly_as_large_as_the_budget_is_covered() {
        let budget = FullDecodeBudget { pixels: 1_000 };
        assert!(budget.covers(10, 100));
        assert!(!budget.covers(11, 100));
    }

    #[test]
    fn a_video_too_large_to_count_is_not_covered() {
        let nearly_everything = FullDecodeBudget {
            pixels: u64::MAX - 1,
        };
        assert!(!nearly_everything.covers(usize::MAX, u64::MAX));
    }

    #[test]
    fn a_video_without_packets_has_no_suspicious_windows() {
        assert!(suspicious_windows(&[], MIN_BLACK_SECONDS).is_empty());
    }

    #[test]
    fn a_packet_exactly_at_the_tiny_limit_is_suspicious() {
        let mut sizes = vec![NORMAL_BYTES; 100];
        sizes[40..50].fill(NORMAL_BYTES / TINY_PACKET_DIVISOR);
        let packets = packets_with_sizes(&sizes);
        assert_eq!(
            suspicious_windows(&packets, MIN_BLACK_SECONDS),
            vec![36..54]
        );
    }

    #[test]
    fn a_video_that_is_black_throughout_is_decoded_in_full() {
        // Next to its own median nothing stands out, but packets this small
        // are suspicious on their own.
        let packets = packets_with_sizes(&[BLACK_BYTES; 100]);
        assert_eq!(
            suspicious_windows(&packets, MIN_BLACK_SECONDS),
            vec![0..100]
        );
    }

    #[test]
    fn a_tiny_run_lasting_exactly_half_the_minimum_is_kept() {
        // Three frames span two frame intervals, a quarter of a second.
        const TWICE_THE_RUN_SECONDS: f64 = 0.5;
        let packets = footage_with_tiny_frames(100, &[40, 41, 42]);
        assert_eq!(
            suspicious_windows(&packets, TWICE_THE_RUN_SECONDS),
            vec![36..47]
        );
    }

    #[test]
    fn tiny_runs_exactly_the_maximum_gap_apart_count_as_one_stretch() {
        // Frames 43 and 47 are half a second apart. Each run alone lasts
        // 0.375 seconds, too short to keep. Joined they last 1.25 seconds.
        let tiny: Vec<usize> = (40..44).chain(47..51).collect();
        let packets = footage_with_tiny_frames(100, &tiny);
        assert_eq!(
            suspicious_windows(&packets, LONG_MIN_BLACK_SECONDS),
            vec![36..55]
        );
    }

    #[test]
    fn tiny_runs_just_over_the_maximum_gap_apart_are_judged_separately() {
        // Frames 43 and 48 are 0.625 seconds apart, so neither short run
        // gets the other's length added to its own.
        let tiny: Vec<usize> = (40..44).chain(48..52).collect();
        let packets = footage_with_tiny_frames(100, &tiny);
        assert!(suspicious_windows(&packets, LONG_MIN_BLACK_SECONDS).is_empty());
    }

    #[test]
    fn a_repeated_picture_every_few_frames_is_not_a_stretch() {
        // Every fifth packet is tiny, as in a film converted from 24 to 30
        // pictures a second. No two of them are half a second apart.
        let repeats: Vec<usize> = (0..400).step_by(5).collect();
        let packets = footage_with_tiny_frames(400, &repeats);
        assert!(suspicious_windows(&packets, MIN_BLACK_SECONDS).is_empty());
    }

    #[test]
    fn a_black_screen_among_repeated_pictures_is_found_on_its_own() {
        let repeats = (0..400).step_by(5);
        let tiny: Vec<usize> = repeats.chain(200..220).collect();
        let packets = footage_with_tiny_frames(400, &tiny);
        let windows = suspicious_windows(&packets, MIN_BLACK_SECONDS);

        assert_eq!(windows.len(), 1, "{windows:?}");
        assert!(
            windows[0].start <= 196 && windows[0].end >= 224,
            "{windows:?}"
        );
        // The repeats after it are taken along only while tiny packets
        // still make up half the stretch.
        assert!(windows[0].len() < 80, "{windows:?}");
    }

    #[test]
    fn windows_that_touch_after_padding_become_one() {
        // The runs are too far apart to be one stretch, but four frames of
        // padding on each side make 16..34 and 34..52.
        let tiny: Vec<usize> = (20..30).chain(38..48).collect();
        let packets = footage_with_tiny_frames(100, &tiny);
        assert_eq!(
            suspicious_windows(&packets, MIN_BLACK_SECONDS),
            vec![16..52]
        );
    }

    #[test]
    fn windows_one_frame_apart_after_padding_stay_separate() {
        let tiny: Vec<usize> = (20..30).chain(39..49).collect();
        let packets = footage_with_tiny_frames(100, &tiny);
        assert_eq!(
            suspicious_windows(&packets, MIN_BLACK_SECONDS),
            vec![16..34, 35..53]
        );
    }

    #[test]
    fn the_median_of_an_even_count_is_the_upper_of_the_two_middle_sizes() {
        let packets = packets_with_sizes(&[40, 10, 30, 20]);
        assert_eq!(median_packet_size(&packets), 30);
    }

    #[test]
    fn a_video_without_packets_has_no_whole_video_windows() {
        assert!(whole_video_windows(&[], 4).is_empty());
    }

    #[test]
    fn asking_for_zero_windows_gives_one_window_over_everything() {
        let packets = footage_with_tiny_frames(95, &[]);
        assert_eq!(whole_video_windows(&packets, 0), vec![0..95]);
    }

    #[test]
    fn a_window_runs_on_to_the_next_keyframe() {
        let packets = footage_with_tiny_frames(30, &[]);
        // Half the frames would end the first window at frame 15, between
        // the keyframes at 10 and 20.
        assert_eq!(whole_video_windows(&packets, 2), vec![0..20, 20..30]);
    }

    #[test]
    fn a_video_without_keyframes_is_one_window() {
        let packets = without_keyframes(footage_with_tiny_frames(95, &[]));
        assert_eq!(whole_video_windows(&packets, 4), vec![0..95]);
    }

    #[test]
    fn asking_for_more_windows_than_keyframes_gives_one_window_per_keyframe() {
        let packets = footage_with_tiny_frames(25, &[]);
        assert_eq!(
            whole_video_windows(&packets, 100),
            vec![0..10, 10..20, 20..25]
        );
    }
}
