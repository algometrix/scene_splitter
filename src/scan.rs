//! Decodes stretches of a video into tiny grey thumbnails with ffmpeg, and
//! marks which frames are black.

use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use crate::black::BlackCriteria;
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

/// Returns one flag per frame of the video. Frames outside every window were
/// not decoded and are reported as not black.
pub fn find_black_frames(
    input: &Path,
    packets: &[VideoPacket],
    windows: &[FrameWindow],
    criteria: &BlackCriteria,
) -> Result<Vec<bool>> {
    // One single-threaded ffmpeg per window, with as many running at once as
    // there are cores. That scales better than ffmpeg's own frame threading.
    let flags_per_window = windows
        .par_iter()
        .map(|window| {
            let mut flags = Vec::with_capacity(window.len());
            decode_thumbnails(input, packets, window, |thumbnail| {
                flags.push(criteria.is_black_frame(thumbnail));
            })?;
            Ok(flags)
        })
        .collect::<Result<Vec<_>>>()?;

    let mut frame_is_black = vec![false; packets.len()];
    for (window, flags) in windows.iter().zip(flags_per_window) {
        frame_is_black[window.start..window.start + flags.len()].copy_from_slice(&flags);
    }
    Ok(frame_is_black)
}

/// Decodes the frames of one window and hands each thumbnail, as grey pixel
/// rows, to `on_thumbnail` in order.
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
) -> Result<()> {
    let window_start = packets[window.start].time;
    let seek_seconds = (window_start - SEEK_MARGIN_SECONDS).max(0.0);
    let thumbnail_filter =
        format!("scale={THUMBNAIL_WIDTH}:{THUMBNAIL_HEIGHT}:flags=fast_bilinear,format=gray");

    let mut ffmpeg = tools::ffmpeg_command()
        .args(["-nostdin", "-v", "error", "-threads", "1"])
        .args(["-ss", &format!("{seek_seconds:.6}")])
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0", "-an", "-sn"])
        .args(["-frames:v", &window.len().to_string()])
        .args(["-vf", &thumbnail_filter])
        .args(["-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // ffmpeg's own error messages go straight to the user's terminal.
        .stderr(Stdio::inherit())
        .spawn()
        .context("could not start ffmpeg, is it on PATH?")?;
    let mut thumbnails = ffmpeg.stdout.take().context("ffmpeg has no output pipe")?;

    let mut thumbnail = [0u8; THUMBNAIL_BYTES];
    let mut decoded_frames = 0;
    while decoded_frames < window.len() {
        match thumbnails.read_exact(&mut thumbnail) {
            Ok(()) => on_thumbnail(&thumbnail),
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error).context("reading decoded frames from ffmpeg"),
        }
        decoded_frames += 1;
    }
    drop(thumbnails);

    let missing_frames = window.len() - decoded_frames;
    if missing_frames > TOLERATED_MISSING_FRAMES {
        // Not fatal, the rest of the video is still worth scanning. But the
        // person should know that this stretch was not looked at.
        eprintln!(
            "Warning: {missing_frames} of {} frames from {window_start:.3}s in {} could not be decoded. \
             Scene changes there may be missed.",
            window.len(),
            input.display()
        );
    }

    let status = ffmpeg.wait().context("waiting for ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg could not decode the frames at {window_start:.3}s ({status})");
    }
    Ok(())
}
