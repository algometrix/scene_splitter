//! Line icons drawn with egui shapes on a 16 by 16 grid, so they stay sharp
//! at any scale and need no icon font. Also the app's own mark, for the
//! window icon and the sidebar.

use std::f32::consts::{PI, TAU};

use eframe::egui::{self, pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

/// The app's mark, drawn by `scripts/make_icon.py`.
const MARK_PNG: &[u8] = include_bytes!("../assets/icon.png");
const MARK_TEXTURE_NAME: &str = "scene_splitter_mark";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Plus,
    Play,
    Pause,
    Folder,
    Gear,
    Close,
    Refresh,
    Clock,
    Film,
    Scissors,
    Merge,
    Undo,
    Layers,
    Trash,
    Alert,
    Check,
    /// To the cut before.
    SkipBack,
    /// To the cut after.
    SkipForward,
    /// One picture back.
    StepBack,
    /// One picture forward.
    StepForward,
    /// Three dots, for a menu of things used less often.
    More,
    /// A speaker with sound coming from it.
    SoundOn,
    /// A speaker with its sound crossed out.
    SoundOff,
}

impl Icon {
    /// Paints the icon centred in `rect`. Only its shorter side is used.
    pub fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let side = rect.width().min(rect.height());
        let c = rect.center();
        let u = side / 16.0;
        let p = |x: f32, y: f32| pos2(c.x + (x - 8.0) * u, c.y + (y - 8.0) * u);
        let stroke = Stroke::new((1.6 * u).max(1.2), color);
        let line = |points: Vec<Pos2>| painter.add(Shape::line(points, stroke));
        let closed = |points: Vec<Pos2>| painter.add(Shape::closed_line(points, stroke));
        let filled = |points: Vec<Pos2>| {
            painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
        };

        match self {
            Icon::Plus => {
                line(vec![p(8.0, 3.0), p(8.0, 13.0)]);
                line(vec![p(3.0, 8.0), p(13.0, 8.0)]);
            }
            Icon::Play => filled(vec![p(5.0, 3.0), p(13.0, 8.0), p(5.0, 13.0)]),
            Icon::Pause => {
                for x in [5.5, 10.5] {
                    let bar = Rect::from_center_size(p(x, 8.0), vec2(2.6 * u, 10.0 * u));
                    painter.rect_filled(bar, 1.0, color);
                }
            }
            Icon::Folder => {
                closed(vec![
                    p(2.0, 4.0),
                    p(6.0, 4.0),
                    p(7.5, 5.5),
                    p(14.0, 5.5),
                    p(14.0, 13.0),
                    p(2.0, 13.0),
                ]);
                line(vec![p(2.0, 7.5), p(14.0, 7.5)]);
            }
            Icon::Gear => {
                painter.circle_stroke(c, 2.2 * u, stroke);
                painter.circle_stroke(c, 4.6 * u, stroke);
                for tooth in 0..8 {
                    let angle = tooth as f32 * TAU / 8.0;
                    let direction = vec2(angle.cos(), angle.sin());
                    line(vec![c + direction * 4.6 * u, c + direction * 6.6 * u]);
                }
            }
            Icon::Close => {
                line(vec![p(4.0, 4.0), p(12.0, 12.0)]);
                line(vec![p(12.0, 4.0), p(4.0, 12.0)]);
            }
            Icon::Refresh => {
                line(arc(c, 5.0 * u, -PI * 0.35, PI * 1.45, 24));
                let tip = c + vec2((-PI * 0.35).cos(), (-PI * 0.35).sin()) * 5.0 * u;
                line(vec![
                    tip + vec2(-3.0 * u, -0.4 * u),
                    tip,
                    tip + vec2(-0.2 * u, 3.0 * u),
                ]);
            }
            Icon::Clock => {
                painter.circle_stroke(c, 6.0 * u, stroke);
                line(vec![p(8.0, 4.5), p(8.0, 8.0), p(10.8, 9.8)]);
            }
            Icon::Film => {
                closed(vec![p(2.0, 2.5), p(14.0, 2.5), p(14.0, 13.5), p(2.0, 13.5)]);
                line(vec![p(4.8, 2.5), p(4.8, 13.5)]);
                line(vec![p(11.2, 2.5), p(11.2, 13.5)]);
                for y in [5.2, 8.0, 10.8] {
                    line(vec![p(2.0, y), p(4.8, y)]);
                    line(vec![p(11.2, y), p(14.0, y)]);
                }
            }
            Icon::Scissors => {
                painter.circle_stroke(p(4.5, 11.5), 2.2 * u, stroke);
                painter.circle_stroke(p(11.5, 11.5), 2.2 * u, stroke);
                line(vec![p(6.0, 9.8), p(12.5, 2.5)]);
                line(vec![p(10.0, 9.8), p(3.5, 2.5)]);
            }
            Icon::Merge => {
                // Two lines that come together into one.
                line(vec![p(3.0, 2.5), p(3.0, 5.0), p(8.0, 9.5), p(8.0, 14.0)]);
                line(vec![p(13.0, 2.5), p(13.0, 5.0), p(8.0, 9.5)]);
                line(vec![p(5.5, 11.5), p(8.0, 14.0), p(10.5, 11.5)]);
            }
            Icon::Undo => {
                line(arc(p(9.0, 9.5), 4.5 * u, -PI * 0.5, PI * 0.5, 16));
                line(vec![p(9.0, 5.0), p(3.0, 5.0)]);
                line(vec![p(5.8, 2.2), p(3.0, 5.0), p(5.8, 7.8)]);
                line(vec![p(9.0, 14.0), p(6.0, 14.0)]);
            }
            Icon::Layers => {
                closed(vec![p(8.0, 2.0), p(14.5, 5.5), p(8.0, 9.0), p(1.5, 5.5)]);
                line(vec![p(1.5, 8.5), p(8.0, 12.0), p(14.5, 8.5)]);
                line(vec![p(1.5, 11.0), p(8.0, 14.5), p(14.5, 11.0)]);
            }
            Icon::Trash => {
                line(vec![p(2.5, 4.5), p(13.5, 4.5)]);
                line(vec![p(6.0, 4.5), p(6.5, 2.5), p(9.5, 2.5), p(10.0, 4.5)]);
                closed(vec![p(4.0, 4.5), p(12.0, 4.5), p(11.2, 14.0), p(4.8, 14.0)]);
                line(vec![p(6.8, 7.0), p(6.8, 11.5)]);
                line(vec![p(9.2, 7.0), p(9.2, 11.5)]);
            }
            Icon::Check => {
                line(vec![p(3.0, 8.5), p(6.5, 12.0), p(13.0, 4.5)]);
            }
            Icon::SkipBack => {
                line(vec![p(4.0, 3.5), p(4.0, 12.5)]);
                filled(vec![p(12.5, 3.5), p(12.5, 12.5), p(5.5, 8.0)]);
            }
            Icon::SkipForward => {
                line(vec![p(12.0, 3.5), p(12.0, 12.5)]);
                filled(vec![p(3.5, 3.5), p(10.5, 8.0), p(3.5, 12.5)]);
            }
            Icon::StepBack => {
                line(vec![p(10.0, 3.5), p(5.5, 8.0), p(10.0, 12.5)]);
            }
            Icon::StepForward => {
                line(vec![p(6.0, 3.5), p(10.5, 8.0), p(6.0, 12.5)]);
            }
            Icon::More => {
                for x in [3.5, 8.0, 12.5] {
                    painter.circle_filled(p(x, 8.0), 1.3 * u, color);
                }
            }
            Icon::SoundOn | Icon::SoundOff => {
                filled(vec![p(2.0, 6.0), p(5.0, 6.0), p(5.0, 10.0), p(2.0, 10.0)]);
                filled(vec![p(5.0, 6.0), p(9.0, 3.0), p(9.0, 13.0), p(5.0, 10.0)]);
                if self == Icon::SoundOn {
                    line(arc(p(9.5, 8.0), 2.5 * u, -0.9, 0.9, 8));
                    line(arc(p(9.5, 8.0), 4.8 * u, -0.9, 0.9, 10));
                } else {
                    line(vec![p(11.0, 6.0), p(15.0, 10.0)]);
                    line(vec![p(15.0, 6.0), p(11.0, 10.0)]);
                }
            }
            Icon::Alert => {
                closed(vec![p(8.0, 2.0), p(14.5, 13.5), p(1.5, 13.5)]);
                line(vec![p(8.0, 6.0), p(8.0, 9.5)]);
                painter.circle_filled(p(8.0, 11.6), 0.9 * u, color);
            }
        }
    }
}

/// Points along a circular arc from `start` to `end` radians.
fn arc(center: Pos2, radius: f32, start: f32, end: f32, segments: usize) -> Vec<Pos2> {
    (0..=segments)
        .map(|step| {
            let angle = start + (end - start) * step as f32 / segments as f32;
            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

fn mark_image() -> egui::ColorImage {
    let icon = window_icon();
    let size = [icon.width as usize, icon.height as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba)
}

/// The app's mark as a texture, made once and kept in egui's memory.
pub fn mark_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new(MARK_TEXTURE_NAME);
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let texture = ctx.load_texture(
        MARK_TEXTURE_NAME,
        mark_image(),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

/// The icon of the window and of its button on the taskbar or dock.
pub fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(MARK_PNG)
        .expect("assets/icon.png is a PNG built into the program")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_built_into_the_program_can_be_read() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
