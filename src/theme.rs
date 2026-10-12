//! The look of the app: a dashboard of rounded cards on a quiet page, with
//! an indigo primary colour, tinted tiles for the key numbers and one colour
//! per scene. It comes in a light and a dark palette. Colours and shared
//! building blocks live here so every screen draws from the same set.

use std::time::Duration;

use eframe::egui;
use egui::{Color32, FontFamily, FontId, RichText};
use serde::{Deserialize, Serialize};

/// Which palette the window is drawn in, as the person chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Light or dark as the operating system is set, and changing with it.
    /// Also what a settings file from before the choice existed means.
    #[default]
    FollowSystem,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [Self; 3] = [Self::FollowSystem, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::FollowSystem => "Same as the system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn preference(self) -> egui::ThemePreference {
        match self {
            Self::FollowSystem => egui::ThemePreference::System,
            Self::Light => egui::ThemePreference::Light,
            Self::Dark => egui::ThemePreference::Dark,
        }
    }

    /// What the window's own title bar is asked to look like.
    fn title_bar(self) -> egui::SystemTheme {
        match self {
            Self::FollowSystem => egui::SystemTheme::SystemDefault,
            Self::Light => egui::SystemTheme::Light,
            Self::Dark => egui::SystemTheme::Dark,
        }
    }
}

/// The app looked light before it could look dark, so that is what it
/// falls back to on a system that does not say which it prefers.
const THEME_WHEN_THE_SYSTEM_DOES_NOT_SAY: egui::Theme = egui::Theme::Light;

const SCENE_COLOR_COUNT: usize = 8;

/// How strongly a scene's colour washes over its card. Both are weak enough
/// for grey text to stay readable on the card.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardTint {
    /// The card of a selected scene.
    Selected,
    /// The card a dragged scene would be merged into.
    WillMerge,
}

impl CardTint {
    fn strength(self) -> f32 {
        match self {
            Self::Selected => 0.12,
            Self::WillMerge => 0.18,
        }
    }
}

/// Every colour the window uses, once for the light look and once for the
/// dark one. A screen asks for the palette in use (`palette`) and never
/// names a colour of its own.
#[derive(Debug, PartialEq)]
pub struct Palette {
    /// Which of the toolkit's two styles this palette dresses.
    theme: egui::Theme,

    pub page: Color32,
    /// The queue panel.
    pub sidebar: Color32,
    pub card: Color32,
    pub card_border: Color32,

    /// The main action, links and selection. Readable as text on the page
    /// and on cards, and as a fill under `on_fill`.
    pub primary: Color32,
    pub primary_tint: Color32,
    /// Progress and highlights. Not readable as text.
    pub accent: Color32,
    pub success: Color32,
    /// Errors as text, and the fill of buttons that cannot be undone.
    pub danger: Color32,
    /// The accent is not readable as text, this is its readable relative.
    pub warning_text: Color32,

    pub text: Color32,
    /// Secondary text, readable at caption size on every background used.
    pub muted_text: Color32,
    /// Text and marks on a filled colour: a scene colour, the primary
    /// colour, the danger colour or the dropped grey.
    pub on_fill: Color32,
    /// A thin line over a scene colour, such as where two pieces were merged.
    pub faint_on_fill: Color32,

    /// A scene that will not be written has no colour of its own: its badge
    /// and outline are this grey.
    pub dropped: Color32,
    /// The card and the timeline block of a scene that will not be written.
    pub dropped_fill: Color32,

    /// Backgrounds of the tiles that show the key numbers, and of notes.
    pub mint_tile: Color32,
    pub lavender_tile: Color32,
    pub peach_tile: Color32,
    pub rose_tile: Color32,

    /// What is behind a video picture that does not fill its place. Black in
    /// both palettes, like the bars a film is shown with.
    pub picture_backdrop: Color32,
    /// The frame around what the keys act on, and the line inside it that
    /// keeps the frame visible on a picture of any colour.
    pub focus_ring: Color32,
    pub focus_ring_inner: Color32,

    /// Each scene gets one of these colours, on its card and on the
    /// timeline, so it is plain which previews belong to which scene.
    /// Neighbouring scenes always differ, and `on_fill` reads on each.
    scene_colors: [Color32; SCENE_COLOR_COUNT],

    /// The outline of buttons and boxes, visible against the page and cards.
    control_outline: Color32,
    control_fill: Color32,
    control_hovered_fill: Color32,
    control_pressed_fill: Color32,
    /// Inside text boxes.
    input_fill: Color32,
    /// The part of a progress bar that is still to do.
    progress_track: Color32,
    /// How dark the page goes behind a dialog that has to be answered first.
    backdrop: Color32,
    shadow: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// White cards on a pale page, pastel tiles, and scene colours dark enough
/// for white text.
const LIGHT: Palette = Palette {
    theme: egui::Theme::Light,
    page: rgb(0xF3F4F9),
    sidebar: rgb(0xFBFBFE),
    card: Color32::WHITE,
    card_border: rgb(0xE7E9F2),
    primary: rgb(0x4F46C8),
    primary_tint: rgb(0xECE9FD),
    accent: rgb(0xFF7A50),
    success: rgb(0x13805D),
    danger: rgb(0xC8323F),
    warning_text: rgb(0xC2410C),
    text: rgb(0x1F2337),
    muted_text: rgb(0x5B6178),
    on_fill: Color32::WHITE,
    faint_on_fill: Color32::from_rgba_premultiplied(140, 140, 140, 140),
    dropped: rgb(0x5B6178),
    dropped_fill: rgb(0xE7E9F2),
    mint_tile: rgb(0xE3F8F3),
    lavender_tile: rgb(0xECE9FD),
    peach_tile: rgb(0xFFEEE4),
    rose_tile: rgb(0xFEEBEE),
    picture_backdrop: Color32::BLACK,
    focus_ring: rgb(0x1F2337),
    focus_ring_inner: Color32::WHITE,
    scene_colors: [
        rgb(0x4F46C8),
        rgb(0xC4501F),
        rgb(0x13805D),
        rgb(0xC2347A),
        rgb(0x1D6FB8),
        rgb(0x8F6400),
        rgb(0x8E4FD6),
        rgb(0x4B5B73),
    ],
    control_outline: rgb(0xC3C7D9),
    control_fill: rgb(0xEEEFF8),
    control_hovered_fill: rgb(0xE2E3F6),
    control_pressed_fill: rgb(0xD6D4F5),
    input_fill: Color32::WHITE,
    progress_track: rgb(0xC3C7D9),
    backdrop: Color32::from_rgba_premultiplied(14, 16, 25, 110),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 16),
};

/// Designed for the dark, not turned inside out: a blue-black page, cards a
/// step lighter, hairlines lighter again so they show. Colours that sit on
/// the dark are lighter relatives of the light palette's, each with the same
/// hue, and what is written on them is dark. Tiles are deep tints, not
/// pastels, so they do not glare.
const DARK: Palette = Palette {
    theme: egui::Theme::Dark,
    page: rgb(0x12141C),
    sidebar: rgb(0x171A25),
    card: rgb(0x1E2130),
    card_border: rgb(0x363C55),
    primary: rgb(0x9D97FF),
    primary_tint: rgb(0x2B2A5C),
    accent: rgb(0xFF8A63),
    success: rgb(0x4CC79A),
    danger: rgb(0xF2727E),
    warning_text: rgb(0xF2A065),
    text: rgb(0xE6E8F2),
    muted_text: rgb(0xABB1C9),
    on_fill: rgb(0x14151F),
    faint_on_fill: Color32::from_rgba_premultiplied(11, 12, 17, 140),
    dropped: rgb(0xABB1C9),
    dropped_fill: rgb(0x2A2E40),
    mint_tile: rgb(0x15362F),
    lavender_tile: rgb(0x2B2A5C),
    peach_tile: rgb(0x3E2A20),
    rose_tile: rgb(0x40222B),
    picture_backdrop: Color32::BLACK,
    focus_ring: rgb(0xE6E8F2),
    focus_ring_inner: rgb(0x14151F),
    scene_colors: [
        rgb(0x9D97FF),
        rgb(0xF08A5D),
        rgb(0x4DC79B),
        rgb(0xF07FB5),
        rgb(0x62AEF2),
        rgb(0xD9AE45),
        rgb(0xC596F5),
        rgb(0xA3B4CC),
    ],
    control_outline: rgb(0x4D5475),
    control_fill: rgb(0x2A2E42),
    control_hovered_fill: rgb(0x343955),
    control_pressed_fill: rgb(0x3F4468),
    input_fill: rgb(0x151722),
    progress_track: rgb(0x3A4060),
    backdrop: Color32::from_rgba_premultiplied(0, 0, 0, 150),
    // A shadow has to be much heavier to show at all on a dark page.
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 96),
};

impl Palette {
    pub fn of(theme: egui::Theme) -> &'static Self {
        match theme {
            egui::Theme::Light => &LIGHT,
            egui::Theme::Dark => &DARK,
        }
    }

    /// The colour of a scene by its place in the video. A scene keeps it
    /// everywhere it appears.
    pub fn scene_color(&self, scene: usize) -> Color32 {
        self.scene_colors[scene % SCENE_COLOR_COUNT]
    }

    /// A card with a wash of a scene's colour over it.
    pub fn tinted_card(&self, scene_color: Color32, tint: CardTint) -> Color32 {
        wash(self.card, scene_color, tint.strength())
    }

    pub fn soft_shadow(&self) -> egui::Shadow {
        egui::Shadow {
            offset: egui::vec2(0.0, 4.0),
            blur: 18.0,
            spread: 0.0,
            color: self.shadow,
        }
    }
}

/// `base` with `over` laid on it at `strength`, from 0 (all base) to 1.
fn wash(base: Color32, over: Color32, strength: f32) -> Color32 {
    let channel = |base: u8, over: u8| {
        (f32::from(base) + (f32::from(over) - f32::from(base)) * strength).round() as u8
    };
    Color32::from_rgb(
        channel(base.r(), over.r()),
        channel(base.g(), over.g()),
        channel(base.b(), over.b()),
    )
}

/// The palette the window is drawn in during this frame.
pub fn palette(ctx: &egui::Context) -> &'static Palette {
    Palette::of(ctx.theme())
}

/// How long a "Copy details" button says that it has copied.
const COPIED_NOTE_SECONDS: f64 = 2.0;
pub const COPY_DETAILS_LABEL: &str = "Copy details";
const COPIED_LABEL: &str = "Copied";
/// Where the context keeps which title bar was last asked for.
const TITLE_BAR_CHOICE_ID: &str = "title_bar_theme_choice";

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

/// Applies the fonts, both palettes and the spacing to the whole app. Call
/// once at start-up, and in tests before drawing any screen. Which palette
/// shows is then a matter of `show_in`.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.options_mut(|options| options.fallback_theme = THEME_WHEN_THE_SYSTEM_DOES_NOT_SAY);
    for palette in [&LIGHT, &DARK] {
        ctx.style_mut_of(palette.theme, |style| {
            style.visuals = visuals(palette);
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
}

/// Draws the window in the palette the person chose, from the next frame
/// on. Cheap enough to call on every frame, which keeps the window in step
/// with the settings however they changed.
pub fn show_in(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(choice.preference());

    // The title bar belongs to the system and is told only when the choice
    // changes, since telling it wakes the window up.
    let title_bar_choice_id = egui::Id::new(TITLE_BAR_CHOICE_ID);
    let asked_for: Option<ThemeChoice> = ctx.data(|data| data.get_temp(title_bar_choice_id));
    if asked_for == Some(choice) {
        return;
    }
    ctx.data_mut(|data| data.insert_temp(title_bar_choice_id, choice));
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(choice.title_bar()));
}

fn visuals(palette: &Palette) -> egui::Visuals {
    let mut visuals = match palette.theme {
        egui::Theme::Light => egui::Visuals::light(),
        egui::Theme::Dark => egui::Visuals::dark(),
    };
    visuals.panel_fill = palette.page;
    visuals.window_fill = palette.card;
    visuals.window_rounding = CARD_ROUNDING.into();
    visuals.window_stroke = egui::Stroke::new(1.0_f32, palette.card_border);
    visuals.window_shadow = palette.soft_shadow();
    visuals.popup_shadow = palette.soft_shadow();
    visuals.extreme_bg_color = palette.input_fill;
    visuals.code_bg_color = palette.control_fill;
    // The topmost floating window would otherwise get a grey title bar,
    // which is not one of this app's colours.
    visuals.window_highlight_topmost = false;
    visuals.selection.bg_fill = palette.primary_tint;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, palette.primary);
    visuals.warn_fg_color = palette.warning_text;
    // Links are secondary actions set in a line of text, in the app's own colour.
    visuals.hyperlink_color = palette.primary;
    visuals.error_fg_color = palette.danger;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, palette.card_border);
    widgets.inactive.weak_bg_fill = palette.control_fill;
    widgets.inactive.bg_fill = palette.control_fill;
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, palette.control_outline);
    widgets.hovered.weak_bg_fill = palette.control_hovered_fill;
    widgets.hovered.bg_fill = palette.control_hovered_fill;
    widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, palette.primary);
    widgets.active.weak_bg_fill = palette.control_pressed_fill;
    widgets.active.bg_fill = palette.control_pressed_fill;
    // A box whose list is open, which the toolkit would leave in its own grey.
    widgets.open.weak_bg_fill = palette.control_hovered_fill;
    widgets.open.bg_fill = palette.control_hovered_fill;
    widgets.open.bg_stroke = egui::Stroke::new(1.0_f32, palette.control_outline);
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.fg_stroke.color = palette.text;
        widget.rounding = CONTROL_ROUNDING.into();
    }
    visuals
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

/// Secondary text at body size, in the grey of the palette in use.
pub fn muted(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(palette(ui.ctx()).muted_text)
}

pub fn caption(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    muted(ui, text).size(CAPTION_SIZE)
}

/// A rounded card with a hairline border and a soft shadow.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let palette = palette(ui.ctx());
    egui::Frame::none()
        .fill(palette.card)
        .rounding(CARD_ROUNDING)
        .stroke(egui::Stroke::new(1.0_f32, palette.card_border))
        .shadow(palette.soft_shadow())
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
pub fn header_panel(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::none()
        .fill(palette(ctx).page)
        .inner_margin(egui::Margin {
            left: PANEL_MARGIN,
            right: PANEL_MARGIN,
            top: PANEL_MARGIN,
            bottom: 4.0,
        })
}

/// A tinted tile with one big number and a small caption under it.
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
                ui.label(caption(ui, caption_text));
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
pub fn danger_button(ui: &egui::Ui, text: impl Into<String>) -> egui::Button<'static> {
    let palette = palette(ui.ctx());
    egui::Button::new(semibold(text, 14.0).color(palette.on_fill))
        .fill(palette.danger)
        .rounding(CONTROL_ROUNDING)
}

/// The one button on a screen that carries out its main action.
pub fn primary_button(ui: &egui::Ui, text: impl Into<String>) -> egui::Button<'static> {
    let palette = palette(ui.ctx());
    egui::Button::new(semibold(text, 14.0).color(palette.on_fill))
        .fill(palette.primary)
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
/// way. It draws its own track, so it shows on a card too.
pub fn progress_bar(ui: &mut egui::Ui, fraction: f32, height: f32) -> egui::Response {
    let size = egui::vec2(ui.available_width(), height);
    let (track, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let rounding = height / 2.0;
    let palette = palette(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(track, rounding, palette.progress_track);

    let done_width = track.width() * fraction.clamp(0.0, 1.0);
    if done_width > 0.0 {
        // Never narrower than it is high, or its round ends would not fit.
        let width = done_width.max(height).min(track.width());
        let done = egui::Rect::from_min_size(track.min, egui::vec2(width, height));
        painter.rect_filled(done, rounding, palette.accent);
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
            ui.painter().rect_filled(screen, 0.0, palette(ctx).backdrop);
            response.clicked()
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    const PALETTES: [&Palette; 2] = [&LIGHT, &DARK];

    #[test]
    fn neighbouring_scenes_never_share_a_colour() {
        for palette in PALETTES {
            for scene in 0..SCENE_COLOR_COUNT * 2 {
                assert_ne!(
                    palette.scene_color(scene),
                    palette.scene_color(scene + 1),
                    "{:?} scene {scene}",
                    palette.theme
                );
            }
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

    /// What text needs against its background.
    const READABLE: f64 = 4.5;
    /// What a coloured shape needs against what is around it.
    const DISTINCT: f64 = 3.0;
    /// A hairline is meant to be faint, but not to vanish.
    const VISIBLE_HAIRLINE: f64 = 1.2;

    fn assert_readable(palette: &Palette, text: Color32, background: Color32) {
        assert!(
            contrast(text, background) >= READABLE,
            "{:?}: {text:?} on {background:?} is {:.2} to 1",
            palette.theme,
            contrast(text, background)
        );
    }

    /// A scene's card in every wash it can get, for every scene colour.
    fn tinted_cards(palette: &Palette) -> Vec<Color32> {
        let mut cards = Vec::new();
        for scene_color in palette.scene_colors {
            for tint in [CardTint::Selected, CardTint::WillMerge] {
                cards.push(palette.tinted_card(scene_color, tint));
            }
        }
        cards
    }

    /// Everything body text and grey text are written on.
    fn text_backgrounds(palette: &Palette) -> Vec<Color32> {
        let mut backgrounds = vec![
            palette.card,
            palette.sidebar,
            palette.page,
            palette.mint_tile,
            palette.peach_tile,
            palette.rose_tile,
            palette.lavender_tile,
            palette.primary_tint,
            palette.dropped_fill,
        ];
        backgrounds.extend(tinted_cards(palette));
        backgrounds
    }

    #[test]
    fn text_on_a_filled_colour_can_be_read() {
        for palette in PALETTES {
            let fills = palette.scene_colors.into_iter().chain([
                palette.primary,
                palette.danger,
                palette.dropped,
            ]);
            for fill in fills {
                assert_readable(palette, palette.on_fill, fill);
            }
        }
    }

    #[test]
    fn grey_text_can_be_read_on_every_background() {
        for palette in PALETTES {
            for background in text_backgrounds(palette) {
                assert_readable(palette, palette.muted_text, background);
            }
        }
    }

    #[test]
    fn body_text_can_be_read_on_every_background_and_control() {
        for palette in PALETTES {
            let controls = [
                palette.control_fill,
                palette.control_hovered_fill,
                palette.control_pressed_fill,
                palette.input_fill,
            ];
            for background in text_backgrounds(palette).into_iter().chain(controls) {
                assert_readable(palette, palette.text, background);
            }
        }
    }

    #[test]
    fn coloured_text_can_be_read_where_it_is_written() {
        for palette in PALETTES {
            for color in [palette.primary, palette.danger, palette.warning_text] {
                for background in [palette.card, palette.sidebar, palette.page] {
                    assert_readable(palette, color, background);
                }
            }
            // The verdict of a split, on its card.
            assert_readable(palette, palette.success, palette.card);
            // The chosen entry of a list, and a note on a scene.
            assert_readable(palette, palette.primary, palette.primary_tint);
            // The number on the tile of a scene that is too long.
            assert_readable(palette, palette.danger, palette.rose_tile);
        }
    }

    #[test]
    fn every_scene_colour_stands_out_from_the_page_and_the_cards() {
        for palette in PALETTES {
            for scene_color in palette.scene_colors {
                for surrounding in [palette.page, palette.card, palette.picture_backdrop] {
                    assert!(
                        contrast(scene_color, surrounding) >= DISTINCT,
                        "{:?}: {scene_color:?} beside {surrounding:?}",
                        palette.theme
                    );
                }
            }
        }
    }

    #[test]
    fn hairlines_and_outlines_show_on_what_they_are_drawn_on() {
        for palette in PALETTES {
            for line in [palette.card_border, palette.control_outline] {
                assert!(
                    contrast(line, palette.card) >= VISIBLE_HAIRLINE,
                    "{:?}: {line:?}",
                    palette.theme
                );
            }
            assert!(contrast(palette.focus_ring, palette.focus_ring_inner) >= READABLE);
            assert!(contrast(palette.focus_ring, palette.page) >= READABLE);
        }
    }

    #[test]
    fn a_dark_card_is_lighter_than_the_dark_page_and_its_hairline_lighter_still() {
        assert!(luminance(DARK.card) > luminance(DARK.page));
        assert!(luminance(DARK.sidebar) > luminance(DARK.page));
        assert!(luminance(DARK.card_border) > luminance(DARK.card));
    }

    /// Where a colour sits on the colour wheel, in degrees.
    fn hue_degrees(color: Color32) -> f64 {
        let [red, green, blue] = [color.r(), color.g(), color.b()].map(f64::from);
        let strongest = red.max(green).max(blue);
        let spread = strongest - red.min(green).min(blue);
        let sixth = if strongest == red {
            ((green - blue) / spread).rem_euclid(6.0)
        } else if strongest == green {
            (blue - red) / spread + 2.0
        } else {
            (red - green) / spread + 4.0
        };
        sixth * 60.0
    }

    #[test]
    fn a_scene_keeps_its_hue_in_both_palettes() {
        const SAME_HUE_DEGREES: f64 = 10.0;
        for scene in 0..SCENE_COLOR_COUNT {
            let light = hue_degrees(LIGHT.scene_color(scene));
            let dark = hue_degrees(DARK.scene_color(scene));
            let apart = (light - dark).abs().min(360.0 - (light - dark).abs());
            assert!(
                apart <= SAME_HUE_DEGREES,
                "scene {scene}: {light:.0} and {dark:.0} degrees"
            );
        }
    }

    #[test]
    fn a_tinted_card_sits_between_the_card_and_the_scene_colour() {
        let red = Color32::from_rgb(255, 0, 0);
        assert_eq!(wash(Color32::WHITE, red, 0.0), Color32::WHITE);
        assert_eq!(wash(Color32::WHITE, red, 1.0), red);
        assert_eq!(
            wash(Color32::WHITE, red, 0.2),
            Color32::from_rgb(255, 204, 204)
        );
        for palette in PALETTES {
            let scene_color = palette.scene_color(0);
            let selected = palette.tinted_card(scene_color, CardTint::Selected);
            let will_merge = palette.tinted_card(scene_color, CardTint::WillMerge);
            assert_ne!(selected, palette.card);
            assert_ne!(will_merge, selected);
        }
    }

    #[test]
    fn the_picture_is_shown_on_black_in_both_palettes() {
        for palette in PALETTES {
            assert_eq!(palette.picture_backdrop, Color32::BLACK);
        }
    }

    #[test]
    fn the_window_follows_the_system_until_the_person_chooses() {
        assert_eq!(ThemeChoice::default(), ThemeChoice::FollowSystem);
        assert_eq!(
            ThemeChoice::FollowSystem.preference(),
            egui::ThemePreference::System
        );
    }

    #[test]
    fn every_choice_has_a_name_of_its_own() {
        let [system, light, dark] = ThemeChoice::ALL.map(ThemeChoice::label);
        assert_ne!(system, light);
        assert_ne!(light, dark);
        assert_ne!(system, dark);
    }

    /// Draws one frame, as a system set to `system_theme` would show it.
    fn draw_a_frame(ctx: &egui::Context, system_theme: Option<egui::Theme>) {
        let input = egui::RawInput {
            system_theme,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let palette = palette(ui.ctx());
                ui.heading("Scenes");
                ui.label(semibold("3", 22.0));
                stat_tile(ui, "3", "scenes", palette.mint_tile, palette.text);
                card(ui).show(ui, |ui| ui.label(caption(ui, "a caption")));
                let _ = ui.add(primary_button(ui, "Split"));
                let _ = ui.add(danger_button(ui, "Replace them"));
                let _ = progress_bar(ui, 0.4, 8.0);
                let _ = copy_details_button(ui, "the reason");
                let _ = ui.add(selectable(muted(ui, "the reason")));
            });
            let _ = show_backdrop(ctx, "test_backdrop", egui::Order::Middle);
        });
    }

    #[test]
    fn the_theme_can_be_applied_and_a_frame_drawn_in_each_palette() {
        let ctx = egui::Context::default();
        apply(&ctx);
        for choice in ThemeChoice::ALL {
            show_in(&ctx, choice);
            draw_a_frame(&ctx, None);
        }
    }

    #[test]
    fn the_chosen_palette_is_in_use_whatever_the_system_says() {
        let ctx = egui::Context::default();
        apply(&ctx);

        show_in(&ctx, ThemeChoice::Dark);
        draw_a_frame(&ctx, Some(egui::Theme::Light));
        assert_eq!(palette(&ctx), &DARK);
        assert_eq!(ctx.style().visuals.panel_fill, DARK.page);

        show_in(&ctx, ThemeChoice::Light);
        draw_a_frame(&ctx, Some(egui::Theme::Dark));
        assert_eq!(palette(&ctx), &LIGHT);
        assert_eq!(ctx.style().visuals.panel_fill, LIGHT.page);
    }

    #[test]
    fn following_the_system_changes_palette_when_the_system_does() {
        let ctx = egui::Context::default();
        apply(&ctx);
        show_in(&ctx, ThemeChoice::FollowSystem);

        draw_a_frame(&ctx, Some(egui::Theme::Dark));
        assert_eq!(palette(&ctx), &DARK);
        draw_a_frame(&ctx, Some(egui::Theme::Light));
        assert_eq!(palette(&ctx), &LIGHT);
    }

    #[test]
    fn a_system_that_does_not_say_gets_the_light_palette() {
        let ctx = egui::Context::default();
        apply(&ctx);
        show_in(&ctx, ThemeChoice::FollowSystem);
        draw_a_frame(&ctx, None);
        assert_eq!(palette(&ctx), &LIGHT);
    }

    #[test]
    fn the_title_bar_is_told_of_a_choice_once() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let commands_after = |choice| {
            show_in(&ctx, choice);
            let output = ctx.run(egui::RawInput::default(), |_| {});
            let root = &output.viewport_output[&egui::ViewportId::ROOT];
            root.commands.clone()
        };

        assert_eq!(
            commands_after(ThemeChoice::Dark),
            [egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark)]
        );
        assert_eq!(commands_after(ThemeChoice::Dark), []);
        assert_eq!(
            commands_after(ThemeChoice::FollowSystem),
            [egui::ViewportCommand::SetTheme(
                egui::SystemTheme::SystemDefault
            )]
        );
    }
}
