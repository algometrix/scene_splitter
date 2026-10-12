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

/// Tiny runs this close together are treated as one stretch. Black screens
/// often contain a few larger packets (a keyframe, a flicker of noise).
const MAX_GAP_INSIDE_STRETCH_SECONDS: f64 = 0.5;

/// Extra footage decoded on each side of a suspicious stretch, so the exact
/// first and last black frames are found even when they are not tiny.
const PADDING_SECONDS: f64 = 0.5;

/// Windows around stretches of unusually small packets.
///
/// Still footage (title cards, frozen frames) also produces small packets.
/// Those windows cost a little decoding time and are then rejected.
pub fn suspicious_windows(packets: &[VideoPacket], min_black_seconds: f64) -> Vec<FrameWindow> {
    let tiny_limit = median_packet_size(packets) / TINY_PACKET_DIVISOR;
    let tiny_runs = runs_of_tiny_packets(packets, tiny_limit);

    let stretches = merge_windows(tiny_runs, |earlier, later| {
        packets[later.start].time - packets[earlier.end - 1].time <= MAX_GAP_INSIDE_STRETCH_SECONDS
    });

    // Half the duration, because the frames at either edge of a black screen
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
}
