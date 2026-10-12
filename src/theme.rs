//! The look of the app: a light dashboard of white rounded cards on a pale
//! background, with an indigo primary colour and pastel tiles for the key
//! numbers. Colours and shared building blocks live here so every screen
//! draws from the same set.

use eframe::egui;
use egui::{Color32, FontFamily, FontId, RichText};

pub const BACKGROUND: Color32 = Color32::from_rgb(0xF3, 0xF4, 0xF9);
pub const SIDEBAR: Color32 = Color32::from_rgb(0xFB, 0xFB, 0xFE);
pub const CARD: Color32 = Color32::WHITE;
pub const CARD_BORDER: Color32 = Color32::from_rgb(0xE7, 0xE9, 0xF2);

pub const PRIMARY: Color32 = Color32::from_rgb(0x4F, 0x46, 0xC8);
pub const PRIMARY_TINT: Color32 = Color32::from_rgb(0xEC, 0xE9, 0xFD);
pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x7A, 0x50);
pub const SUCCESS: Color32 = Color32::from_rgb(0x1F, 0xA9, 0x7C);
pub const DANGER: Color32 = Color32::from_rgb(0xE0, 0x3E, 0x52);

pub const TEXT: Color32 = Color32::from_rgb(0x1F, 0x23, 0x37);
pub const MUTED_TEXT: Color32 = Color32::from_rgb(0x8A, 0x90, 0xA6);

/// Backgrounds of the tiles that show the key numbers.
pub const MINT_TILE: Color32 = Color32::from_rgb(0xE3, 0xF8, 0xF3);
pub const LAVENDER_TILE: Color32 = PRIMARY_TINT;
pub const PEACH_TILE: Color32 = Color32::from_rgb(0xFF, 0xEE, 0xE4);
pub const ROSE_TILE: Color32 = Color32::from_rgb(0xFD, 0xE6, 0xEA);

/// Each scene gets one of these colours, on its card and on the timeline, so
/// it is plain which strips of previews belong to which scene. Neighbouring
/// scenes always differ.
pub const SCENE_COLORS: [Color32; 8] = [
    Color32::from_rgb(0x4F, 0x46, 0xC8),
    Color32::from_rgb(0xFF, 0x7A, 0x50),
    Color32::from_rgb(0x1F, 0xA9, 0x7C),
    Color32::from_rgb(0xD9, 0x46, 0x8A),
    Color32::from_rgb(0x2E, 0x90, 0xD9),
    Color32::from_rgb(0xE0, 0xA1, 0x1B),
    Color32::from_rgb(0x8E, 0x4F, 0xD6),
    Color32::from_rgb(0x4B, 0x5B, 0x73),
];

pub const CARD_ROUNDING: f32 = 12.0;
pub const CONTROL_ROUNDING: f32 = 8.0;
pub const PANEL_MARGIN: f32 = 16.0;
const PILL_ROUNDING: f32 = 10.0;
const TILE_MIN_WIDTH: f32 = 128.0;
const TILE_VALUE_SIZE: f32 = 22.0;
const CAPTION_SIZE: f32 = 12.0;
const PRIMARY_BUTTON_HEIGHT: f32 = 36.0;

const SEMIBOLD_FAMILY: &str = "semibold";
/// The app borrows a font from the system when it is there, since the font
/// built into the toolkit is thin and hard to read at small sizes.
const REGULAR_FONT_NAME: &str = "system_regular";
const SEMIBOLD_FONT_NAME: &str = "system_semibold";

/// Where a regular and a heavier font may be found. The first file that
/// exists is used.
struct SystemFonts {
    regular: Vec<std::path::PathBuf>,
    semibold: Vec<std::path::PathBuf>,
}

/// Fonts that ship with each operating system. Linux has no single set, so
/// the usual homes of DejaVu Sans on the common distributions are tried.
fn system_fonts() -> SystemFonts {
    use std::path::PathBuf;

    if cfg!(windows) {
        let fonts_folder = std::env::var_os("WINDIR")
            .map(|windows| PathBuf::from(windows).join("Fonts"))
            .unwrap_or_default();
        return SystemFonts {
            regular: vec![fonts_folder.join("segoeui.ttf")],
            semibold: vec![fonts_folder.join("seguisb.ttf")],
        };
    }
    if cfg!(target_os = "macos") {
        let supplemental = PathBuf::from("/System/Library/Fonts/Supplemental");
        return SystemFonts {
            regular: vec![supplemental.join("Arial.ttf")],
            semibold: vec![supplemental.join("Arial Bold.ttf")],
        };
    }
    let dejavu_folders = [
        "/usr/share/fonts/truetype/dejavu",
        "/usr/share/fonts/TTF",
        "/usr/share/fonts/dejavu-sans-fonts",
        "/usr/share/fonts/dejavu",
    ];
    let in_each_folder = |file_name: &str| {
        dejavu_folders
            .iter()
            .map(|folder| PathBuf::from(folder).join(file_name))
            .collect()
    };
    SystemFonts {
        regular: in_each_folder("DejaVuSans.ttf"),
        semibold: in_each_folder("DejaVuSans-Bold.ttf"),
    }
}

/// Applies the fonts, colours and spacing to the whole app. Call once at
/// start-up, and in tests before drawing any screen.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::ThemePreference::Light);
    ctx.all_styles_mut(|style| {
        style.visuals = visuals();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        // Text in this app is for reading, not selecting. Left selectable,
        // every label would swallow the drags meant for the card under it.
        style.interaction.selectable_labels = false;
        style.text_styles = [
            (egui::TextStyle::Heading, semibold_font(20.0)),
            (egui::TextStyle::Body, FontId::proportional(14.0)),
            (egui::TextStyle::Button, FontId::proportional(14.0)),
            (egui::TextStyle::Small, FontId::proportional(CAPTION_SIZE)),
            (egui::TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
    });
}

fn visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = CARD;
    visuals.window_rounding = CARD_ROUNDING.into();
    visuals.window_stroke = egui::Stroke::new(1.0_f32, CARD_BORDER);
    visuals.window_shadow = soft_shadow();
    visuals.popup_shadow = soft_shadow();
    visuals.extreme_bg_color = CARD;
    visuals.selection.bg_fill = PRIMARY_TINT;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, PRIMARY);
    visuals.warn_fg_color = ACCENT;
    visuals.error_fg_color = DANGER;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.fg_stroke.color = TEXT;
    widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, CARD_BORDER);
    widgets.inactive.fg_stroke.color = TEXT;
    widgets.inactive.weak_bg_fill = Color32::from_rgb(0xEE, 0xEF, 0xF8);
    widgets.inactive.bg_fill = Color32::from_rgb(0xEE, 0xEF, 0xF8);
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, CARD_BORDER);
    widgets.hovered.weak_bg_fill = Color32::from_rgb(0xE2, 0xE3, 0xF6);
    widgets.hovered.bg_fill = Color32::from_rgb(0xE2, 0xE3, 0xF6);
    widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, PRIMARY);
    widgets.active.weak_bg_fill = Color32::from_rgb(0xD6, 0xD4, 0xF5);
    widgets.active.bg_fill = Color32::from_rgb(0xD6, 0xD4, 0xF5);
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.rounding = CONTROL_ROUNDING.into();
    }
    visuals
}

pub fn soft_shadow() -> egui::Shadow {
    egui::Shadow {
        offset: egui::vec2(0.0, 4.0),
        blur: 18.0,
        spread: 0.0,
        color: Color32::from_black_alpha(16),
    }
}

/// The toolkit's own fonts stay in place as a fallback, so the app looks the
/// same, only plainer, on a system without the fonts named above.
fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let mut proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let system_fonts = system_fonts();
    if let Some(regular) = load_first_font(&mut fonts, REGULAR_FONT_NAME, &system_fonts.regular) {
        proportional.insert(0, regular);
    }
    let mut semibold = proportional.clone();
    if let Some(heavier) = load_first_font(&mut fonts, SEMIBOLD_FONT_NAME, &system_fonts.semibold) {
        semibold.insert(0, heavier);
    }

    fonts
        .families
        .insert(FontFamily::Proportional, proportional);
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD_FAMILY.into()), semibold);
    fonts
}

/// Registers the first of `candidates` that can be read under `name` and
/// returns that name, or `None` when the system has none of them.
fn load_first_font(
    fonts: &mut egui::FontDefinitions,
    name: &str,
    candidates: &[std::path::PathBuf],
) -> Option<String> {
    let bytes = candidates
        .iter()
        .find_map(|font_file| std::fs::read(font_file).ok())?;
    fonts
        .font_data
        .insert(name.to_owned(), egui::FontData::from_owned(bytes));
    Some(name.to_owned())
}

fn semibold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD_FAMILY.into()))
}

/// Heavier text, for names and numbers that should stand out.
pub fn semibold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(semibold_font(size))
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text).color(MUTED_TEXT)
}

pub fn caption(text: impl Into<String>) -> RichText {
    RichText::new(text).size(CAPTION_SIZE).color(MUTED_TEXT)
}

/// A white rounded card with a hairline border and a soft shadow.
pub fn card() -> egui::Frame {
    egui::Frame::none()
        .fill(CARD)
        .rounding(CARD_ROUNDING)
        .stroke(egui::Stroke::new(1.0_f32, CARD_BORDER))
        .shadow(soft_shadow())
        .inner_margin(egui::Margin::same(14.0))
}

/// The frame of a whole panel, which only sets the background and margin.
pub fn panel(fill: Color32) -> egui::Frame {
    egui::Frame::none()
        .fill(fill)
        .inner_margin(egui::Margin::same(PANEL_MARGIN))
}

/// A pastel tile with one big number and a small caption under it.
pub fn stat_tile(
    ui: &mut egui::Ui,
    value: &str,
    caption_text: &str,
    tint: Color32,
    value_color: Color32,
) {
    egui::Frame::none()
        .fill(tint)
        .rounding(CARD_ROUNDING)
        .inner_margin(egui::Margin::symmetric(16.0, 10.0))
        .show(ui, |ui| {
            ui.set_min_width(TILE_MIN_WIDTH);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(semibold(value, TILE_VALUE_SIZE).color(value_color));
                ui.label(caption(caption_text));
            });
        });
}

/// A small rounded label on a filled background, for badges.
pub fn pill(ui: &mut egui::Ui, text: RichText, fill: Color32) -> egui::Response {
    egui::Frame::none()
        .fill(fill)
        .rounding(PILL_ROUNDING)
        .inner_margin(egui::Margin::symmetric(9.0, 2.0))
        .show(ui, |ui| ui.label(text))
        .response
}

/// The one button on a screen that carries out its main action.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(semibold(text, 14.0).color(Color32::WHITE))
        .fill(PRIMARY)
        .rounding(CONTROL_ROUNDING)
        .min_size(egui::vec2(0.0, PRIMARY_BUTTON_HEIGHT))
}

pub fn scene_color(scene: usize) -> Color32 {
    SCENE_COLORS[scene % SCENE_COLORS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbouring_scenes_never_share_a_colour() {
        for scene in 0..SCENE_COLORS.len() * 2 {
            assert_ne!(scene_color(scene), scene_color(scene + 1));
        }
    }

    #[test]
    fn the_theme_can_be_applied_and_a_frame_drawn() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Scenes");
                ui.label(semibold("3", 22.0));
                stat_tile(ui, "3", "scenes", MINT_TILE, TEXT);
                let _ = ui.add(primary_button("Split"));
            });
        });
    }
}
