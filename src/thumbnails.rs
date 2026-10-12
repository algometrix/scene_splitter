//! Grabs small preview pictures of the video with ffmpeg.

use std::ops::RangeInclusive;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::ffmpeg::{FfmpegError, NotUnderstood, Program, TimeLimit};
use crate::timecode::TimeSpan;
use crate::tools;

pub const THUMBNAIL_WIDTH: usize = 320;
pub const THUMBNAIL_HEIGHT: usize = 180;
/// The size of a preview on a screen of ordinary sharpness.
pub const SMALLEST_THUMBNAIL: PictureSize = PictureSize {
    width: THUMBNAIL_WIDTH,
    height: THUMBNAIL_HEIGHT,
};
/// A preview is never decoded wider than this, however sharp the screen. A
/// larger one would take ffmpeg longer to make and megabytes to keep, for a
/// picture that is only there to recognise a scene by.
pub const LARGEST_THUMBNAIL_WIDTH: usize = 960;
const THUMBNAIL_WIDTHS: RangeInclusive<usize> = THUMBNAIL_WIDTH..=LARGEST_THUMBNAIL_WIDTH;
const BYTES_PER_RGB_PIXEL: usize = 3;
/// Pictures are sixteen wide for every nine high. Widths go up in steps of
/// sixteen pixels, so the height is always a whole number of pixels.
const WIDESCREEN_WIDTH_UNITS: usize = 16;
const WIDESCREEN_HEIGHT_UNITS: usize = 9;

/// Getting one picture means jumping to the nearest earlier frame a decoder
/// can start on and decoding forward from there, usually a few dozen frames
/// and a fraction of a second. A minute covers videos where such frames are
/// very far apart, on a slow drive, with a dozen grabs running at once.
const SINGLE_PICTURE_LIMIT: TimeLimit = TimeLimit::WholeRun(Duration::from_secs(60));

pub const PREVIEWS_PER_SEGMENT: usize = 4;

/// Scenes cut on a black screen begin or end with a moment of black, so the
/// first and last previews are taken this far inside the segment.
const PREVIEW_EDGE_MARGIN_SECONDS: f64 = 2.5;

/// A short segment cannot afford the full margin, it gets this share instead.
const MAX_EDGE_MARGIN_SHARE: f64 = 0.125;

/// One picture, as tightly packed RGB rows of `size`.
pub struct Thumbnail {
    pub size: PictureSize,
    pub rgb: Vec<u8>,
}

/// How large to decode a preview that is drawn at most `widest_drawn_points`
/// wide, so that it is sharp on a screen that puts `pixels_per_point` pixels
/// in every point.
pub fn thumbnail_size_for(widest_drawn_points: f32, pixels_per_point: f32) -> PictureSize {
    pixel_size_for(widest_drawn_points, pixels_per_point, THUMBNAIL_WIDTHS)
}

/// The widescreen size, in physical pixels, of a picture drawn `points_wide`
/// wide. Sizes in the window are in points, and a sharp screen has more than
/// one pixel to the point, so a picture decoded at its size in points looks
/// soft there. The width stays within `widths`.
pub fn pixel_size_for(
    points_wide: f32,
    pixels_per_point: f32,
    widths: RangeInclusive<usize>,
) -> PictureSize {
    let pixels_wide = (points_wide * pixels_per_point).ceil();
    // A window that is still opening can report a scale of zero or nonsense.
    let pixels_wide = if pixels_wide.is_finite() && pixels_wide > 0.0 {
        pixels_wide as usize
    } else {
        0
    };
    let width = pixels_wide
        .min(*widths.end())
        .next_multiple_of(WIDESCREEN_WIDTH_UNITS)
        .clamp(*widths.start(), *widths.end());
    PictureSize::widescreen(width)
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

/// Decodes the frame shown at `time`, letterboxed to `size`.
///
/// A grab is tried twice. Once, while a dozen grabs and a scan started in the
/// same instant, ffmpeg returned nothing for three of them and the same
/// command succeeded when run again. Grabbing only reads, so repeating it is
/// harmless. A grab that could not start or ran out of time is not repeated,
/// since the second try would end the same way.
pub fn grab_thumbnail(input: &Path, time: f64, size: PictureSize) -> Result<Thumbnail> {
    // The window decodes many previews at once, so each uses one core.
    let grab_once = || grab_frame(input, time, size, DecoderThreads::One);
    let rgb = grab_once()
        .or_else(|first_failure| {
            if first_failure.is_worth_one_more_try() {
                grab_once()
            } else {
                Err(first_failure)
            }
        })
        .with_context(|| format!("could not get the picture at {time:.3}s"))?;
    Ok(Thumbnail { size, rgb })
}

/// The size of a grabbed picture. The video is scaled to fit inside it and
/// centred on black, so its shape is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PictureSize {
    pub width: usize,
    pub height: usize,
}

impl PictureSize {
    /// The size sixteen wide for every nine high that is `width` wide.
    /// `width` is a multiple of sixteen wherever this is used.
    pub const fn widescreen(width: usize) -> Self {
        Self {
            width,
            height: width / WIDESCREEN_WIDTH_UNITS * WIDESCREEN_HEIGHT_UNITS,
        }
    }

    /// How many bytes a picture of this size takes as tightly packed RGB.
    pub const fn rgb_bytes(self) -> usize {
        self.width * self.height * BYTES_PER_RGB_PIXEL
    }

    /// The ffmpeg filter that scales a video to fit this size and centres it
    /// on black.
    pub fn letterbox_filter(self) -> String {
        let Self { width, height } = self;
        format!(
            "scale={width}:{height}:force_original_aspect_ratio=decrease,\
             pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"
        )
    }
}

/// How many processor cores ffmpeg may use to decode one picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderThreads {
    /// For many grabs running side by side, which already fill the cores.
    One,
    /// For a single grab that should be as quick as it can, as ffmpeg
    /// chooses when left alone.
    Automatic,
}

/// Decodes the one picture shown at `time` and returns it as tightly packed
/// RGB rows of `size`. This is the only place a single picture is taken from
/// a video.
pub fn grab_frame(
    input: &Path,
    time: f64,
    size: PictureSize,
    threads: DecoderThreads,
) -> Result<Vec<u8>, FfmpegError> {
    let mut ffmpeg = tools::command(Program::Ffmpeg).args(["-nostdin", "-v", "error"]);
    ffmpeg = match threads {
        DecoderThreads::One => ffmpeg.args(["-threads", "1"]),
        DecoderThreads::Automatic => ffmpeg,
    };
    let rgb = ffmpeg
        .args(["-ss", &format!("{time:.6}")])
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0", "-frames:v", "1"])
        .args(["-vf", &size.letterbox_filter()])
        .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .run(SINGLE_PICTURE_LIMIT)?;

    check_picture_bytes(rgb.len(), size).map_err(|NotUnderstood(detail)| {
        FfmpegError::OutputNotUnderstood {
            program: Program::Ffmpeg,
            detail,
        }
    })?;
    Ok(rgb)
}

/// ffmpeg prints nothing at all when asked for a picture past the end of the
/// video, and still reports success.
fn check_picture_bytes(received_bytes: usize, size: PictureSize) -> Result<(), NotUnderstood> {
    let expected_bytes = size.rgb_bytes();
    if received_bytes == expected_bytes {
        return Ok(());
    }
    Err(NotUnderstood(format!(
        "{received_bytes} bytes instead of a {expected_bytes} byte picture"
    )))
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

    const SMALL: PictureSize = PictureSize {
        width: 4,
        height: 2,
    };

    #[test]
    fn a_picture_takes_three_bytes_for_every_pixel() {
        assert_eq!(SMALL.rgb_bytes(), 24);
        assert_eq!(
            SMALLEST_THUMBNAIL.rgb_bytes(),
            THUMBNAIL_WIDTH * THUMBNAIL_HEIGHT * 3
        );
    }

    #[test]
    fn the_video_is_scaled_to_fit_and_centred_in_the_picture() {
        assert_eq!(
            SMALL.letterbox_filter(),
            "scale=4:2:force_original_aspect_ratio=decrease,pad=4:2:(ow-iw)/2:(oh-ih)/2"
        );
    }

    #[test]
    fn on_an_ordinary_screen_a_preview_keeps_its_usual_size() {
        assert_eq!(thumbnail_size_for(240.0, 1.0), SMALLEST_THUMBNAIL);
        // Up to a third sharper, the usual size still has a pixel for each.
        assert_eq!(thumbnail_size_for(240.0, 1.25), SMALLEST_THUMBNAIL);
    }

    #[test]
    fn on_a_sharp_screen_a_preview_has_a_pixel_for_every_pixel_it_covers() {
        assert_eq!(
            thumbnail_size_for(240.0, 2.0),
            PictureSize {
                width: 480,
                height: 270
            }
        );
        // 360 pixels are needed, and the next width in steps of sixteen is 368.
        assert_eq!(
            thumbnail_size_for(240.0, 1.5),
            PictureSize {
                width: 368,
                height: 207
            }
        );
    }

    #[test]
    fn a_preview_is_never_larger_than_the_largest_thumbnail() {
        let largest = PictureSize {
            width: LARGEST_THUMBNAIL_WIDTH,
            height: 540,
        };
        assert_eq!(thumbnail_size_for(240.0, 4.0), largest);
        assert_eq!(thumbnail_size_for(240.0, 1000.0), largest);
    }

    #[test]
    fn a_scale_that_makes_no_sense_gives_the_smallest_size() {
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                thumbnail_size_for(240.0, scale),
                SMALLEST_THUMBNAIL,
                "{scale}"
            );
        }
    }

    #[test]
    fn a_pixel_size_stays_within_the_widths_it_is_given() {
        assert_eq!(
            pixel_size_for(640.0, 2.0, 640..=1280),
            PictureSize {
                width: 1280,
                height: 720
            }
        );
        assert_eq!(pixel_size_for(320.0, 1.0, 640..=1280).width, 640);
        assert_eq!(pixel_size_for(640.0, 3.0, 640..=1280).width, 1280);
    }

    #[test]
    fn every_size_has_whole_pixels_and_the_widescreen_shape() {
        for tenths in 5..=40 {
            let size = thumbnail_size_for(240.0, tenths as f32 / 10.0);
            assert_eq!(size.width * 9, size.height * 16, "{size:?}");
        }
    }

    #[test]
    fn a_picture_of_the_right_size_is_accepted() {
        assert_eq!(check_picture_bytes(24, SMALL), Ok(()));
    }

    #[test]
    fn an_empty_or_cut_short_picture_is_rejected() {
        assert_eq!(
            check_picture_bytes(0, SMALL),
            Err(NotUnderstood(
                "0 bytes instead of a 24 byte picture".to_owned()
            ))
        );
        assert!(check_picture_bytes(23, SMALL).is_err());
    }
}
