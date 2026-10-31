//! Saves what the window shows, for the README and for checking a screen,
//! without the window frame and title bar a screen capture would include.
//!
//! Started with `SCENE_SPLITTER_SCREENSHOT=<file>`, the app looks for
//! `<file>.request` on every frame. When it appears, the app removes it,
//! takes a picture of its next frame and writes it to `<file>` as raw RGBA:
//! width and height as little-endian u32, then the pixels row by row.
//! `scripts/frame_to_png.py` turns that into a PNG. Without the variable
//! nothing here runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;

const TARGET_VARIABLE: &str = "SCENE_SPLITTER_SCREENSHOT";
const REQUEST_SUFFIX: &str = "request";
const PARTIAL_SUFFIX: &str = "part";
/// How often the app looks for a request while nothing else redraws it.
const LOOK_FOR_REQUESTS_EVERY: Duration = Duration::from_millis(250);

pub struct ScreenshotHook {
    target: PathBuf,
}

impl ScreenshotHook {
    /// The hook, when the variable names where pictures go.
    pub fn from_environment() -> Option<Self> {
        let target = std::env::var_os(TARGET_VARIABLE)?;
        Some(Self {
            target: PathBuf::from(target),
        })
    }

    /// Asks for a picture when one was requested, and saves the picture the
    /// last frame delivered. Returns why a picture could not be saved.
    pub fn run_frame(&self, ctx: &egui::Context) -> Result<(), String> {
        ctx.request_repaint_after(LOOK_FOR_REQUESTS_EVERY);
        let request = with_suffix(&self.target, REQUEST_SUFFIX);
        if request.exists() {
            std::fs::remove_file(&request)
                .map_err(|error| format!("remove {}: {error}", request.display()))?;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        let pictures: Vec<Arc<egui::ColorImage>> = ctx.input(|input| {
            input
                .raw
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });
        match pictures.last() {
            Some(picture) => self.save(picture),
            None => Ok(()),
        }
    }

    /// Written beside the target first and renamed, so a script waiting for
    /// the file never reads half of it.
    fn save(&self, picture: &egui::ColorImage) -> Result<(), String> {
        let partial = with_suffix(&self.target, PARTIAL_SUFFIX);
        std::fs::write(&partial, raw_rgba(picture))
            .map_err(|error| format!("write {}: {error}", partial.display()))?;
        std::fs::rename(&partial, &self.target)
            .map_err(|error| format!("rename to {}: {error}", self.target.display()))
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

/// Width and height as little-endian u32, then RGBA pixels row by row.
fn raw_rgba(picture: &egui::ColorImage) -> Vec<u8> {
    let [width, height] = picture.size.map(|side| side as u32);
    let mut bytes = Vec::with_capacity(8 + picture.pixels.len() * 4);
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    for pixel in &picture.pixels {
        bytes.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_written_as_its_size_then_its_pixels() {
        let picture = egui::ColorImage::new([2, 1], egui::Color32::from_rgb(10, 20, 30));
        let bytes = raw_rgba(&picture);
        assert_eq!(&bytes[..8], &[2, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&bytes[8..], &[10, 20, 30, 255, 10, 20, 30, 255]);
    }

    #[test]
    fn the_request_and_the_partial_file_sit_beside_the_target() {
        let target = Path::new("shots").join("review.rgba");
        assert_eq!(
            with_suffix(&target, REQUEST_SUFFIX),
            Path::new("shots").join("review.rgba.request")
        );
    }
}
