//! The look of the app: a dashboard of rounded cards on a charcoal or soft
//! grey page, a violet primary colour from the icon, pill-shaped buttons
//! with icons, tiles for the key numbers and one colour per scene. It comes
//! in a light and a dark palette. Colours and shared building blocks live
//! here so every screen draws from the same set.

use std::time::Duration;

use eframe::egui;
use egui::{Color32, FontFamily, FontId, RichText};
use serde::{Deserialize, Serialize};

use crate::icons::Icon;
use crate::review_session::AnswerTone;

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

/// How the window is drawn: its colours and the shape of its surfaces. Each
/// style comes in a light and a dark palette, chosen apart from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Style {
    /// Soft cards raised off the page by light and shadow, gently rounded
    /// buttons, and one blue for the main action.
    Soft,
    /// The picture as large as the window allows, flat near-black or paper
    /// around it, the tools down the left, and a yellow main action.
    Studio,
    /// Like a video editor: dark slate panels with hairline edges, small
    /// square-cornered buttons, and teal for the main action. Also what a
    /// settings file from before there were styles means.
    #[default]
    Editor,
}

impl Style {
    pub const ALL: [Self; 3] = [Self::Editor, Self::Soft, Self::Studio];

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::Studio => "Studio",
            Self::Editor => "Editor",
        }
    }

    /// How the review screen is laid out in this style.
    pub fn review_layout(self) -> ReviewLayout {
        match self {
            Self::Editor => ReviewLayout::Editor,
            Self::Soft => ReviewLayout::List,
            Self::Studio => ReviewLayout::Stage,
        }
    }

    pub fn palette(self, theme: egui::Theme) -> &'static Palette {
        match (self, theme) {
            (Self::Soft, egui::Theme::Light) => &LIGHT,
            (Self::Soft, egui::Theme::Dark) => &DARK,
            (Self::Studio, egui::Theme::Light) => &STUDIO_LIGHT,
            (Self::Studio, egui::Theme::Dark) => &STUDIO_DARK,
            (Self::Editor, egui::Theme::Light) => &EDITOR_LIGHT,
            (Self::Editor, egui::Theme::Dark) => &EDITOR_DARK,
        }
    }
}

/// How the review screen is laid out. Each style has its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewLayout {
    /// Like a video editor: a preview beside a table of the scenes, a film
    /// strip under them, and the split at the bottom right.
    Editor,
    /// Every scene as a card with its pictures, one under the other, and
    /// the player in a window of its own.
    List,
    /// A large player across the screen with the scenes as a strip of
    /// pictures under it, and the tools along the side.
    Stage,
}

/// How a style lifts cards off the page.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Depth {
    /// Lit from the top left: a glow above and a shadow below each card.
    /// Buttons stay flat, so the cards are what stands off the page.
    Raised,
    /// No shadows at all. Surfaces differ by colour alone.
    Flat,
}

/// How a style finishes its surfaces. The parts come as a set: raised
/// cards go with rounder controls that have no edge, flat cards with
/// edged controls and small corners, as editors draw them.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Finish {
    Soft,
    Edged,
}

impl Finish {
    fn depth(self) -> Depth {
        match self {
            Finish::Soft => Depth::Raised,
            Finish::Edged => Depth::Flat,
        }
    }

    fn control_rounding(self) -> f32 {
        match self {
            Finish::Soft => SOFT_CONTROL_ROUNDING,
            Finish::Edged => EDGED_CONTROL_ROUNDING,
        }
    }

    /// Buttons, tick boxes and text boxes are drawn with their edge.
    fn edged_controls(self) -> bool {
        self == Finish::Edged
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

    /// Links, selection and marks that belong to the main action. Readable
    /// as text on the page and on cards.
    pub primary: Color32,
    /// The fill of the one main action on a screen, under `on_primary`.
    pub primary_fill: Color32,
    pub on_primary: Color32,
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

    /// Buttons and boxes have no outline. Their fill is a shade apart from
    /// the page and the cards, so they show on both.
    pub control_fill: Color32,
    /// The soft edge of buttons, tick boxes and text boxes.
    control_edge: Color32,
    pub control_hovered_fill: Color32,
    control_pressed_fill: Color32,
    /// Inside text boxes.
    pub input_fill: Color32,
    /// The part of a progress bar that is still to do.
    progress_track: Color32,
    /// How dark the page goes behind a dialog that has to be answered first.
    pub backdrop: Color32,
    /// Below and right of a raised surface.
    shadow: Color32,
    /// Above and left of a raised surface, where the light comes from.
    highlight: Color32,
    finish: Finish,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Soft surfaces on a pale grey page, lit from the top left: a white glow
/// above each raised surface and a grey shadow below it. One blue marks the
/// main action. Scene colours are dark enough for white text.
const LIGHT: Palette = Palette {
    theme: egui::Theme::Light,
    page: rgb(0xE8EBEF),
    sidebar: rgb(0xEDEFF2),
    card: rgb(0xEEF1F4),
    card_border: rgb(0xD5DAE1),
    primary: rgb(0x1565C0),
    primary_fill: rgb(0x1976D2),
    on_primary: Color32::WHITE,
    primary_tint: rgb(0xDCE8F7),
    accent: rgb(0x1E88E5),
    success: rgb(0x0F7553),
    danger: rgb(0xC0303D),
    warning_text: rgb(0xA8390A),
    text: rgb(0x23262B),
    muted_text: rgb(0x555B64),
    on_fill: Color32::WHITE,
    faint_on_fill: Color32::from_rgba_premultiplied(140, 140, 140, 140),
    dropped: rgb(0x555B64),
    dropped_fill: rgb(0xE0E3E8),
    mint_tile: rgb(0xDDEFE8),
    lavender_tile: rgb(0xDCE8F7),
    peach_tile: rgb(0xF5E7DC),
    rose_tile: rgb(0xF8E7E9),
    picture_backdrop: Color32::BLACK,
    focus_ring: rgb(0x23262B),
    focus_ring_inner: Color32::WHITE,
    scene_colors: [
        rgb(0x4F46C8),
        rgb(0xB84A1C),
        rgb(0x13805D),
        rgb(0xB83174),
        rgb(0x1D6AAE),
        rgb(0x855D00),
        rgb(0x8549CC),
        rgb(0x4B5B73),
    ],
    control_fill: rgb(0xDDE2E8),
    control_edge: rgb(0xD0D5DC),
    control_hovered_fill: rgb(0xD4DAE1),
    control_pressed_fill: rgb(0xD2D8E0),
    input_fill: rgb(0xF4F6F8),
    progress_track: rgb(0xD5DAE1),
    backdrop: Color32::from_rgba_premultiplied(14, 16, 22, 110),
    shadow: Color32::from_rgba_premultiplied(40, 50, 66, 60),
    highlight: Color32::from_rgba_premultiplied(255, 255, 255, 230),
    finish: Finish::Soft,
};

/// Near-black charcoal, with surfaces a shade lighter, lit from the top
/// left: a faint glow above each raised surface and a deep shadow below it,
/// so it needs no outline. One blue marks the main action. Scene
/// colours are lighter relatives of the light palette's, same hues, and what
/// is written on them is dark.
const DARK: Palette = Palette {
    theme: egui::Theme::Dark,
    page: rgb(0x232529),
    sidebar: rgb(0x26292D),
    card: rgb(0x2B2E33),
    card_border: rgb(0x3A3E44),
    primary: rgb(0x5AAEFF),
    primary_fill: rgb(0x1976D2),
    on_primary: Color32::WHITE,
    primary_tint: rgb(0x1F3247),
    accent: rgb(0x2196F3),
    success: rgb(0x5CD3A3),
    danger: rgb(0xFF8E97),
    warning_text: rgb(0xF5AE74),
    text: rgb(0xEEEFF1),
    muted_text: rgb(0xB6BAC2),
    on_fill: rgb(0x17191C),
    faint_on_fill: Color32::from_rgba_premultiplied(11, 12, 14, 140),
    dropped: rgb(0xA9AEB6),
    dropped_fill: rgb(0x27292D),
    mint_tile: rgb(0x203A33),
    lavender_tile: rgb(0x1F3247),
    peach_tile: rgb(0x3A3027),
    rose_tile: rgb(0x3D272C),
    picture_backdrop: Color32::BLACK,
    focus_ring: rgb(0xEEEFF1),
    focus_ring_inner: rgb(0x17191C),
    scene_colors: [
        rgb(0x9D97FF),
        rgb(0xF0905F),
        rgb(0x55CC9C),
        rgb(0xF085B8),
        rgb(0x67B1F2),
        rgb(0xD9AE45),
        rgb(0xC398F5),
        rgb(0xA5B5CB),
    ],
    control_fill: rgb(0x31353A),
    control_edge: rgb(0x3A3E44),
    control_hovered_fill: rgb(0x383C42),
    control_pressed_fill: rgb(0x26292D),
    input_fill: rgb(0x1E2023),
    progress_track: rgb(0x1B1D20),
    backdrop: Color32::from_rgba_premultiplied(0, 0, 0, 140),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 150),
    highlight: Color32::from_rgba_premultiplied(9, 9, 10, 10),
    finish: Finish::Soft,
};

/// Paper and white pills, black for the main action with yellow words.
const STUDIO_LIGHT: Palette = Palette {
    page: rgb(0xEFEDE3),
    sidebar: rgb(0xF6F5EE),
    card: Color32::WHITE,
    card_border: rgb(0xE3E0D3),
    primary: rgb(0x5E5300),
    primary_fill: rgb(0x17181A),
    on_primary: rgb(0xF3E650),
    primary_tint: rgb(0xF4EEC4),
    accent: rgb(0xD9C51E),
    text: rgb(0x1B1C1F),
    muted_text: rgb(0x54524A),
    dropped: rgb(0x54524A),
    dropped_fill: rgb(0xE9E7DE),
    mint_tile: rgb(0xE2F0E6),
    lavender_tile: rgb(0xF4EEC4),
    peach_tile: rgb(0xF6E9DA),
    rose_tile: rgb(0xF8E7E9),
    control_fill: rgb(0xE6E3D6),
    control_edge: rgb(0xD6D2C2),
    control_hovered_fill: rgb(0xDEDACB),
    control_pressed_fill: rgb(0xD3CFBE),
    input_fill: Color32::WHITE,
    progress_track: rgb(0xE3E0D3),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 0),
    highlight: Color32::TRANSPARENT,
    finish: Finish::Edged,
    ..LIGHT
};

/// Near-black panels, light grey pills, and a yellow main action.
const STUDIO_DARK: Palette = Palette {
    page: rgb(0x101113),
    sidebar: rgb(0x151618),
    card: rgb(0x1C1D20),
    card_border: rgb(0x2C2D32),
    primary: rgb(0xEADF4C),
    primary_fill: rgb(0xF1E54F),
    on_primary: rgb(0x141414),
    primary_tint: rgb(0x34321A),
    accent: rgb(0xF1E54F),
    text: rgb(0xF5F5F2),
    muted_text: rgb(0xB5B6BA),
    dropped_fill: rgb(0x18191B),
    mint_tile: rgb(0x17302A),
    lavender_tile: rgb(0x34321A),
    peach_tile: rgb(0x33281C),
    rose_tile: rgb(0x37201F),
    control_fill: rgb(0x27282C),
    control_edge: rgb(0x323338),
    control_hovered_fill: rgb(0x303136),
    control_pressed_fill: rgb(0x1F2023),
    input_fill: rgb(0x0C0D0E),
    progress_track: rgb(0x2A2B30),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 0),
    highlight: Color32::TRANSPARENT,
    finish: Finish::Edged,
    ..DARK
};

/// White panels with hairline edges on a cool grey page, and teal.
const EDITOR_LIGHT: Palette = Palette {
    page: rgb(0xECF0F1),
    sidebar: rgb(0xF5F7F8),
    card: Color32::WHITE,
    card_border: rgb(0xD5DDE0),
    primary: rgb(0x0B6F66),
    primary_fill: rgb(0x0B7A70),
    on_primary: Color32::WHITE,
    primary_tint: rgb(0xDDF1EE),
    accent: rgb(0x14A394),
    text: rgb(0x1B2427),
    muted_text: rgb(0x4F5D62),
    dropped: rgb(0x4F5D62),
    dropped_fill: rgb(0xE4E9EB),
    mint_tile: rgb(0xDFF1EA),
    lavender_tile: rgb(0xDDF1EE),
    peach_tile: rgb(0xFBEBDD),
    rose_tile: rgb(0xFBE6E8),
    control_fill: rgb(0xF4F7F8),
    control_edge: rgb(0xC9D3D7),
    control_hovered_fill: rgb(0xE7EDEF),
    control_pressed_fill: rgb(0xDDF1EE),
    input_fill: Color32::WHITE,
    progress_track: rgb(0xDCE3E6),
    shadow: Color32::from_rgba_premultiplied(8, 14, 16, 26),
    highlight: Color32::TRANSPARENT,
    finish: Finish::Edged,
    ..LIGHT
};

/// Dark slate panels with hairline edges, as video editors are, and teal
/// with dark words for the main action.
const EDITOR_DARK: Palette = Palette {
    page: rgb(0x0E1315),
    sidebar: rgb(0x121A1D),
    card: rgb(0x152024),
    card_border: rgb(0x25343A),
    primary: rgb(0x4FD8C8),
    primary_fill: rgb(0x3FD0BF),
    on_primary: rgb(0x08191A),
    primary_tint: rgb(0x123735),
    accent: rgb(0x3FD0BF),
    text: rgb(0xE9EFF1),
    muted_text: rgb(0xA3B3B8),
    dropped_fill: rgb(0x131B1E),
    mint_tile: rgb(0x13322E),
    lavender_tile: rgb(0x123735),
    peach_tile: rgb(0x33291E),
    rose_tile: rgb(0x371F24),
    control_fill: rgb(0x18252A),
    control_edge: rgb(0x2D3F46),
    control_hovered_fill: rgb(0x1F3036),
    control_pressed_fill: rgb(0x123735),
    input_fill: rgb(0x0B1012),
    progress_track: rgb(0x22333A),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 120),
    highlight: Color32::TRANSPARENT,
    finish: Finish::Edged,
    ..DARK
};

impl Palette {
    /// The colour of a scene by its place in the video. A scene keeps it
    /// everywhere it appears.
    pub fn scene_color(&self, scene: usize) -> Color32 {
        self.scene_colors[scene % SCENE_COLOR_COUNT]
    }

    /// A card with a wash of a scene's colour over it.
    pub fn tinted_card(&self, scene_color: Color32, tint: CardTint) -> Color32 {
        wash(self.card, scene_color, tint.strength())
    }

    /// How round the corners of buttons and boxes are.
    pub fn control_rounding(&self) -> f32 {
        self.finish.control_rounding()
    }

    /// The shadow below and right of a raised surface.
    pub fn soft_shadow(&self) -> egui::Shadow {
        egui::Shadow {
            offset: egui::vec2(RAISE_OFFSET, RAISE_OFFSET),
            blur: RAISE_BLUR,
            spread: 0.0,
            color: self.shadow,
        }
    }

    /// The glow above and left of a raised surface.
    fn soft_highlight(&self) -> egui::Shadow {
        egui::Shadow {
            offset: egui::vec2(-RAISE_OFFSET, -RAISE_OFFSET),
            blur: RAISE_BLUR,
            spread: 0.0,
            color: self.highlight,
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

/// The palette the window is drawn in during this frame: the light or dark
/// one of the style in use.
pub fn palette(ctx: &egui::Context) -> &'static Palette {
    style_in_use(ctx).palette(ctx.theme())
}

fn style_in_use(ctx: &egui::Context) -> Style {
    ctx.data(|data| data.get_temp(egui::Id::new(STYLE_ID)))
        .unwrap_or_default()
}

/// How long a "Copy details" button says that it has copied.
const COPIED_NOTE_SECONDS: f64 = 2.0;
pub const COPY_DETAILS_LABEL: &str = "Copy details";
const COPIED_LABEL: &str = "Copied";
/// Where the context keeps which title bar was last asked for.
const TITLE_BAR_CHOICE_ID: &str = "title_bar_theme_choice";
/// Where the context keeps the style the window is dressed in.
const STYLE_ID: &str = "window_style";

pub const CARD_ROUNDING: f32 = 14.0;
/// The height of buttons, text boxes and the rows they sit in.
pub const CONTROL_HEIGHT: f32 = 32.0;
/// The soft style's controls are a little rounder than the others, never
/// pills: a row of pills reads as a row of tags, not of buttons.
const SOFT_CONTROL_ROUNDING: f32 = 8.0;
/// Buttons and boxes with an edge have small corners, as editors draw them.
const EDGED_CONTROL_ROUNDING: f32 = 6.0;
const TOOL_BUTTON_SIZE: f32 = 36.0;
const LARGE_TOOL_BUTTON_SIZE: f32 = 40.0;
const LARGE_BUTTON_ICON_SIZE: f32 = 18.0;
const DIALOG_ROUNDING: f32 = 12.0;
pub const PANEL_MARGIN: f32 = 16.0;
const PILL_ROUNDING: f32 = 10.0;
const TILE_VALUE_SIZE: f32 = 20.0;
const TILE_ICON_SIZE: f32 = 14.0;
/// Between one key number and the next in their row.
const TILE_GAP: f32 = 22.0;
const CARD_MARGIN: f32 = 16.0;
/// How far a raised surface's glow and shadow reach from it, and how soft
/// they are. Small controls get a closer, sharper pair.
const RAISE_OFFSET: f32 = 6.0;
const RAISE_BLUR: f32 = 18.0;
const PRIMARY_BUTTON_PADDING: f32 = 14.0;
/// How much deeper a filled button gets under the pointer.
const HOVER_DEEPEN: f32 = 0.12;
/// An icon beside the words on a button.
const BUTTON_ICON_SIZE: f32 = 14.0;
const BUTTON_ICON_GAP: f32 = 6.0;
/// Space from the edge of a button with an icon to its icon and words. A
/// little less than a plain button has, since the icon makes the button wider.
const ICON_BUTTON_PADDING: f32 = 11.0;
const SECTION_LABEL_SIZE: f32 = 11.5;
const APP_BAR_MARK_SIZE: f32 = 24.0;
const APP_BAR_NAME_SIZE: f32 = 16.0;
const APP_BAR_PADDING: f32 = 6.0;
const CHOICE_ICON_SIZE: f32 = 24.0;
const CAPTION_SIZE: f32 = 12.0;
/// The words on buttons.
const BUTTON_FONT_SIZE: f32 = 14.0;
/// How much deeper a filled button gets while it is pressed.
const PRESS_DEEPEN: f32 = 0.22;
const SCENE_BUTTON_HEIGHT: f32 = 28.0;
const SCENE_BUTTON_FONT_SIZE: f32 = 13.0;

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
    dress(ctx, Style::default());
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
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

/// Dresses the window in `style` from the next frame on. Cheap enough to
/// call on every frame: it changes the toolkit's look only when the style
/// does.
pub fn dress_in(ctx: &egui::Context, style: Style) {
    if style_in_use(ctx) != style {
        dress(ctx, style);
    }
}

fn dress(ctx: &egui::Context, style: Style) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(STYLE_ID), style));
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let palette = style.palette(theme);
        ctx.style_mut_of(theme, |egui_style| egui_style.visuals = visuals(palette));
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
    // Dialogs are cards with an edge, lifted well off what they cover.
    visuals.window_rounding = DIALOG_ROUNDING.into();
    visuals.window_stroke = egui::Stroke::new(1.0_f32, palette.card_border);
    visuals.window_shadow = egui::Shadow {
        offset: egui::vec2(0.0, 12.0),
        blur: 40.0,
        spread: 0.0,
        color: palette.backdrop,
    };
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
    // A soft edge barely a shade from the fill: enough for a tick box or a
    // text box to show on the page, too little to read as an outline.
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, palette.control_edge);
    if palette.finish.edged_controls() {
        widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, palette.control_edge);
    }
    widgets.hovered.weak_bg_fill = palette.control_hovered_fill;
    widgets.hovered.bg_fill = palette.control_hovered_fill;
    widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, palette.primary);
    widgets.active.weak_bg_fill = palette.control_pressed_fill;
    widgets.active.bg_fill = palette.control_pressed_fill;
    // A box whose list is open, which the toolkit would leave in its own grey.
    widgets.open.weak_bg_fill = palette.control_hovered_fill;
    widgets.open.bg_fill = palette.control_hovered_fill;
    widgets.open.bg_stroke = egui::Stroke::NONE;
    widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, palette.primary);
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.fg_stroke.color = palette.text;
        widget.rounding = palette.control_rounding().into();
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

/// How wide `text` is on one line, for showing it only when it fits whole:
/// a line cut short reads as a fault.
pub fn text_width(ui: &egui::Ui, text: &RichText) -> f32 {
    egui::WidgetText::from(text.clone())
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x
}

/// The colour of an answer to something the person did, on the page: a
/// refusal in the colour of a warning, so it is not taken for a success.
pub fn answer_color(palette: &Palette, tone: AnswerTone, ordinary: Color32) -> Color32 {
    match tone {
        AnswerTone::Done => ordinary,
        AnswerTone::Refused => palette.warning_text,
    }
}

/// A note drawn over the picture, which may be any colour at all, so the
/// note has colours of its own that do not follow the palette: nearly
/// opaque black under light text.
pub const PICTURE_NOTE_FILL: Color32 = Color32::from_black_alpha(220);
pub const PICTURE_NOTE_TEXT: Color32 = Color32::from_gray(240);
pub const PICTURE_NOTE_WARNING: Color32 = Color32::from_rgb(255, 196, 120);
pub const PICTURE_NOTE_PROBLEM: Color32 = Color32::from_rgb(255, 150, 150);
pub const PICTURE_NOTE_FONT_SIZE: f32 = 13.0;

/// The colour of an answer written on the picture, as `answer_color` is on
/// the page.
pub fn picture_note_color(tone: AnswerTone) -> Color32 {
    match tone {
        AnswerTone::Done => PICTURE_NOTE_TEXT,
        AnswerTone::Refused => PICTURE_NOTE_WARNING,
    }
}

/// A rounded card raised off the page: lit from the top left, with a glow
/// above it and a shadow below, and no outline.
pub fn card(ui: &egui::Ui) -> Raised {
    let palette = palette(ui.ctx());
    Raised(
        egui::Frame::none()
            .fill(palette.card)
            .rounding(CARD_ROUNDING)
            .inner_margin(egui::Margin::same(CARD_MARGIN)),
    )
}

/// A frame drawn raised off what is under it. egui gives a frame one
/// shadow, so the glow on the lit side is painted into a place kept for it
/// beneath the frame, once the frame knows its size.
pub struct Raised(pub egui::Frame);

impl Raised {
    pub fn show<R>(
        self,
        ui: &mut egui::Ui,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let palette = palette(ui.ctx());
        let glow_place = ui.painter().add(egui::Shape::Noop);
        let frame = match palette.finish.depth() {
            Depth::Raised => self.0.shadow(palette.soft_shadow()),
            Depth::Flat => self.0,
        };
        let rounding = frame.rounding;
        let shown = frame.show(ui, add_contents);
        if palette.finish.depth() == Depth::Raised {
            let glow = palette
                .soft_highlight()
                .as_shape(shown.response.rect, rounding);
            ui.painter().set(glow_place, glow);
        }
        shown
    }
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

/// What a key number shows: a small icon in `icon_color` beside its
/// caption, and the number in `value_color` under them.
pub struct StatTile<'a> {
    pub icon: Icon,
    pub icon_color: Color32,
    pub value: &'a str,
    pub value_color: Color32,
    pub caption: &'a str,
}

/// One key number, set large, with a small caption and icon above it. No
/// box: the numbers are read, not pressed, so they stay quiet.
pub fn stat_tile(ui: &mut egui::Ui, tile: StatTile<'_>) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
        ui.horizontal(|ui| {
            let icon_size = egui::vec2(TILE_ICON_SIZE, TILE_ICON_SIZE);
            let (icon_place, _) = ui.allocate_exact_size(icon_size, egui::Sense::hover());
            tile.icon.paint(ui.painter(), icon_place, tile.icon_color);
            ui.label(caption(ui, tile.caption));
        });
        ui.label(semibold(tile.value, TILE_VALUE_SIZE).color(tile.value_color));
    });
    ui.add_space(TILE_GAP);
}

/// A small heading in capitals over a group, as "VIDEOS" in the sidebar.
pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(
        semibold(text.to_uppercase(), SECTION_LABEL_SIZE)
            .color(palette(ui.ctx()).muted_text)
            .extra_letter_spacing(0.6),
    );
}

/// A large button of a fixed size with an icon, a title and a line under
/// it, for a choice between a few things, as in the dialog to add videos.
pub fn choice_tile(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    icon: Icon,
    title: &str,
    detail: &str,
) -> egui::Response {
    let palette = palette(ui.ctx());
    let (place, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    let hovered = response.hovered();
    let fill = if hovered {
        palette.control_hovered_fill
    } else {
        palette.control_fill
    };
    let edge_color = if hovered {
        palette.primary
    } else {
        palette.control_edge
    };
    let painter = ui.painter();
    painter.rect(
        place,
        CARD_ROUNDING,
        fill,
        egui::Stroke::new(1.0_f32, edge_color),
    );
    let icon_place = egui::Rect::from_center_size(
        place.center_top() + egui::vec2(0.0, 28.0),
        egui::vec2(CHOICE_ICON_SIZE, CHOICE_ICON_SIZE),
    );
    icon.paint(painter, icon_place, palette.primary);
    painter.text(
        place.center_top() + egui::vec2(0.0, 52.0),
        egui::Align2::CENTER_TOP,
        title,
        semibold_font(15.0),
        palette.text,
    );
    painter.text(
        place.center_top() + egui::vec2(0.0, 72.0),
        egui::Align2::CENTER_TOP,
        detail,
        FontId::proportional(CAPTION_SIZE),
        palette.muted_text,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The frame of the bar across the top of the window.
pub fn app_bar_panel(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::none()
        .fill(palette(ctx).sidebar)
        .inner_margin(egui::Margin::symmetric(PANEL_MARGIN, APP_BAR_PADDING))
}

/// The app's mark and its name in one line, for the bar across the top.
pub fn brand_mark_and_name(ui: &mut egui::Ui, name: &str) {
    let mark = crate::icons::mark_texture(ui.ctx());
    let mark_size = egui::vec2(APP_BAR_MARK_SIZE, APP_BAR_MARK_SIZE);
    ui.add(egui::Image::new(&mark).fit_to_exact_size(mark_size));
    ui.label(semibold(name, APP_BAR_NAME_SIZE).color(palette(ui.ctx()).text));
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
        .rounding(palette.control_rounding())
}

/// A switch for a setting that is on or off. It draws no words: the row
/// it sits in names the setting on the left, and `label` names it to
/// screen readers and tests.
pub struct Toggle<'a> {
    on: &'a mut bool,
    label: &'a str,
}

impl<'a> Toggle<'a> {
    pub fn new(on: &'a mut bool, label: &'a str) -> Self {
        Self { on, label }
    }
}

pub const TOGGLE_SIZE: egui::Vec2 = egui::vec2(38.0, 22.0);

impl egui::Widget for Toggle<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let palette = palette(ui.ctx());
        let (place, mut response) = ui.allocate_exact_size(TOGGLE_SIZE, egui::Sense::click());
        if response.clicked() {
            *self.on = !*self.on;
            response.mark_changed();
        }
        let on = *self.on;
        let enabled = ui.is_enabled();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, self.label)
        });
        if !ui.is_rect_visible(place) {
            return response;
        }
        let how_far_on = ui.ctx().animate_bool(response.id, on);
        let radius = place.height() / 2.0;
        let (track, track_edge, knob) = if on {
            (
                palette.primary_fill,
                palette.primary_fill,
                palette.on_primary,
            )
        } else if response.hovered() && enabled {
            (
                palette.control_hovered_fill,
                palette.card_border,
                palette.text,
            )
        } else {
            (
                palette.control_fill,
                palette.card_border,
                palette.muted_text,
            )
        };
        let painter = ui.painter();
        painter.rect(place, radius, track, egui::Stroke::new(1.0_f32, track_edge));
        let knob_x = egui::lerp(
            (place.left() + radius)..=(place.right() - radius),
            how_far_on,
        );
        painter.circle_filled(egui::pos2(knob_x, place.center().y), radius - 4.0, knob);
        response
    }
}

/// The one button on a screen that carries out its main action, without
/// an icon.
pub fn primary_button(text: impl Into<String>) -> IconButton {
    IconButton {
        icon: None,
        text: text.into(),
        kind: ButtonKind::Primary,
        least_width: 0.0,
    }
}

/// How much a button stands out. A screen has one primary button, so the
/// eye finds the main action first. What changes the scenes is plain, and
/// what is used now and then stays quiet until the pointer is on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ButtonKind {
    /// A flat button in the shade of the controls.
    Plain,
    /// The one button on a screen that carries out its main action, filled
    /// with the style's main colour. It is told apart by its colour, not
    /// its size, so it lines up with the buttons beside it.
    Primary,
    /// Only an icon and words, in grey, with the shade of the controls
    /// under them while the pointer is on it.
    Quiet,
}

/// What the pointer is doing to a button.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Interaction {
    Idle,
    Hovered,
    Pressed,
}

impl Interaction {
    /// A button that is turned off answers nothing.
    fn of(response: &egui::Response, enabled: bool) -> Self {
        if !enabled {
            Interaction::Idle
        } else if response.is_pointer_button_down_on() {
            Interaction::Pressed
        } else if response.hovered() {
            Interaction::Hovered
        } else {
            Interaction::Idle
        }
    }
}

/// How a button is painted: what fills it, the colour of its icon and
/// words, and its edge.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ControlLook {
    fill: Color32,
    ink: Color32,
    edge: egui::Stroke,
}

impl ControlLook {
    fn paint(self, painter: &egui::Painter, place: egui::Rect, palette: &Palette) {
        painter.rect(place, palette.control_rounding(), self.fill, self.edge);
    }
}

/// The one rule for how every kind of button looks, whatever draws it.
fn control_look(palette: &Palette, kind: ButtonKind, interaction: Interaction) -> ControlLook {
    let edge = if palette.finish.edged_controls() {
        egui::Stroke::new(1.0_f32, palette.control_edge)
    } else {
        egui::Stroke::NONE
    };
    let shaded = |fill| ControlLook {
        fill,
        ink: palette.text,
        edge,
    };
    match (kind, interaction) {
        (ButtonKind::Primary, interaction) => ControlLook {
            fill: match interaction {
                Interaction::Idle => palette.primary_fill,
                Interaction::Hovered => deeper(palette.primary_fill),
                Interaction::Pressed => wash(palette.primary_fill, Color32::BLACK, PRESS_DEEPEN),
            },
            ink: palette.on_primary,
            edge: egui::Stroke::NONE,
        },
        (ButtonKind::Plain, Interaction::Idle) => shaded(palette.control_fill),
        (ButtonKind::Quiet, Interaction::Idle) => ControlLook {
            fill: Color32::TRANSPARENT,
            ink: palette.muted_text,
            edge: egui::Stroke::NONE,
        },
        (ButtonKind::Plain | ButtonKind::Quiet, Interaction::Hovered) => {
            shaded(palette.control_hovered_fill)
        }
        (ButtonKind::Plain | ButtonKind::Quiet, Interaction::Pressed) => {
            shaded(palette.control_pressed_fill)
        }
    }
}

/// A button with an icon before its words. It tells screen readers its
/// words like any button does. A key that does the same belongs in its
/// hover text, so the button says one thing.
pub struct IconButton {
    icon: Option<Icon>,
    text: String,
    kind: ButtonKind,
    /// At least this wide, with the icon and words in the middle.
    least_width: f32,
}

impl IconButton {
    pub fn new(icon: Icon, text: impl Into<String>, kind: ButtonKind) -> Self {
        Self {
            icon: Some(icon),
            text: text.into(),
            kind,
            least_width: 0.0,
        }
    }

    /// Makes the button at least `width` wide, as a button that spans its
    /// column is, with what it shows in the middle.
    pub fn least_width(mut self, width: f32) -> Self {
        self.least_width = width;
        self
    }

    /// The width of the icon and the words together.
    fn content_width(&self, text_width: f32) -> f32 {
        let icon = match self.icon {
            Some(_) => BUTTON_ICON_SIZE + BUTTON_ICON_GAP,
            None => 0.0,
        };
        icon + text_width
    }
}

impl egui::Widget for IconButton {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let palette = palette(ui.ctx());
        let (font, padding) = match self.kind {
            ButtonKind::Plain | ButtonKind::Quiet => {
                (FontId::proportional(BUTTON_FONT_SIZE), ICON_BUTTON_PADDING)
            }
            ButtonKind::Primary => (semibold_font(BUTTON_FONT_SIZE), PRIMARY_BUTTON_PADDING),
        };
        // Laid out without a colour: the look picks it once the pointer is known.
        let galley = ui
            .painter()
            .layout_no_wrap(self.text.clone(), font, Color32::PLACEHOLDER);
        let content_width = self.content_width(galley.size().x);
        let width = (padding * 2.0 + content_width).max(self.least_width);
        let (place, response) =
            ui.allocate_exact_size(egui::vec2(width, CONTROL_HEIGHT), egui::Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &self.text)
        });
        if !ui.is_rect_visible(place) {
            return response;
        }

        let look = control_look(palette, self.kind, Interaction::of(&response, enabled));
        let ink = look.ink;
        let painter = ui.painter();
        look.paint(painter, place, palette);

        let mut content_left = place.center().x - content_width / 2.0;
        if let Some(icon) = self.icon {
            let icon_center = egui::pos2(content_left + BUTTON_ICON_SIZE / 2.0, place.center().y);
            let icon_place = egui::Rect::from_center_size(
                icon_center,
                egui::vec2(BUTTON_ICON_SIZE, BUTTON_ICON_SIZE),
            );
            icon.paint(painter, icon_place, ink);
            content_left += BUTTON_ICON_SIZE + BUTTON_ICON_GAP;
        }
        let text_top = place.center().y - galley.size().y / 2.0;
        painter.galley(egui::pos2(content_left, text_top), galley, ink);
        response
    }
}

/// A square button with only an icon, for a column or row of tools. Its
/// name is what screen readers say.
pub struct ToolButton {
    icon: Icon,
    name: String,
    kind: ButtonKind,
    size: ToolButtonSize,
}

/// How large a tool button is. The large one is for the one action a row
/// of them is about, such as play among the ways to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolButtonSize {
    Small,
    Ordinary,
    Large,
}

impl ToolButtonSize {
    fn side(self) -> f32 {
        match self {
            ToolButtonSize::Small => CONTROL_HEIGHT,
            ToolButtonSize::Ordinary => TOOL_BUTTON_SIZE,
            ToolButtonSize::Large => LARGE_TOOL_BUTTON_SIZE,
        }
    }

    fn icon_side(self) -> f32 {
        match self {
            ToolButtonSize::Small | ToolButtonSize::Ordinary => BUTTON_ICON_SIZE,
            ToolButtonSize::Large => LARGE_BUTTON_ICON_SIZE,
        }
    }
}

impl ToolButton {
    pub fn new(icon: Icon, name: impl Into<String>, kind: ButtonKind) -> Self {
        Self {
            icon,
            name: name.into(),
            kind,
            size: ToolButtonSize::Ordinary,
        }
    }

    pub fn size(mut self, size: ToolButtonSize) -> Self {
        self.size = size;
        self
    }

    /// How wide the button is, for laying out a row of them.
    pub fn width(&self) -> f32 {
        self.size.side()
    }
}

impl egui::Widget for ToolButton {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let palette = palette(ui.ctx());
        let side = self.size.side();
        let size = egui::vec2(side, side);
        let (place, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &self.name)
        });
        if !ui.is_rect_visible(place) {
            return response;
        }
        let look = control_look(palette, self.kind, Interaction::of(&response, enabled));
        let painter = ui.painter();
        look.paint(painter, place, palette);
        let icon_side = self.size.icon_side();
        let icon_place =
            egui::Rect::from_center_size(place.center(), egui::vec2(icon_side, icon_side));
        self.icon.paint(painter, icon_place, look.ink);
        response
    }
}

/// A small square button with three dots that opens a menu of `add_contents`,
/// for things used less often than the buttons beside it. `ui.close_menu()`
/// closes it, as in any egui menu.
pub fn more_menu<R>(
    ui: &mut egui::Ui,
    hint: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let palette = palette(ui.ctx());
    let side = ToolButtonSize::Small.side();
    // egui's menus open from one of its own buttons. This one draws
    // nothing, and the dots are painted over it.
    let blank = egui::Button::new("")
        .frame(false)
        .min_size(egui::vec2(side, side));
    let shown = egui::menu::menu_custom_button(ui, blank, add_contents);
    let place = shown.response.rect;
    // While its menu is open it looks as it does under the pointer.
    let interaction = match Interaction::of(&shown.response, ui.is_enabled()) {
        Interaction::Idle if shown.inner.is_some() => Interaction::Hovered,
        interaction => interaction,
    };
    let look = control_look(palette, ButtonKind::Quiet, interaction);
    look.paint(ui.painter(), place, palette);
    let icon_place = egui::Rect::from_center_size(
        place.center(),
        egui::vec2(BUTTON_ICON_SIZE, BUTTON_ICON_SIZE),
    );
    Icon::More.paint(ui.painter(), icon_place, look.ink);
    shown.response.on_hover_text(hint);
    shown.inner
}

/// A scene's button: its number and length, filled with the main colour
/// while the scene is chosen. Screen readers are told whether it is.
pub struct SceneButton {
    words: String,
    chosen: bool,
}

impl SceneButton {
    pub fn new(words: impl Into<String>) -> Self {
        Self {
            words: words.into(),
            chosen: false,
        }
    }

    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }
}

impl egui::Widget for SceneButton {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let palette = palette(ui.ctx());
        let galley = ui.painter().layout_no_wrap(
            self.words.clone(),
            semibold_font(SCENE_BUTTON_FONT_SIZE),
            Color32::PLACEHOLDER,
        );
        let size = egui::vec2(
            galley.size().x + 2.0 * ICON_BUTTON_PADDING,
            SCENE_BUTTON_HEIGHT,
        );
        let (place, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, self.chosen, &self.words)
        });
        if !ui.is_rect_visible(place) {
            return response;
        }
        let kind = if self.chosen {
            ButtonKind::Primary
        } else {
            ButtonKind::Plain
        };
        let look = control_look(palette, kind, Interaction::of(&response, enabled));
        look.paint(ui.painter(), place, palette);
        let text_top_left = place.center() - galley.size() / 2.0;
        ui.painter().galley(text_top_left, galley, look.ink);
        response
    }
}

/// `hint` with the key that does the same, as the system names it, such as
/// "Takes back the last change (Ctrl+Z)", or Cmd on a Mac.
pub fn hint_with_key(ctx: &egui::Context, hint: &str, key: &egui::KeyboardShortcut) -> String {
    format!("{hint} ({})", ctx.format_shortcut(key))
}

/// A colour a little deeper, for a filled button under the pointer. Deeper,
/// not lighter, so its white words stay readable.
fn deeper(color: Color32) -> Color32 {
    wash(color, Color32::BLACK, HOVER_DEEPEN)
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

    const PALETTES: [&Palette; 6] = [
        &LIGHT,
        &DARK,
        &STUDIO_LIGHT,
        &STUDIO_DARK,
        &EDITOR_LIGHT,
        &EDITOR_DARK,
    ];

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
    /// A button without an outline is told apart by its fill alone.
    const VISIBLE_CONTROL: f64 = 1.08;

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
            // The main action, and its lighter look under the pointer.
            assert_readable(palette, palette.on_primary, palette.primary_fill);
            assert_readable(palette, palette.on_primary, deeper(palette.primary_fill));
        }
    }

    #[test]
    fn every_button_can_be_read_whatever_the_pointer_does() {
        let kinds = [ButtonKind::Plain, ButtonKind::Primary, ButtonKind::Quiet];
        let interactions = [
            Interaction::Idle,
            Interaction::Hovered,
            Interaction::Pressed,
        ];
        for palette in PALETTES {
            for kind in kinds {
                for interaction in interactions {
                    let look = control_look(palette, kind, interaction);
                    // A quiet button at rest shows what it sits on.
                    let under = if look.fill == Color32::TRANSPARENT {
                        palette.card
                    } else {
                        look.fill
                    };
                    assert_readable(palette, look.ink, under);
                }
            }
        }
    }

    #[test]
    fn a_quiet_button_shows_a_fill_only_under_the_pointer() {
        for palette in PALETTES {
            let at_rest = control_look(palette, ButtonKind::Quiet, Interaction::Idle);
            let hovered = control_look(palette, ButtonKind::Quiet, Interaction::Hovered);
            assert_eq!(at_rest.fill, Color32::TRANSPARENT);
            assert_eq!(hovered.fill, palette.control_hovered_fill);
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

    /// The note's fill laid over `picture`, as it is drawn.
    fn note_fill_over(picture: Color32) -> Color32 {
        let cover = f64::from(PICTURE_NOTE_FILL.a()) / 255.0;
        let channel = |value: u8| (f64::from(value) * (1.0 - cover)).round() as u8;
        Color32::from_rgb(
            channel(picture.r()),
            channel(picture.g()),
            channel(picture.b()),
        )
    }

    #[test]
    fn a_note_on_the_picture_is_readable_over_the_brightest_picture() {
        for picture in [Color32::BLACK, Color32::WHITE] {
            let fill = note_fill_over(picture);
            for text in [
                PICTURE_NOTE_TEXT,
                PICTURE_NOTE_WARNING,
                PICTURE_NOTE_PROBLEM,
            ] {
                assert!(
                    contrast(text, fill) >= READABLE,
                    "{text:?} on {fill:?} is {:.2} to 1",
                    contrast(text, fill)
                );
            }
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
            assert!(
                contrast(palette.card_border, palette.card) >= VISIBLE_HAIRLINE,
                "{:?}: the hairline of a card",
                palette.theme
            );
            // A tick box is an empty edge until it is ticked.
            assert!(
                contrast(palette.control_edge, palette.page) >= VISIBLE_HAIRLINE,
                "{:?}: the edge of a tick box",
                palette.theme
            );
            // A button shows by its edge where it has one, by its fill alone
            // where it has none.
            for surrounding in [palette.card, palette.page, palette.sidebar] {
                let (shown_by, enough) = if palette.finish.edged_controls() {
                    (palette.control_edge, VISIBLE_HAIRLINE)
                } else {
                    (palette.control_fill, VISIBLE_CONTROL)
                };
                assert!(
                    contrast(shown_by, surrounding) >= enough,
                    "{:?}: a button on {surrounding:?}",
                    palette.theme
                );
            }
            assert!(contrast(palette.focus_ring, palette.focus_ring_inner) >= READABLE);
            assert!(contrast(palette.focus_ring, palette.page) >= READABLE);
        }
    }

    #[test]
    fn a_dark_card_is_lighter_than_the_dark_page_and_its_hairline_lighter_still() {
        for style in Style::ALL {
            let dark = style.palette(egui::Theme::Dark);
            assert!(luminance(dark.card) > luminance(dark.page), "{style:?}");
            assert!(luminance(dark.sidebar) > luminance(dark.page), "{style:?}");
            assert!(
                luminance(dark.card_border) > luminance(dark.card),
                "{style:?}"
            );
        }
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
                stat_tile(
                    ui,
                    StatTile {
                        icon: Icon::Film,
                        icon_color: palette.success,
                        value: "3",
                        value_color: palette.text,
                        caption: "scenes",
                    },
                );
                section_label(ui, "Videos");
                brand_mark_and_name(ui, "Cutaway");
                let _ = ui.add(IconButton::new(
                    Icon::Scissors,
                    "Split",
                    ButtonKind::Primary,
                ));
                let _ = ui.add(IconButton::new(Icon::Undo, "Undo", ButtonKind::Plain));
                card(ui).show(ui, |ui| ui.label(caption(ui, "a caption")));
                let _ = ui.add(primary_button("Split"));
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
        assert_eq!(palette(&ctx), Style::default().palette(egui::Theme::Dark));
        assert_eq!(
            ctx.style().visuals.panel_fill,
            Style::default().palette(egui::Theme::Dark).page
        );

        show_in(&ctx, ThemeChoice::Light);
        draw_a_frame(&ctx, Some(egui::Theme::Dark));
        assert_eq!(palette(&ctx), Style::default().palette(egui::Theme::Light));
        assert_eq!(
            ctx.style().visuals.panel_fill,
            Style::default().palette(egui::Theme::Light).page
        );
    }

    #[test]
    fn the_chosen_style_dresses_the_window_in_its_own_palettes() {
        let ctx = egui::Context::default();
        apply(&ctx);
        show_in(&ctx, ThemeChoice::Dark);
        for style in Style::ALL {
            dress_in(&ctx, style);
            draw_a_frame(&ctx, None);
            assert_eq!(palette(&ctx), style.palette(egui::Theme::Dark));
            assert_eq!(
                ctx.style().visuals.panel_fill,
                style.palette(egui::Theme::Dark).page
            );
        }
    }

    #[test]
    fn every_style_has_a_name_of_its_own() {
        let names: std::collections::HashSet<_> = Style::ALL.map(Style::label).into();
        assert_eq!(names.len(), Style::ALL.len());
    }

    #[test]
    fn following_the_system_changes_palette_when_the_system_does() {
        let ctx = egui::Context::default();
        apply(&ctx);
        show_in(&ctx, ThemeChoice::FollowSystem);

        draw_a_frame(&ctx, Some(egui::Theme::Dark));
        assert_eq!(palette(&ctx), Style::default().palette(egui::Theme::Dark));
        draw_a_frame(&ctx, Some(egui::Theme::Light));
        assert_eq!(palette(&ctx), Style::default().palette(egui::Theme::Light));
    }

    #[test]
    fn a_system_that_does_not_say_gets_the_light_palette() {
        let ctx = egui::Context::default();
        apply(&ctx);
        show_in(&ctx, ThemeChoice::FollowSystem);
        draw_a_frame(&ctx, None);
        assert_eq!(palette(&ctx), Style::default().palette(egui::Theme::Light));
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
