//! The look of the app: a light dashboard of white rounded cards on a pale
//! background, with an indigo primary colour and pastel tiles for the key
//! numbers. Colours and shared building blocks live here so every screen
//! draws from the same set.

use std::time::Duration;

use eframe::egui;
use egui::{Color32, FontFamily, FontId, RichText};

pub const BACKGROUND: Color32 = Color32::from_rgb(0xF3, 0xF4, 0xF9);
pub const SIDEBAR: Color32 = Color32::from_rgb(0xFB, 0xFB, 0xFE);
pub const CARD: Color32 = Color32::WHITE;
pub const CARD_BORDER: Color32 = Color32::from_rgb(0xE7, 0xE9, 0xF2);

pub const PRIMARY: Color32 = Color32::from_rgb(0x4F, 0x46, 0xC8);
pub const PRIMARY_TINT: Color32 = Color32::from_rgb(0xEC, 0xE9, 0xFD);
pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x7A, 0x50);
pub const SUCCESS: Color32 = Color32::from_rgb(0x13, 0x80, 0x5D);
/// Dark enough for white text on it, and for use as text on the page.
pub const DANGER: Color32 = Color32::from_rgb(0xC8, 0x32, 0x3F);
/// The accent is too light to read as text, this is its readable relative.
pub const WARNING_TEXT: Color32 = Color32::from_rgb(0xC2, 0x41, 0x0C);
/// The outline of buttons and boxes, visible against the page and cards.
const CONTROL_OUTLINE: Color32 = Color32::from_rgb(0xC3, 0xC7, 0xD9);

/// How dark the page goes behind a dialog that has to be answered first.
const BACKDROP: Color32 = Color32::from_rgba_premultiplied(14, 16, 25, 110);
/// The part of a progress bar that is still to do.
const PROGRESS_TRACK: Color32 = CONTROL_OUTLINE;
/// How long a "Copy details" button says that it has copied.
const COPIED_NOTE_SECONDS: f64 = 2.0;
pub const COPY_DETAILS_LABEL: &str = "Copy details";
const COPIED_LABEL: &str = "Copied";

pub const TEXT: Color32 = Color32::from_rgb(0x1F, 0x23, 0x37);
/// Grey, but dark enough to read at caption size on every background used
/// here (at least 5 to 1 against each).
pub const MUTED_TEXT: Color32 = Color32::from_rgb(0x5B, 0x61, 0x78);

/// Backgrounds of the tiles that show the key numbers.
pub const MINT_TILE: Color32 = Color32::from_rgb(0xE3, 0xF8, 0xF3);
pub const LAVENDER_TILE: Color32 = PRIMARY_TINT;
pub const PEACH_TILE: Color32 = Color32::from_rgb(0xFF, 0xEE, 0xE4);
pub const ROSE_TILE: Color32 = Color32::from_rgb(0xFD, 0xE6, 0xEA);

/// Each scene gets one of these colours, on its card and on the timeline, so
/// it is plain which previews belong to which scene. Neighbouring scenes
/// always differ, and each is dark enough for white text on it.
pub const SCENE_COLORS: [Color32; 8] = [
    Color32::from_rgb(0x4F, 0x46, 0xC8),
    Color32::from_rgb(0xC4, 0x50, 0x1F),
    Color32::from_rgb(0x13, 0x80, 0x5D),
    Color32::from_rgb(0xC2, 0x34, 0x7A),
    Color32::from_rgb(0x1D, 0x6F, 0xB8),
    Color32::from_rgb(0x8F, 0x64, 0x00),
    Color32::from_rgb(0x8E, 0x4F, 0xD6),
    Color32::from_rgb(0x4B, 0x5B, 0x73),
];

pub const CARD_ROUNDING: f32 = 12.0;
pub const CONTROL_ROUNDING: f32 = 8.0;
/// The height of buttons, text boxes and the rows they sit in.
pub const CONTROL_HEIGHT: f32 = 32.0;
pub const PANEL_MARGIN: f32 = 16.0;
const PILL_ROUNDING: f32 = 10.0;
const TILE_VALUE_SIZE: f32 = 20.0;
const CAPTION_SIZE: f32 = 12.0;
pub const PRIMARY_BUTTON_HEIGHT: f32 = 36.0;

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
        style.spacing.window_margin = egui::Margin::same(16.0);
        // Every row is as tall as a button from the start. A label placed
        // before a taller control would otherwise sit above its middle,
        // since a row only grows when the taller control arrives.
        style.spacing.interact_size = egui::vec2(40.0, CONTROL_HEIGHT);
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
    // The topmost floating window would otherwise get a grey title bar,
    // which is not one of this app's colours.
    visuals.window_highlight_topmost = false;
    visuals.selection.bg_fill = PRIMARY_TINT;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, PRIMARY);
    visuals.warn_fg_color = WARNING_TEXT;
    // Links are secondary actions set in a line of text, in the app's own colour.
    visuals.hyperlink_color = PRIMARY;
    visuals.error_fg_color = DANGER;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.fg_stroke.color = TEXT;
    widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, CARD_BORDER);
    widgets.inactive.fg_stroke.color = TEXT;
    widgets.inactive.weak_bg_fill = Color32::from_rgb(0xEE, 0xEF, 0xF8);
    widgets.inactive.bg_fill = Color32::from_rgb(0xEE, 0xEF, 0xF8);
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, CONTROL_OUTLINE);
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

/// The heavier font, for text painted directly and not laid out as a label.
pub fn semibold_font(size: f32) -> FontId {
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

/// The frame of the review screen's fixed header. No gap below it, since
/// the list underneath brings its own.
pub fn header_panel() -> egui::Frame {
    egui::Frame::none()
        .fill(BACKGROUND)
        .inner_margin(egui::Margin {
            left: PANEL_MARGIN,
            right: PANEL_MARGIN,
            top: PANEL_MARGIN,
            bottom: 4.0,
        })
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

/// A pill that keeps a minimum width, so pills in a column line up.
pub fn pill_at_least(
    ui: &mut egui::Ui,
    text: RichText,
    fill: Color32,
    min_width: f32,
) -> egui::Response {
    egui::Frame::none()
        .fill(fill)
        .rounding(PILL_ROUNDING)
        .inner_margin(egui::Margin::symmetric(9.0, 2.0))
        .show(ui, |ui| {
            ui.set_min_width(min_width);
            ui.label(text)
        })
        .response
}

/// A button for something that cannot be taken back, such as replacing files.
pub fn danger_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(semibold(text, 14.0).color(Color32::WHITE))
        .fill(DANGER)
        .rounding(CONTROL_ROUNDING)
}

/// The one button on a screen that carries out its main action.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(semibold(text, 14.0).color(Color32::WHITE))
        .fill(PRIMARY)
        .rounding(CONTROL_ROUNDING)
        .min_size(egui::vec2(0.0, PRIMARY_BUTTON_HEIGHT))
}

/// Text the person may want to take elsewhere, such as the reason for an
/// error. It can be selected with the mouse, unlike the rest of the app's
/// text.
pub fn selectable(text: RichText) -> egui::Label {
    egui::Label::new(text).selectable(true).wrap()
}

/// A button that puts `details` on the clipboard, whole, however little of
/// it is in view. It says so for a moment afterwards.
pub fn copy_details_button(ui: &mut egui::Ui, details: &str) -> egui::Response {
    let copied_at_id = ui.auto_id_with("details_copied_at");
    let now = ui.input(|input| input.time);
    let copied_at: Option<f64> = ui.data(|data| data.get_temp(copied_at_id));
    let just_copied = copied_at.is_some_and(|copied_at| now - copied_at < COPIED_NOTE_SECONDS);
    let label = if just_copied {
        COPIED_LABEL
    } else {
        COPY_DETAILS_LABEL
    };

    let button = ui
        .button(label)
        .on_hover_text("Puts the whole text on the clipboard, so you can paste it somewhere.");
    if button.clicked() {
        ui.ctx().copy_text(details.to_owned());
        ui.data_mut(|data| data.insert_temp(copied_at_id, now));
        // Nothing else redraws the window when the note is due to go away.
        ui.ctx()
            .request_repaint_after(Duration::from_secs_f64(COPIED_NOTE_SECONDS));
    }
    button
}

/// A bar as wide as the room there is, filled as far as `fraction` of the
/// way. It draws its own track, so it shows on a white card too.
pub fn progress_bar(ui: &mut egui::Ui, fraction: f32, height: f32) -> egui::Response {
    let size = egui::vec2(ui.available_width(), height);
    let (track, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let rounding = height / 2.0;
    let painter = ui.painter();
    painter.rect_filled(track, rounding, PROGRESS_TRACK);

    let done_width = track.width() * fraction.clamp(0.0, 1.0);
    if done_width > 0.0 {
        // Never narrower than it is high, or its round ends would not fit.
        let width = done_width.max(height).min(track.width());
        let done = egui::Rect::from_min_size(track.min, egui::vec2(width, height));
        painter.rect_filled(done, rounding, ACCENT);
    }
    response
}

/// Dims everything drawn below `order` behind a dialog, and takes the clicks
/// meant for it. Returns true when the person clicked the dimmed part, which
/// closes the dialog like a click beside any menu does.
pub fn show_backdrop(ctx: &egui::Context, id: &'static str, order: egui::Order) -> bool {
    egui::Area::new(egui::Id::new(id))
        .order(order)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            let response = ui.allocate_rect(screen, egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, BACKDROP);
            response.clicked()
        })
        .inner
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

    /// Relative luminance and contrast as WCAG 2 defines them.
    fn luminance(color: Color32) -> f64 {
        let channel = |value: u8| {
            let share = f64::from(value) / 255.0;
            if share <= 0.03928 {
                share / 12.92
            } else {
                ((share + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(one: Color32, other: Color32) -> f64 {
        let (lighter, darker) = if luminance(one) > luminance(other) {
            (luminance(one), luminance(other))
        } else {
            (luminance(other), luminance(one))
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    const READABLE: f64 = 4.5;

    #[test]
    fn white_text_can_be_read_on_every_scene_colour() {
        for color in SCENE_COLORS.into_iter().chain([PRIMARY, DANGER]) {
            assert!(contrast(Color32::WHITE, color) >= READABLE, "{color:?}");
        }
    }

    #[test]
    fn grey_text_can_be_read_on_every_background() {
        let backgrounds = [
            CARD,
            SIDEBAR,
            BACKGROUND,
            MINT_TILE,
            PEACH_TILE,
            ROSE_TILE,
            LAVENDER_TILE,
        ];
        for background in backgrounds {
            assert!(
                contrast(MUTED_TEXT, background) >= READABLE,
                "{background:?}"
            );
        }
    }

    #[test]
    fn coloured_text_can_be_read_on_a_card() {
        for color in [TEXT, PRIMARY, DANGER, WARNING_TEXT, SUCCESS] {
            assert!(contrast(color, CARD) >= READABLE, "{color:?}");
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
                let _ = progress_bar(ui, 0.4, 8.0);
                let _ = copy_details_button(ui, "the reason");
                let _ = ui.add(selectable(muted("the reason")));
            });
        });
    }
}
