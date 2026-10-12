//! Decodes stretches of a video into tiny grey thumbnails with ffmpeg, and
//! marks which frames are black.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::black::BlackCriteria;
use crate::ffmpeg::{Program, STEADY_OUTPUT_LIMIT};
use crate::probe::VideoPacket;
use crate::tools;
use crate::windows::FrameWindow;

/// Frames are judged on a thumbnail. Black is black at any size, and a small
/// frame keeps the pipe between ffmpeg and this program cheap.
const THUMBNAIL_WIDTH: usize = 64;
const THUMBNAIL_HEIGHT: usize = 36;
pub const THUMBNAIL_BYTES: usize = THUMBNAIL_WIDTH * THUMBNAIL_HEIGHT;

/// A window may come up this many frames short without comment, since a
/// decoder does not always output the last packets of a file.
const TOLERATED_MISSING_FRAMES: usize = 3;

/// Seeking slightly before a frame's timestamp makes sure that frame is the
/// first one ffmpeg returns, whatever rounding the container applies.
const SEEK_MARGIN_SECONDS: f64 = 0.001;

/// Progress is reported in steps of half a percent: fine enough for a
/// smooth progress bar, and a few hundred calls instead of one per frame.
const PROGRESS_STEPS: usize = 200;

/// Receives how far a scan has come, from 0.0 to 1.0.
///
/// A scan decodes several stretches of the video at once and reports from
/// whichever thread reached the next step, so the receiver must be callable
/// from any thread, and two reports close together can arrive out of order.
pub type OnProgress<'a> = &'a (dyn Fn(f32) + Sync);

/// Counts decoded frames across the threads of one scan and reports each new
/// step of progress.
pub struct FrameProgress<'a> {
    total_frames: usize,
    decoded_frames: AtomicUsize,
    on_progress: OnProgress<'a>,
}

impl<'a> FrameProgress<'a> {
    pub fn new(total_frames: usize, on_progress: OnProgress<'a>) -> Self {
        Self {
            total_frames,
            decoded_frames: AtomicUsize::new(0),
            on_progress,
        }
    }

    pub fn frame_decoded(&self) {
        let decoded_frames = self.decoded_frames.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(fraction) = newly_reached_fraction(decoded_frames, self.total_frames) {
            (self.on_progress)(fraction);
        }
    }

    /// Reports the end of the scan. A decoder can come up a few frames
    /// short, and a scan with nothing to decode counts no frames at all, so
    /// counting alone does not always arrive at the end.
    pub fn finish(&self) {
        if self.decoded_frames.load(Ordering::Relaxed) < self.total_frames || self.total_frames == 0
        {
            (self.on_progress)(1.0);
        }
    }
}

/// The fraction to report now that `decoded_frames` are done, or `None` when
/// this frame did not reach a new step.
fn newly_reached_fraction(decoded_frames: usize, total_frames: usize) -> Option<f32> {
    if total_frames == 0 || decoded_frames == 0 || decoded_frames > total_frames {
        return None;
    }
    let step_after = |frames: usize| frames * PROGRESS_STEPS / total_frames;
    let step = step_after(decoded_frames);
    if step == step_after(decoded_frames - 1) {
        return None;
    }
    Some(step as f32 / PROGRESS_STEPS as f32)
}

/// A stretch of the video that could not be fully decoded, so it was not
/// fully looked at. Not fatal: the rest of the video is still worth
/// scanning. But the person should know.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UndecodedFrames {
    /// Where the stretch begins, in seconds from the start of the video.
    pub from_seconds: f64,
    pub missing_frames: usize,
    pub stretch_frames: usize,
}

impl fmt::Display for UndecodedFrames {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} of {} frames from {:.3}s could not be decoded. Scene changes there may be missed.",
            self.missing_frames, self.stretch_frames, self.from_seconds
        )
    }
}

/// What the search for black frames found.
#[derive(Debug, PartialEq)]
pub struct BlackFrameScan {
    /// One flag per frame of the video. Frames outside every window were not
    /// decoded and are reported as not black.
    pub frame_is_black: Vec<bool>,
    pub undecoded: Vec<UndecodedFrames>,
}

/// Decodes the frames of every window and marks the black ones, reporting
/// progress as the share of those frames decoded so far.
pub fn find_black_frames(
    input: &Path,
    packets: &[VideoPacket],
    windows: &[FrameWindow],
    criteria: &BlackCriteria,
    on_progress: OnProgress,
) -> Result<BlackFrameScan> {
    let progress = FrameProgress::new(windows.iter().map(FrameWindow::len).sum(), on_progress);
    // One single-threaded ffmpeg per window, with as many running at once as
    // there are cores. That scales better than ffmpeg's own frame threading.
    let decoded_windows = windows
        .par_iter()
        .map(|window| {
            let mut flags = Vec::with_capacity(window.len());
            let undecoded = decode_thumbnails(input, packets, window, |thumbnail| {
                flags.push(criteria.is_black_frame(thumbnail));
                progress.frame_decoded();
            })?;
            Ok((flags, undecoded))
        })
        .collect::<Result<Vec<_>>>()?;
    progress.finish();

    let mut frame_is_black = vec![false; packets.len()];
    let mut undecoded = Vec::new();
    for (window, (flags, undecoded_here)) in windows.iter().zip(decoded_windows) {
        frame_is_black[window.start..window.start + flags.len()].copy_from_slice(&flags);
        undecoded.extend(undecoded_here);
    }
    Ok(BlackFrameScan {
        frame_is_black,
        undecoded,
    })
}

/// Decodes the frames of one window and hands each thumbnail, as grey pixel
/// rows, to `on_thumbnail` in order. Returns what could not be decoded, if
/// that is more than a decoder normally leaves out.
///
/// The nth thumbnail is the nth frame of the window, because ffmpeg returns
/// frames in presentation order starting at the seek point. A window that
/// runs to the end of the file can come up a frame or two short, since the
/// decoder may not output every packet.
pub fn decode_thumbnails(
    input: &Path,
    packets: &[VideoPacket],
    window: &FrameWindow,
    mut on_thumbnail: impl FnMut(&[u8]),
) -> Result<Option<UndecodedFrames>> {
    let window_start = packets[window.start].time;
    let seek_seconds = (window_start - SEEK_MARGIN_SECONDS).max(0.0);
    let thumbnail_filter =
        format!("scale={THUMBNAIL_WIDTH}:{THUMBNAIL_HEIGHT}:flags=fast_bilinear,format=gray");

    let mut decoded_frames = 0;
    // Decoding takes as long as the window is, and a frame arrives many
    // times a second all the while.
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-threads", "1"])
        .args(["-ss", &format!("{seek_seconds:.6}")])
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0", "-an", "-sn"])
        .args(["-frames:v", &window.len().to_string()])
        .args(["-vf", &thumbnail_filter])
        .args(["-f", "rawvideo", "-"])
        .stream_records(THUMBNAIL_BYTES, STEADY_OUTPUT_LIMIT, |thumbnail| {
            // ffmpeg was asked for exactly the window. The count is checked
            // anyway, because a frame too many would be taken for a frame of
            // the next window.
            if decoded_frames < window.len() {
                on_thumbnail(thumbnail);
                decoded_frames += 1;
            }
        })
        .with_context(|| format!("could not decode the frames at {window_start:.3}s"))?;

    Ok(undecoded_frames(window_start, window.len(), decoded_frames))
}

fn undecoded_frames(
    from_seconds: f64,
    stretch_frames: usize,
    decoded_frames: usize,
) -> Option<UndecodedFrames> {
    let missing_frames = stretch_frames.saturating_sub(decoded_frames);
    (missing_frames > TOLERATED_MISSING_FRAMES).then_some(UndecodedFrames {
        from_seconds,
        missing_frames,
        stretch_frames,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Runs a scan of `total_frames` in which `decoded_frames` arrive, and
    /// returns every fraction it reported.
    fn reported(total_frames: usize, decoded_frames: usize) -> Vec<f32> {
        let fractions = Mutex::new(Vec::new());
        let record = |fraction: f32| fractions.lock().unwrap().push(fraction);
        let progress = FrameProgress::new(total_frames, &record);
        for _ in 0..decoded_frames {
            progress.frame_decoded();
        }
        progress.finish();
        fractions.into_inner().unwrap()
    }

    #[test]
    fn a_short_scan_reports_every_frame_and_ends_at_one() {
        assert_eq!(reported(4, 4), vec![0.25, 0.5, 0.75, 1.0]);
    }

    #[test]
    fn a_long_scan_reports_each_step_once() {
        const FRAMES: usize = 100_000;
        let fractions = reported(FRAMES, FRAMES);

        assert_eq!(fractions.len(), PROGRESS_STEPS);
        assert!(fractions.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(fractions.first(), Some(&(1.0 / PROGRESS_STEPS as f32)));
        assert_eq!(fractions.last(), Some(&1.0));
    }

    #[test]
    fn a_scan_that_comes_up_short_still_ends_at_one() {
        assert_eq!(reported(4, 3), vec![0.25, 0.5, 0.75, 1.0]);
    }

    #[test]
    fn a_scan_with_nothing_to_decode_is_done_at_once() {
        assert_eq!(reported(0, 0), vec![1.0]);
    }

    #[test]
    fn frames_beyond_the_total_report_nothing() {
        assert_eq!(newly_reached_fraction(5, 4), None);
        assert_eq!(newly_reached_fraction(1, 0), None);
    }

    #[test]
    fn a_frame_inside_a_step_reports_nothing() {
        const FRAMES: usize = 100_000;
        let frames_per_step = FRAMES / PROGRESS_STEPS;
        assert_eq!(newly_reached_fraction(frames_per_step - 1, FRAMES), None);
        assert_eq!(
            newly_reached_fraction(frames_per_step, FRAMES),
            Some(1.0 / PROGRESS_STEPS as f32)
        );
        assert_eq!(newly_reached_fraction(frames_per_step + 1, FRAMES), None);
    }

    #[test]
    fn a_few_missing_frames_at_the_end_are_normal() {
        assert_eq!(undecoded_frames(10.0, 100, 100), None);
        assert_eq!(
            undecoded_frames(10.0, 100, 100 - TOLERATED_MISSING_FRAMES),
            None
        );
    }

    #[test]
    fn more_missing_frames_are_reported_with_where_they_are() {
        let undecoded = undecoded_frames(12.5, 100, 60).unwrap();
        assert_eq!(
            undecoded,
            UndecodedFrames {
                from_seconds: 12.5,
                missing_frames: 40,
                stretch_frames: 100,
            }
        );
        assert_eq!(
            undecoded.to_string(),
            "40 of 100 frames from 12.500s could not be decoded. \
             Scene changes there may be missed."
        );
    }
}
