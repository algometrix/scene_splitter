//! Grabs small preview pictures of the video with ffmpeg.

use std::path::Path;
use std::process::Stdio;

use anyhow::{bail, Context, Result};

use crate::black::TimeSpan;
use crate::tools;

pub const THUMBNAIL_WIDTH: usize = 320;
pub const THUMBNAIL_HEIGHT: usize = 180;
const BYTES_PER_RGB_PIXEL: usize = 3;

pub const PREVIEWS_PER_SEGMENT: usize = 4;

/// Scenes cut on a black screen begin or end with a moment of black, so the
/// first and last previews are taken this far inside the segment.
const PREVIEW_EDGE_MARGIN_SECONDS: f64 = 2.5;

/// A short segment cannot afford the full margin, it gets this share instead.
const MAX_EDGE_MARGIN_SHARE: f64 = 0.125;

/// One picture, as tightly packed RGB rows of the thumbnail size.
pub struct Thumbnail {
    pub rgb: Vec<u8>,
}

/// The moments of a segment to preview, evenly spread from just after its
/// start to just before its end.
pub fn preview_times(segment: TimeSpan) -> [f64; PREVIEWS_PER_SEGMENT] {
    let margin = PREVIEW_EDGE_MARGIN_SECONDS.min(segment.seconds() * MAX_EDGE_MARGIN_SHARE);
    let first = segment.start + margin;
    let last = segment.end - margin;
    let step = (last - first) / (PREVIEWS_PER_SEGMENT - 1) as f64;
    std::array::from_fn(|preview| first + step * preview as f64)
}

/// Decodes the frame shown at `time`, letterboxed to the thumbnail size.
///
/// A grab is tried twice. Once, while a dozen grabs and a scan started in the
/// same instant, ffmpeg returned nothing for three of them and the same
/// command succeeded when run again. Grabbing only reads, so repeating it is
/// harmless.
pub fn grab_thumbnail(input: &Path, time: f64) -> Result<Thumbnail> {
    grab_thumbnail_once(input, time).or_else(|_first_failure| grab_thumbnail_once(input, time))
}

fn grab_thumbnail_once(input: &Path, time: f64) -> Result<Thumbnail> {
    let letterbox_filter = format!(
        "scale={THUMBNAIL_WIDTH}:{THUMBNAIL_HEIGHT}:force_original_aspect_ratio=decrease,\
         pad={THUMBNAIL_WIDTH}:{THUMBNAIL_HEIGHT}:(ow-iw)/2:(oh-ih)/2"
    );
    let output = tools::ffmpeg_command()
        .args(["-nostdin", "-v", "error", "-threads", "1"])
        .args(["-ss", &format!("{time:.6}")])
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0", "-frames:v", "1"])
        .args(["-vf", &letterbox_filter])
        .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .output()
        .context("could not start ffmpeg, is it on PATH?")?;

    let expected_bytes = THUMBNAIL_WIDTH * THUMBNAIL_HEIGHT * BYTES_PER_RGB_PIXEL;
    if !output.status.success() || output.stdout.len() != expected_bytes {
        bail!(
            "ffmpeg returned {} bytes instead of a {expected_bytes} byte picture at {time:.3}s ({}): {}",
            output.stdout.len(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(Thumbnail { rgb: output.stdout })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_of_a_long_segment_skip_the_black_edges() {
        let times = preview_times(TimeSpan {
            start: 100.0,
            end: 400.0,
        });
        assert_eq!(times[0], 102.5);
        assert_eq!(times[PREVIEWS_PER_SEGMENT - 1], 397.5);
        assert!(times.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn previews_of_a_short_segment_stay_inside_it() {
        let times = preview_times(TimeSpan {
            start: 10.0,
            end: 14.0,
        });
        assert_eq!(times, [10.5, 11.5, 12.5, 13.5]);
    }
}
