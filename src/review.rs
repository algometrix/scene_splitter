//! The review screen of one video: its scenes as colour-coded cards with
//! previews. The person drags one scene onto another to merge them, before
//! anything is written.
//!
//! The screen only draws. What the person decides lives in a
//! `ReviewSession`, and the settings belong to the app. The screen is given
//! both to read, and returns each thing the person asked for, a change of
//! settings included, as a `ReviewRequest` for the app to apply.

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use anyhow::Result;
use eframe::egui;

use crate::detect::ScanKind;
use crate::exact_cut::CutPrecision;
use crate::export::{ExportOptions, OriginalAlwaysKept, TRASH_NAME};
use crate::fates::SceneFate;
use crate::focus::{self, PicturePosition, PictureStep, SceneStep};
use crate::folder_settings::{self, DetectionInUse, DetectionSettings, SettingsScope};
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::keyboard::{self, KeyboardOwner};
use crate::plan::{
    CutIndex, NudgeDirection, NudgeStep, SceneIndex, SceneLayout, SegmentIndex, SplitPlan,
};
use crate::review_session::{
    counted, EarlierSceneFiles, ExportObstacle, ReviewRequest, ReviewSession, SceneListNote,
};
use crate::scan::UndecodedFrames;
use crate::scene_list::SceneListFormat;
use crate::selection::ClickKind;
use crate::settings::{Settings, SettingsChange};
use crate::settings_controls;
use crate::split;
use crate::theme::{self, CardTint};
use crate::thumbnails::{self, PictureSize, Thumbnail};
use crate::timecode::{format_clock, format_length, format_timestamp, TimeSpan};

const NEW_CUT_FIELD_WIDTH: f32 = 90.0;
/// The room the "Add a split at" label, box and button take together.
const NEW_CUT_CONTROLS_WIDTH: f32 = 340.0;
const PREVIEW_ROUNDING: f32 = 6.0;
const SPACE_BETWEEN_SCENES: f32 = 10.0;
const FILE_NAME_FONT_SIZE: f32 = 16.0;
const SCENE_LENGTH_FONT_SIZE: f32 = 16.0;

const CARD_COLOR_BAR_WIDTH: f32 = 6.0;
const CARD_MARGIN: egui::Margin = egui::Margin {
    left: CARD_COLOR_BAR_WIDTH + 12.0,
    right: 12.0,
    top: 10.0,
    bottom: 12.0,
};

const TIMELINE_HEIGHT: f32 = 32.0;
const TIMELINE_GAP_BETWEEN_SCENES: f32 = 3.0;
const TIMELINE_MIN_BLOCK_WIDTH: f32 = 3.0;
/// Narrower blocks have no room for even a short scene number, so their
/// label is not measured at all. A video of a thousand scenes has a
/// thousand such blocks.
const TIMELINE_MIN_LABELLED_WIDTH: f32 = 22.0;
/// Room kept between a block's label and each of its ends.
const TIMELINE_LABEL_PADDING: f32 = 6.0;
const TIMELINE_LABEL_FONT_SIZE: f32 = 13.0;
/// Between a scene's name and its length on the timeline.
const TIMELINE_LABEL_GAP: f32 = 10.0;
const TIMELINE_ROUNDING: f32 = 6.0;
const PLAYHEAD_WIDTH: f32 = 2.0;

/// A scene longer than this has its length highlighted, as a hint that a
/// scene change inside it may have been missed.
const LONG_SCENE_SECONDS: f64 = 45.0 * 60.0;
const LONG_SCENE_OUTLINE_WIDTH: f32 = 3.0;

/// Dragging a scene this close to the top or bottom of the list scrolls it.
const AUTO_SCROLL_EDGE: f32 = 48.0;
const AUTO_SCROLL_STEP: f32 = 14.0;
const DRAG_HINT_OFFSET: egui::Vec2 = egui::vec2(16.0, 16.0);
/// What each tip is about, and the tip itself. Three, because a first-time
/// user reads three lines and skips eight.
const TIPS: [(&str, &str); 3] = [
    (
        "Merge",
        "Drag one scene onto another, or press Merge between two scenes.",
    ),
    (
        "Split",
        "Click a picture to play from there, then press S where the scene should end.",
    ),
    (
        "Undo",
        "Ctrl+Z takes back the last change. Reset scenes goes back to the scenes as found.",
    ),
];
/// Every key of the review screen, and what it does. "The scene" is the
/// one with the thick frame, which `KEYS_INTRODUCTION` says. The frame is
/// named by its shape: it is dark in the light palette and light in the
/// dark one.
const KEYS: [(&str, &str); 16] = [
    (
        "Up, Down",
        "Go to the scene above or below. Home and End go to the first and the last.",
    ),
    ("Shift+Up or Down", "Select more scenes, or fewer."),
    ("Enter, Space", "Play the scene."),
    ("Left, Right", "Choose a picture of the scene to play from."),
    ("J", "Merge the scene with the one below it."),
    ("M", "Merge the selected scenes."),
    ("U", "Take a merged scene apart."),
    (
        "X",
        "Drop the scene, so no file is written for it. Press X again to keep it.",
    ),
    (
        "Alt+Left or Right",
        "Move the split at the start of the scene earlier or later.",
    ),
    ("Ctrl+Z", "Take back the last change."),
    ("Ctrl+A, Esc", "Select every scene, or none."),
    ("Ctrl+Enter", "Split into files."),
    (
        "Ctrl+Shift+Enter",
        "Split later, with the other videos that are ready.",
    ),
    (
        "Tab",
        "Go through the buttons and boxes. Space presses the one that is marked.",
    ),
    ("Ctrl+O", "Add videos."),
    ("F1", "Show or hide this list."),
];
const KEYS_INTRODUCTION: &str = "The keys work on the scene with the thick frame around it. \
     M, U and X work on the selected scenes when any are selected. \
     While the player is open, Space pauses it and Left and Right skip in it.";
/// The list names the keys as most keyboards do. `input.modifiers.command`,
/// which the screens read, is Cmd on a Mac.
const KEYS_ON_THIS_SYSTEM: Option<&str> = if cfg!(target_os = "macos") {
    Some("Use Cmd where this list says Ctrl.")
} else {
    None
};
const KEYS_HEADING: &str = "Keys";
const SHOW_KEYS_LABEL: &str = "Show keys";
const HIDE_KEYS_LABEL: &str = "Hide keys";
const KEYS_LINK_HINT: &str = "Lists what every key does on this screen (F1).";
const KEY_NAME_WIDTH: f32 = 130.0;

const SPLIT_HINT: &str = "Starts the split (Ctrl+Enter).";
const JOIN_HINT: &str = "Makes these two scenes one. You can also drag one onto the other, \
     or press J on the upper one.";
const MOVE_SPLIT_LABEL: &str = "Move this split";
const MOVE_SPLIT_EARLIER_LABEL: &str = "Earlier";
const MOVE_SPLIT_LATER_LABEL: &str = "Later";
/// Lines the row between two cards up with the text on the cards.
const BETWEEN_SCENES_INDENT: f32 = 12.0;
const BETWEEN_SCENES_FONT_SIZE: f32 = 12.0;
const BETWEEN_SCENES_BUTTON_HEIGHT: f32 = 24.0;
const MOVE_SPLIT_EARLIER_HINT: &str =
    "Moves the split between these two scenes to the point before it where the video can be cut \
     (Alt+Left on the lower scene).";
const MOVE_SPLIT_LATER_HINT: &str =
    "Moves the split between these two scenes to the point after it where the video can be cut \
     (Alt+Right on the lower scene).";
/// With exact cuts on a split can be on any picture, so it moves by one.
const MOVE_SPLIT_ONE_PICTURE_EARLIER_HINT: &str =
    "Moves the split between these two scenes one picture earlier \
     (Alt+Left on the lower scene). Cuts are exact, so the split can be on any picture.";
const MOVE_SPLIT_ONE_PICTURE_LATER_HINT: &str =
    "Moves the split between these two scenes one picture later \
     (Alt+Right on the lower scene). Cuts are exact, so the split can be on any picture.";

/// The frame around the scene, and the picture, the keys act on. It is
/// thicker and stronger than any other outline on the screen, so it does
/// not rest on colour alone.
const FOCUS_RING_WIDTH: f32 = 3.0;
/// A line of the opposite lightness inside the frame, which keeps the frame
/// visible around a picture of its own lightness.
const PICTURE_FOCUS_INNER_WIDTH: f32 = 2.0;
/// Room kept around a focused picture that is scrolled into view.
const FOCUSED_PICTURE_MARGIN: f32 = 8.0;

const TIP_TOPIC_WIDTH: f32 = 56.0;
const TIP_COLUMN_GAP: f32 = 24.0;
const MIN_TIP_WIDTH: f32 = 120.0;
const NOTHING_FOUND_HINT: &str = "Only one scene was found, so there is nothing to split yet. \
     This video may change scene without a black screen in between.";
const NOTHING_FOUND_BY_HAND_HINT: &str =
    "You can also click a picture and press S in the player to split by hand.";
const LOOK_HARDER_LABEL: &str = "Look harder for scenes";
const LOOK_HARDER_EXPLANATION: &str =
    "Looks at every picture of the video for places where it changes abruptly, even subtly. \
     This is much slower than the first scan.";
const RESCAN_WARNING: &str = "Scanning again removes the merges and splits you made on this video.";
const SPLIT_LATER_LABEL: &str = "Split later";
const SPLIT_LATER_EXPLANATION: &str =
    "Keeps these scenes and shows the next video (Ctrl+Shift+Enter). Nothing is written yet. \
     When you have reviewed the others, press \"Split ready videos\" above the list on the left.";
const LEAVE_OUT_CHAPTERS_LABEL: &str = "Leave out the chapter marks";
const LEAVE_OUT_CHAPTERS_EXPLANATION: &str =
    "Merges the scenes that only a chapter mark keeps apart. Undo brings the splits back.";
const BLACK_SCREEN_SETTINGS_LABEL: &str = "Black screen settings";
const HIDE_BLACK_SCREEN_SETTINGS_LABEL: &str = "Hide the black screen settings";
const BLACK_SCREEN_SETTINGS_EXPLANATION: &str =
    "What counts as a black screen when this video is scanned.";
const BLACK_SCREEN_SETTINGS_HEADING: &str = "What counts as a black screen";
const BLACK_SCREEN_SETTINGS_APPLY_NOTE: &str =
    "Changes are saved straight away. Press Scan again to scan this video with them.";
const GENERAL_SETTINGS_IN_USE: &str = "This video is scanned with the general settings.";
const FOLDER_SETTINGS_IN_USE: &str =
    "This video is scanned with the settings remembered for its folder.";
const REMEMBER_FOR_FOLDER_LABEL: &str = "Remember these settings for this folder";
/// How the folder is named when the video's path does not say which it is.
const FOLDER_NOT_KNOWN: &str = "this video's folder";
const SCAN_AGAIN_EXPLANATION: &str =
    "Looks for black screens again, with the scan settings as they are now.";
const HIDE_MISSED_SCENES_LABEL: &str = "Hide the settings for missed scenes";
const MERGE_SELECTED_LABEL: &str = "Merge selected";
const MERGE_SELECTED_HINT: &str = "Merges the selected scenes into one (M).";
const TAKE_APART_LABEL: &str = "Take merged scenes apart";
const TAKE_APART_HINT: &str = "Takes the selected merged scenes apart (U).";
const TAKE_APART_HERE_HINT: &str = "Takes the merged scene apart at this point.";
const REPLACE_EARLIER_FILES_LABEL: &str = "Replace them";
/// How many earlier scene files, or splits, a warning names before it says
/// how many more there are. The hover names the rest.
const MOST_NAMED_IN_A_WARNING: usize = 3;
const MOST_NAMED_IN_A_HOVER: usize = 30;
const USE_GENERAL_SETTINGS_LABEL: &str = "Use the general settings for this folder";
/// A chapter title longer than this is cut short on the scene card. The
/// whole title is one hover away.
const MOST_TITLE_CHARACTERS_SHOWN: usize = 48;
const TITLE_CUT_SHORT_MARK: &str = "...";
const CHAPTER_TITLE_FONT_SIZE: f32 = 14.0;

const DROP_SCENE_LABEL: &str = "Drop this scene";
const DROP_SCENE_HINT: &str = "Leaves this scene out of the split, so no file is written for it. \
     X does the same for the selected scenes, or for the scene with the thick frame.";
const KEEP_SCENE_LABEL: &str = "Keep this scene";
const KEEP_SCENE_HINT: &str =
    "Writes this scene to a file after all. X does the same for the selected scenes, \
     or for the scene with the thick frame.";
/// On the card and in the hover text of a scene that is not written.
const DROPPED_BADGE: &str = "Dropped, not written";
/// On a timeline block too narrow to name its scene as well.
const DROPPED_SHORT_LABEL: &str = "Dropped";
const DROPPED_TILE_CAPTION: &str = "dropped, not written";
const SAVE_SCENE_LIST_LABEL: &str = "Save the scene list";
const SAVE_SCENE_LIST_HINT: &str =
    "Saves the scenes with their times to a file, without splitting the video: \
     as a table, as chapter marks, or as an edit list.";
const SAVE_SCENE_LIST_DIALOG_TITLE: &str = "Save the scene list";
const SCENE_LIST_NOT_SAVED: &str = "The scene list was not saved.";

/// Previews are drawn as wide as the card allows, within these limits.
const PREVIEWS_PER_ROW: f32 = 4.0;
const MIN_PREVIEW_WIDTH: f32 = 96.0;
const MAX_PREVIEW_WIDTH: f32 = 240.0;
const PREVIEW_ASPECT: f32 = 9.0 / 16.0;
/// Room kept beside the previews of a merged scene for its length and button.
const MERGED_PART_CONTROLS_WIDTH: f32 = 200.0;
const SCENE_BADGE_MIN_WIDTH: f32 = 56.0;

/// A merged scene with more parts than this shows only its first and last
/// few, until the person asks for all of them. A scene of fifty parts would
/// otherwise push every scene after it far down the list.
const MOST_PARTS_SHOWN_UNASKED: usize = 6;
/// How many parts of a long merged scene are shown at its start and its end.
const PARTS_SHOWN_AT_EACH_END: usize = 2;

const MILLISECONDS_PER_SECOND: f64 = 1000.0;

/// A moment of the video in whole milliseconds, exact enough to tell two
/// frames apart and, unlike seconds with a fraction, fit to look things up by.
type Millisecond = u64;

fn millisecond(time: f64) -> Millisecond {
    (time * MILLISECONDS_PER_SECOND).round() as Millisecond
}

/// Names a preview: the moment it shows, and the size it was decoded at. On
/// a sharper screen the same moment is decoded larger, which makes it
/// another preview, so a change of scale fetches the previews afresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PreviewKey {
    shows: Millisecond,
    size: PictureSize,
}

fn preview_key(time: f64, size: PictureSize) -> PreviewKey {
    PreviewKey {
        shows: millisecond(time),
        size,
    }
}

/// Names a scene for as long as the review is open, by the moment it begins.
/// Its place in the list would not do: that shifts with every merge above it.
type SceneKey = Millisecond;

fn scene_key(layout: &SceneLayout) -> SceneKey {
    millisecond(layout.span.start)
}

/// Whether a merged scene with many parts shows them all.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PartsView {
    /// The first and last few parts, which is how every long scene starts.
    Collapsed,
    Expanded,
}

/// One row of a merged scene's card.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PartRow {
    /// The part at this position in the scene, with its previews.
    Part(usize),
    /// The row that stands in for this many parts that are not shown.
    Hidden { part_count: usize },
}

fn has_too_many_parts_to_show_unasked(part_count: usize) -> bool {
    part_count > MOST_PARTS_SHOWN_UNASKED
}

/// The rows of a scene of `part_count` parts, top to bottom.
fn part_rows(part_count: usize, view: PartsView) -> Vec<PartRow> {
    let every_part = (0..part_count).map(PartRow::Part);
    if view == PartsView::Expanded || !has_too_many_parts_to_show_unasked(part_count) {
        return every_part.collect();
    }
    let hidden = PartRow::Hidden {
        part_count: part_count - 2 * PARTS_SHOWN_AT_EACH_END,
    };
    let first_parts = every_part.clone().take(PARTS_SHOWN_AT_EACH_END);
    let last_parts = every_part.skip(part_count - PARTS_SHOWN_AT_EACH_END);
    first_parts.chain([hidden]).chain(last_parts).collect()
}

/// One part of a scene, as its card draws it.
#[derive(Clone, Copy)]
struct ScenePart<'layout> {
    scene: SceneIndex,
    layout: &'layout SceneLayout,
    position_in_scene: usize,
}

/// What the split does with one scene, as its card says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CardFate {
    /// Written, and no scene of the video is dropped, so the file has the
    /// number of the scene and nothing more needs saying.
    Written,
    /// Written, as this one of the files. Said once a scene is dropped,
    /// since from then on files and scenes are numbered differently.
    WrittenAsFile {
        number: usize,
        of: usize,
    },
    Dropped,
}

impl CardFate {
    fn is_dropped(self) -> bool {
        self == Self::Dropped
    }
}

/// What each scene's card says about its file, in scene order. The scenes
/// that are written are numbered from one without gaps.
fn card_fates(fates: &[SceneFate]) -> Vec<CardFate> {
    let file_count = fates
        .iter()
        .filter(|&&fate| fate == SceneFate::Keep)
        .count();
    if file_count == fates.len() {
        return vec![CardFate::Written; fates.len()];
    }
    let mut files_before = 0;
    fates
        .iter()
        .map(|fate| match fate {
            SceneFate::Drop => CardFate::Dropped,
            SceneFate::Keep => {
                files_before += 1;
                CardFate::WrittenAsFile {
                    number: files_before,
                    of: file_count,
                }
            }
        })
        .collect()
}

/// One scene as its card is drawn.
#[derive(Clone, Copy)]
struct SceneCard<'layout> {
    scene: SceneIndex,
    layout: &'layout SceneLayout,
    fate: CardFate,
}

/// Everything the height of a scene's card depends on. A card that is out of
/// sight is not laid out, only given the height a card of the same shape had
/// when it was last drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CardShape {
    scene_starts: Millisecond,
    scene_ends: Millisecond,
    rows: usize,
    /// In whole points. Previews wrap onto more rows in a narrower card.
    width: u32,
    /// A dropped scene and a file number add to the heading, which may
    /// then wrap onto another row.
    fate: CardFate,
}

/// The timeline is one area that takes clicks, whatever the number of scenes.
fn timeline_id() -> egui::Id {
    egui::Id::new("review_timeline")
}

/// Only one review is on screen at a time, so a scene's place in the list
/// is enough to name its card's click and drag area.
fn scene_card_id(scene: SceneIndex) -> egui::Id {
    egui::Id::new(("scene_card", scene.position()))
}

enum Preview {
    Loading,
    Ready(egui::TextureHandle),
    /// Why the picture could not be made, for the person to read or copy.
    Failed(String),
}

struct LoadedPreview {
    key: PreviewKey,
    thumbnail: Result<Thumbnail>,
}

enum Highlight {
    None,
    Selected,
    /// Part of the run that releasing the current drag would merge. The run
    /// is shown in the colour of its first scene, which the merged scene
    /// will keep.
    WillMerge {
        first_scene: SceneIndex,
    },
}

/// What the person did to the scene list during one frame.
#[derive(Default)]
struct SceneListInput {
    clicked_scene: Option<SceneIndex>,
    drag_started_on: Option<SceneIndex>,
    cut_to_split_again: Option<CutIndex>,
    /// A strip the person wants out of its merged scene.
    segment_to_isolate: Option<SegmentIndex>,
    /// The time of a preview the person clicked, to play the video from.
    play_from: Option<f64>,
    /// The scene to join with the one after it.
    join_with_next: Option<SceneIndex>,
    split_to_move: Option<SplitToMove>,
    /// A scene the person wants dropped, or written after all.
    fate_wanted: Option<(SceneIndex, SceneFate)>,
}

/// A split the person wants at the next point, earlier or later, where the
/// video can be cut.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SplitToMove {
    /// The scene that starts at the split.
    scene_after: SceneIndex,
    direction: NudgeDirection,
}

/// Whether the list of keys is on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
enum KeysList {
    Hidden,
    Shown,
}

impl KeysList {
    fn toggled(self) -> Self {
        match self {
            Self::Hidden => Self::Shown,
            Self::Shown => Self::Hidden,
        }
    }
}

/// Whether a preview is the picture the keys would play from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PictureMark {
    Plain,
    Focused,
}

impl SceneListInput {
    /// A click on a preview selects its scene, like a click anywhere else on
    /// the card. A plain click also plays the video from that preview. With
    /// Shift or Ctrl held the person is building a selection, so it does not.
    fn note_preview_click(&mut self, scene: SceneIndex, preview_time: f64, ui: &egui::Ui) {
        self.clicked_scene = Some(scene);
        let modifiers = ui.input(|input| input.modifiers);
        if click_kind(modifiers) == ClickKind::Single {
            self.play_from = Some(preview_time);
        }
    }
}

/// The card with the black screen settings, which sits above the scenes and
/// scrolls with them.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SettingsCard {
    Hidden,
    /// Asked for in this frame or the one before. The list may be scrolled
    /// far down, so the card is brought into view once, or the click on its
    /// link would seem to do nothing.
    JustOpened,
    Open,
}

/// The screen of one review: the previews it has loaded, where it drew the
/// scene cards, and a drag that is under way. What the person decides about
/// the video is not here but in the `ReviewSession` the screen draws. The
/// screen never changes that. It returns what the person asked for.
pub struct ReviewScreen {
    previews: HashMap<PreviewKey, Preview>,
    loaded_previews_sender: Sender<LoadedPreview>,
    loaded_previews: Receiver<LoadedPreview>,
    /// Where each scene's card was drawn in the last frame. The whole card
    /// takes clicks and drags, and a drag ends on the card under the pointer.
    scene_cards: Vec<egui::Rect>,
    /// The visible part of the scene list, which cards can scroll out of.
    list_area: egui::Rect,
    /// The scene being dragged onto another, while the mouse button is down.
    dragged_scene: Option<SceneIndex>,
    /// A scene clicked on the timeline, to bring its card into view.
    scene_to_scroll_to: Option<SceneIndex>,
    /// Whether the options for finding missed scenes are shown.
    missed_scenes_open: bool,
    /// Whether the card with the black screen settings is shown.
    black_screen_settings: SettingsCard,
    /// The size previews are decoded at, which suits the sharpness of the
    /// screen the window is on.
    preview_pixels: PictureSize,
    /// The long merged scenes the person asked to see every part of.
    expanded_scenes: HashSet<SceneKey>,
    /// How tall each scene's card was when it was last drawn, for leaving
    /// the right room for it while it is out of sight.
    card_heights: HashMap<CardShape, f32>,
    /// How tall the row between two cards is, with its "Join scenes" button,
    /// once one was drawn. They are all alike.
    join_button_height: Option<f32>,
    keys_list: KeysList,
    /// Set when the list of keys was just asked for, to bring it into view.
    /// It scrolls with the scenes and may be far above them.
    scroll_to_keys_list: bool,
    /// Whether a key that works on the focused scene was pressed in the
    /// frame before this one. Its request has been applied by now.
    focus_keys_used_last_frame: bool,
    /// Whether this frame scrolls the focused scene into view, which it
    /// does after such a key and not after a click.
    bring_focus_into_view: bool,
}

impl Default for ReviewScreen {
    fn default() -> Self {
        let (loaded_previews_sender, loaded_previews) = mpsc::channel();
        Self {
            previews: HashMap::new(),
            loaded_previews_sender,
            loaded_previews,
            scene_cards: Vec::new(),
            list_area: egui::Rect::NOTHING,
            dragged_scene: None,
            scene_to_scroll_to: None,
            missed_scenes_open: false,
            black_screen_settings: SettingsCard::Hidden,
            preview_pixels: thumbnails::SMALLEST_THUMBNAIL,
            expanded_scenes: HashSet::new(),
            card_heights: HashMap::new(),
            join_button_height: None,
            keys_list: KeysList::Hidden,
            scroll_to_keys_list: false,
            focus_keys_used_last_frame: false,
            bring_focus_into_view: false,
        }
    }
}

impl ReviewScreen {
    /// Draws the review and returns what the person asked for, in the order
    /// it is to be applied. `playhead` is where the player is in this video,
    /// if it is playing it.
    ///
    /// The header stays put and holds only what is needed all the time: the
    /// name, the main action, the timeline and the editing buttons. The
    /// numbers, hints and tips scroll with the scenes, so the scenes always
    /// have room however small the window is.
    ///
    /// `keyboard` says whether the keys pressed in this frame are this
    /// screen's to act on.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        session: &ReviewSession,
        settings: &Settings,
        playhead: Option<f64>,
        keyboard: KeyboardOwner,
    ) -> Vec<ReviewRequest> {
        let palette = theme::palette(ctx);
        self.decode_previews_for_the_sharpness_of_the_screen(ctx);
        self.receive_loaded_previews(ctx);
        let mut requests = shortcut_requests(ctx, keyboard);
        let asked_with_focus_keys = focus_key_requests(ctx, keyboard, session);
        self.follow_the_focus_after_its_keys(ctx, &asked_with_focus_keys);
        requests.extend(asked_with_focus_keys);
        if keys_list_key_pressed(ctx, keyboard) {
            self.toggle_keys_list();
        }
        let detection = settings.detection_for(session.video());

        let asked_in_header = egui::TopBottomPanel::top("review_header")
            .frame(theme::header_panel(ctx))
            .show_separator_line(false)
            .show(ctx, |ui| {
                self.show_header(ui, session, settings, &detection, playhead)
            })
            .inner;
        requests.extend(asked_in_header);
        let settings_card = self.black_screen_settings;
        if settings_card == SettingsCard::JustOpened {
            self.black_screen_settings = SettingsCard::Open;
        }

        let (asked_in_overview, input) = egui::CentralPanel::default()
            .frame(theme::panel(palette.page))
            .show(ctx, |ui| {
                self.list_area = ui.max_rect();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    // Dragging moves scenes, the wheel and scroll bar scroll.
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        let asked_in_overview =
                            self.show_overview(ui, session, settings, &detection, settings_card);
                        (asked_in_overview, self.show_scenes(ui, session))
                    })
                    .inner
            })
            .inner;
        requests.extend(asked_in_overview);
        requests.extend(self.requests_from_scene_list(ctx, input));
        self.show_drag_hint(ctx);
        requests
    }

    /// A key's request is applied once this frame is over, so the scene it
    /// leaves the focus on is only known in the next frame. That frame
    /// scrolls it into view.
    fn follow_the_focus_after_its_keys(&mut self, ctx: &egui::Context, asked: &[ReviewRequest]) {
        self.bring_focus_into_view = self.focus_keys_used_last_frame;
        self.focus_keys_used_last_frame = !asked.is_empty();
        if self.focus_keys_used_last_frame {
            ctx.request_repaint();
        }
    }

    fn toggle_keys_list(&mut self) {
        self.keys_list = self.keys_list.toggled();
        self.scroll_to_keys_list = self.keys_list == KeysList::Shown;
    }

    /// The part that scrolls away above the scenes: the key numbers, what
    /// deserves attention, the tips and the list of keys.
    fn show_overview(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        settings: &Settings,
        detection: &DetectionInUse,
        settings_card: SettingsCard,
    ) -> Vec<ReviewRequest> {
        let mut requests = show_notes(ui, session, detection, settings_card);
        if settings.show_tips {
            match show_tips_card(ui, self.keys_list) {
                TipsCard::HideAsked => {
                    requests.push(ReviewRequest::ChangeSettings(SettingsChange::HideTips));
                }
                TipsCard::KeysListToggled => self.toggle_keys_list(),
                TipsCard::Kept => {}
            }
        }
        if self.keys_list == KeysList::Shown {
            let keys_card = show_keys_card(ui);
            if std::mem::take(&mut self.scroll_to_keys_list) {
                ui.scroll_to_rect(keys_card.area, Some(egui::Align::TOP));
            }
            if keys_card.hide_asked {
                self.toggle_keys_list();
            }
        }
        ui.add_space(SPACE_BETWEEN_SCENES);
        requests
    }

    fn show_header(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        settings: &Settings,
        detection: &DetectionInUse,
        playhead: Option<f64>,
    ) -> Vec<ReviewRequest> {
        let palette = theme::palette(ui.ctx());
        let mut requests = Vec::new();
        requests.extend(show_title_and_main_action(ui, session));
        requests.extend(show_export_obstacle(ui, session));
        show_exact_cut_notice(ui, session, settings.export.cut_precision);
        requests.extend(show_export_options(
            ui,
            session.video(),
            settings.export,
            session.dropped_count(),
        ));
        requests.extend(self.show_timeline(ui, session, playhead));
        ui.horizontal_wrapped(|ui| {
            requests.extend(show_merge_controls(ui, session));
            requests.extend(show_undo_and_reset(ui, session));
            ui.separator();
            // The label, box and button stay together: when they do not all
            // fit on this row they start the next one.
            if ui.available_size_before_wrap().x < NEW_CUT_CONTROLS_WIDTH {
                ui.end_row();
            }
            requests.extend(show_new_split_controls(ui, session));
            if !settings.show_tips && ui.button("Show tips").clicked() {
                requests.push(ReviewRequest::ChangeSettings(SettingsChange::ShowTips));
            }
        });
        if let Some(refusal) = session.drop_refusal() {
            ui.colored_label(palette.warning_text, refusal.to_string());
        }
        requests.extend(self.show_scan_row(ui, session, detection));
        show_scene_list_note(ui, session);
        requests.extend(show_rescan_consent(ui, session));
        requests
    }

    /// What the scan found, and the ways to scan again.
    fn show_scan_row(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        detection: &DetectionInUse,
    ) -> Vec<ReviewRequest> {
        let mut requests = Vec::new();
        ui.horizontal_wrapped(|ui| {
            // Same text size as the links beside it, so the line reads level.
            ui.label(theme::muted(ui, session.scan_summary()))
                .on_hover_text(session.scan_details());
            if session.chapter_split_count() > 0 {
                let leave_out = whole_link(ui, LEAVE_OUT_CHAPTERS_LABEL)
                    .on_hover_text(LEAVE_OUT_CHAPTERS_EXPLANATION);
                if leave_out.clicked() {
                    requests.push(ReviewRequest::LeaveOutChapterSplits);
                }
            }
            let scan_again = whole_link(ui, "Scan again").on_hover_text(SCAN_AGAIN_EXPLANATION);
            if scan_again.clicked() {
                requests.push(ReviewRequest::Rescan(ScanKind::BlackScreens));
            }
            let settings_label = match self.black_screen_settings {
                SettingsCard::Hidden => BLACK_SCREEN_SETTINGS_LABEL,
                SettingsCard::JustOpened | SettingsCard::Open => HIDE_BLACK_SCREEN_SETTINGS_LABEL,
            };
            let settings_link =
                whole_link(ui, settings_label).on_hover_text(BLACK_SCREEN_SETTINGS_EXPLANATION);
            if settings_link.clicked() {
                self.black_screen_settings = match self.black_screen_settings {
                    SettingsCard::Hidden => SettingsCard::JustOpened,
                    SettingsCard::JustOpened | SettingsCard::Open => SettingsCard::Hidden,
                };
            }
            let missed_label = if self.missed_scenes_open {
                HIDE_MISSED_SCENES_LABEL
            } else {
                "Scenes missed?"
            };
            let missed = whole_link(ui, missed_label)
                .on_hover_text("For videos that change scene without a black screen.");
            if missed.clicked() {
                self.missed_scenes_open = !self.missed_scenes_open;
            }
            let keys_label = match self.keys_list {
                KeysList::Hidden => KEYS_HEADING,
                KeysList::Shown => HIDE_KEYS_LABEL,
            };
            if whole_link(ui, keys_label)
                .on_hover_text(KEYS_LINK_HINT)
                .clicked()
            {
                self.toggle_keys_list();
            }
            requests.extend(show_save_scene_list_menu(ui, session.video()));
        });
        if self.missed_scenes_open {
            requests.extend(show_hard_cut_scan_controls(ui, detection.detection));
            requests.extend(show_settings_scope(ui, session.video(), &detection.scope));
        }
        requests
    }

    /// The whole video as one bar, with a coloured block per scene. Clicking
    /// a block brings that scene's card into view.
    ///
    /// The bar is one area that takes the pointer, and the scene under the
    /// pointer is worked out from where it is. A widget per scene would cost
    /// a video of a thousand scenes a thousand widgets in every frame.
    fn show_timeline(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        playhead: Option<f64>,
    ) -> Option<ReviewRequest> {
        let palette = theme::palette(ui.ctx());
        let plan = session.plan();
        let whole_video = plan.whole_video();
        let layouts = plan.scene_layouts();
        let fates = plan.scene_fates();
        let size = egui::vec2(ui.available_width(), TIMELINE_HEIGHT);
        let (_, bar) = ui.allocate_space(size);
        let response = ui.interact(bar, timeline_id(), egui::Sense::click());
        let blocks = timeline_blocks(
            layouts.iter().map(|layout| layout.span),
            whole_video,
            bar.x_range(),
        );

        let painter = ui.painter();
        for (((scene, layout), &across), &fate) in
            with_scene_indices(&layouts).zip(&blocks).zip(&fates)
        {
            let block = egui::Rect::from_x_y_ranges(across, bar.y_range());
            match fate {
                SceneFate::Keep => paint_timeline_block(painter, block, scene, layout),
                SceneFate::Drop => paint_dropped_timeline_block(painter, block, scene),
            }
            // A faint line where two detected pieces were merged.
            for &cut_index in &layout.merged_cuts {
                let cut_x = x_at_time(plan.cut_time(cut_index), whole_video, bar.x_range());
                let line = egui::Stroke::new(1.0_f32, palette.faint_on_fill);
                painter.vline(cut_x, block.y_range(), line);
            }
        }
        if let Some(playhead) = playhead {
            let line = egui::Stroke::new(PLAYHEAD_WIDTH, palette.text);
            let playhead_x = x_at_time(playhead, whole_video, bar.x_range());
            painter.vline(playhead_x, bar.y_range(), line);
        }

        let scene = response
            .hover_pos()
            .and_then(|pointer| scene_at_x(&blocks, pointer.x))?;
        let layout = layouts.get(scene.position())?;
        let fate = fates.get(scene.position()).copied().unwrap_or_default();
        let clicked = response.clicked();
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text_at_pointer(scene_description(
                scene,
                layout,
                session.chapter_title(layout.span),
                fate,
            ));
        if !clicked {
            return None;
        }
        self.scene_to_scroll_to = Some(scene);
        Some(ReviewRequest::ClickScene {
            scene,
            kind: ClickKind::Single,
        })
    }

    fn show_scenes(&mut self, ui: &mut egui::Ui, session: &ReviewSession) -> SceneListInput {
        self.scroll_while_dragging_near_an_edge(ui);

        let drop_target = self.drop_target(ui.ctx());
        let cards_last_frame = std::mem::take(&mut self.scene_cards);
        // Rebuilt from the cards of this frame, so the heights of scenes
        // that no longer exist do not pile up.
        let heights_last_frame = std::mem::take(&mut self.card_heights);
        let mut input = SceneListInput::default();
        let layouts = session.plan().scene_layouts();
        let fates = card_fates(&session.scene_fates());
        for ((scene, layout), &fate) in with_scene_indices(&layouts).zip(&fates) {
            let scene_card = SceneCard {
                scene,
                layout,
                fate,
            };
            let shape = self.card_shape(scene_card, ui.available_width());
            // A card far from the part of the list in view is not laid out.
            // It only keeps its room, so the scroll bar and every card after
            // it stay where they are. A card of a shape not drawn before is
            // laid out once, to learn its height.
            let room_out_of_sight = heights_last_frame
                .get(&shape)
                .and_then(|&height| leave_room_if_out_of_sight(ui, height));
            let was_out_of_sight = room_out_of_sight.is_some();
            let card = match room_out_of_sight {
                Some(room) => room,
                None => {
                    // The card's own click and drag area is registered
                    // before its contents, so the buttons and previews on
                    // top of it still work. Its size is only known after
                    // drawing, hence last frame's rectangle.
                    if let Some(&card_last_frame) = cards_last_frame.get(scene.position()) {
                        self.note_card_input(ui, scene, card_last_frame, &mut input);
                    }
                    self.show_scene(ui, session, scene_card, drop_target, &mut input)
                }
            };
            self.card_heights.insert(shape, card.height());
            if self.scene_to_scroll_to == Some(scene) {
                ui.scroll_to_rect(card, Some(egui::Align::TOP));
            }
            let focused_here = session.focus().filter(|focus| focus.scene() == scene);
            if let Some(focus) = focused_here.filter(|_| self.bring_focus_into_view) {
                // A chosen picture brings itself into view where it is
                // drawn, which shows more of a tall card than its top.
                if focus.picture().is_none() || was_out_of_sight {
                    ui.scroll_to_rect(card.expand(FOCUS_RING_WIDTH), None);
                }
            }
            self.scene_cards.push(card);

            let is_last_scene = scene.next().position() == layouts.len();
            if is_last_scene {
                ui.add_space(SPACE_BETWEEN_SCENES);
                continue;
            }
            match self.show_join_row_if_in_sight(ui, scene, session.nudge_step()) {
                Some(BetweenScenes::Join) => input.join_with_next = Some(scene),
                Some(BetweenScenes::MoveSplit(direction)) => {
                    input.split_to_move = Some(SplitToMove {
                        scene_after: scene.next(),
                        direction,
                    });
                }
                None => {}
            }
        }
        self.scene_to_scroll_to = None;
        input
    }

    /// Draws the row between `scene` and the next one, or only leaves room
    /// for it while it is out of sight. Returns what was asked for on it.
    fn show_join_row_if_in_sight(
        &mut self,
        ui: &mut egui::Ui,
        scene: SceneIndex,
        nudge_step: NudgeStep,
    ) -> Option<BetweenScenes> {
        let room_out_of_sight = self
            .join_button_height
            .and_then(|height| leave_room_if_out_of_sight(ui, height));
        if room_out_of_sight.is_some() {
            return None;
        }
        let row = show_join_row(ui, scene, nudge_step);
        self.join_button_height = Some(row.height);
        row.asked
    }

    fn card_shape(&self, scene_card: SceneCard, width: f32) -> CardShape {
        let layout = scene_card.layout;
        CardShape {
            scene_starts: millisecond(layout.span.start),
            scene_ends: millisecond(layout.span.end),
            rows: part_rows(layout.segments.len(), self.parts_view(layout)).len(),
            width: width.round() as u32,
            fate: scene_card.fate,
        }
    }

    fn parts_view(&self, layout: &SceneLayout) -> PartsView {
        if self.expanded_scenes.contains(&scene_key(layout)) {
            PartsView::Expanded
        } else {
            PartsView::Collapsed
        }
    }

    /// Remembered for as long as this review is open.
    fn set_parts_view(&mut self, layout: &SceneLayout, view: PartsView) {
        let scene = scene_key(layout);
        match view {
            PartsView::Expanded => self.expanded_scenes.insert(scene),
            PartsView::Collapsed => self.expanded_scenes.remove(&scene),
        };
    }

    fn scroll_while_dragging_near_an_edge(&self, ui: &mut egui::Ui) {
        if self.dragged_scene.is_none() {
            return;
        }
        let Some(pointer) = ui.ctx().pointer_latest_pos() else {
            return;
        };
        if pointer.y < self.list_area.top() + AUTO_SCROLL_EDGE {
            ui.scroll_with_delta(egui::vec2(0.0, AUTO_SCROLL_STEP));
        } else if pointer.y > self.list_area.bottom() - AUTO_SCROLL_EDGE {
            ui.scroll_with_delta(egui::vec2(0.0, -AUTO_SCROLL_STEP));
        }
        // Keep scrolling while the pointer rests at the edge.
        ui.ctx().request_repaint();
    }

    /// Takes clicks and the start of a drag anywhere on a scene's card.
    fn note_card_input(
        &self,
        ui: &mut egui::Ui,
        scene: SceneIndex,
        card: egui::Rect,
        input: &mut SceneListInput,
    ) {
        let response = ui.interact(card, scene_card_id(scene), egui::Sense::click_and_drag());
        if response.clicked() {
            input.clicked_scene = Some(scene);
        }
        if response.drag_started() {
            input.drag_started_on = Some(scene);
        }
        if response.hovered() && self.dragged_scene.is_none() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    /// Draws one scene card and returns where it ended up.
    fn show_scene(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        scene_card: SceneCard,
        drop_target: Option<SceneIndex>,
        input: &mut SceneListInput,
    ) -> egui::Rect {
        let palette = theme::palette(ui.ctx());
        let SceneCard {
            scene,
            layout,
            fate,
        } = scene_card;
        // A dropped scene gives up its colour, on its card as on the
        // timeline, and says in words that it is not written.
        let (own_color, plain_fill) = if fate.is_dropped() {
            (palette.dropped, palette.dropped_fill)
        } else {
            (palette.scene_color(scene.position()), palette.card)
        };
        let (color, fill, outline) = match self.highlight(session, scene, drop_target) {
            Highlight::None => {
                let color = own_color;
                (
                    color,
                    plain_fill,
                    egui::Stroke::new(1.0_f32, palette.card_border),
                )
            }
            Highlight::Selected => {
                let color = own_color;
                (
                    color,
                    palette.tinted_card(color, CardTint::Selected),
                    egui::Stroke::new(2.0_f32, color),
                )
            }
            Highlight::WillMerge { first_scene } => {
                let color = palette.scene_color(first_scene.position());
                (
                    color,
                    palette.tinted_card(color, CardTint::WillMerge),
                    egui::Stroke::new(2.0_f32, color),
                )
            }
        };

        let parts_view = self.parts_view(layout);
        let mut wanted_parts_view = None;
        let focus_here = session.focus().filter(|focus| focus.scene() == scene);
        let focused_picture = focus_here.and_then(|focus| focus.picture());
        let card = egui::Frame::none()
            .fill(fill)
            .rounding(theme::CARD_ROUNDING)
            .stroke(outline)
            .shadow(palette.soft_shadow())
            .inner_margin(CARD_MARGIN)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let chapter_title = session.chapter_title(layout.span);
                if let Some(wanted) = show_scene_heading(ui, scene_card, color, chapter_title) {
                    input.fate_wanted = Some((scene, wanted));
                }
                let room_for_previews = if layout.is_merged() {
                    ui.available_width() - MERGED_PART_CONTROLS_WIDTH
                } else {
                    ui.available_width()
                };
                let preview_size = preview_size_for(room_for_previews, ui.spacing().item_spacing.x);
                let part_count = layout.segments.len();
                let rows = part_rows(part_count, parts_view);
                // A picture chosen with the keys in a part that is not
                // shown would be a focus nobody can see.
                if focused_picture.is_some_and(|picture| !shows_part_of(&rows, picture)) {
                    wanted_parts_view = Some(PartsView::Expanded);
                }
                for (row_position, row) in rows.iter().enumerate() {
                    match *row {
                        PartRow::Part(position_in_scene) => {
                            let part = ScenePart {
                                scene,
                                layout,
                                position_in_scene,
                            };
                            self.show_part(
                                ui,
                                session.video(),
                                part,
                                preview_size,
                                focused_picture,
                                input,
                            );
                            // The button that separates two parts is only
                            // offered between two parts that are both shown.
                            let next_part = PartRow::Part(position_in_scene + 1);
                            if rows.get(row_position + 1) == Some(&next_part) {
                                show_separate_button(ui, session.plan(), part, input);
                            }
                        }
                        PartRow::Hidden {
                            part_count: hidden_count,
                        } => {
                            if show_hidden_parts_row(ui, hidden_count, part_count) {
                                wanted_parts_view = Some(PartsView::Expanded);
                            }
                        }
                    }
                }
                let can_be_collapsed = parts_view == PartsView::Expanded
                    && has_too_many_parts_to_show_unasked(part_count);
                if can_be_collapsed && show_fewer_parts_button(ui) {
                    wanted_parts_view = Some(PartsView::Collapsed);
                }
            })
            .response
            .rect;
        if let Some(view) = wanted_parts_view {
            self.set_parts_view(layout, view);
            // The card changes height, which the next frame has to draw.
            ui.ctx().request_repaint();
        }

        let color_bar =
            egui::Rect::from_min_size(card.min, egui::vec2(CARD_COLOR_BAR_WIDTH, card.height()));
        let left_corners_only = egui::Rounding {
            nw: theme::CARD_ROUNDING,
            sw: theme::CARD_ROUNDING,
            ne: 0.0,
            se: 0.0,
        };
        ui.painter()
            .rect_filled(color_bar, left_corners_only, color);
        if focus_here.is_some() {
            paint_focus_ring(ui.painter(), card, theme::CARD_ROUNDING);
        }
        card
    }

    /// One part of a scene: its previews and, in a merged scene, its length
    /// and the button that takes it out again.
    fn show_part(
        &mut self,
        ui: &mut egui::Ui,
        video: &Arc<Path>,
        part: ScenePart,
        preview_size: egui::Vec2,
        focused_picture: Option<PicturePosition>,
        input: &mut SceneListInput,
    ) {
        let ScenePart {
            scene,
            layout,
            position_in_scene,
        } = part;
        let segment = layout.segments[position_in_scene];
        // Wrapped, so a narrow card shows two rows of pictures instead of
        // cutting the last ones off.
        ui.horizontal_wrapped(|ui| {
            let preview_times = thumbnails::preview_times(segment);
            for (place_in_part, time) in preview_times.into_iter().enumerate() {
                let picture = focus::picture_position(position_in_scene, place_in_part);
                let mark = if focused_picture == Some(picture) {
                    PictureMark::Focused
                } else {
                    PictureMark::Plain
                };
                if self.show_preview(ui, video, time, preview_size, mark) {
                    input.note_preview_click(scene, time, ui);
                }
            }
            if layout.is_merged() {
                ui.vertical(|ui| {
                    ui.label(theme::muted(ui, format_length(segment.seconds())));
                    if show_move_out_button(ui) {
                        input.segment_to_isolate = Some(layout.segment_index(position_in_scene));
                    }
                });
            }
        });
    }

    /// The scene the dragged scene would be merged with if released now.
    fn drop_target(&self, ctx: &egui::Context) -> Option<SceneIndex> {
        let source = self.dragged_scene?;
        let pointer = ctx.pointer_latest_pos()?;
        // A floating window, such as the player, may be covering the list.
        let over_the_list_itself = ctx
            .layer_id_at(pointer)
            .is_some_and(|layer| layer.order == egui::Order::Background);
        if !self.list_area.contains(pointer) || !over_the_list_itself {
            return None;
        }
        self.scene_cards
            .iter()
            .position(|card| card.contains(pointer))
            .map(SceneIndex::new)
            .filter(|&target| target != source)
    }

    fn highlight(
        &self,
        session: &ReviewSession,
        scene: SceneIndex,
        drop_target: Option<SceneIndex>,
    ) -> Highlight {
        if let (Some(source), Some(target)) = (self.dragged_scene, drop_target) {
            let will_merge = span_between(source, target);
            if will_merge.contains(&scene) {
                return Highlight::WillMerge {
                    first_scene: *will_merge.start(),
                };
            }
        }
        if session.selection().contains(scene) || self.dragged_scene == Some(scene) {
            Highlight::Selected
        } else {
            Highlight::None
        }
    }

    /// Turns what the person did to the scene list into requests, and
    /// follows a drag from its start to its release.
    fn requests_from_scene_list(
        &mut self,
        ctx: &egui::Context,
        input: SceneListInput,
    ) -> Vec<ReviewRequest> {
        let play = input.play_from.map(ReviewRequest::PlayFrom);
        let mut requests = self.edits_from_scene_list(ctx, &input);
        requests.extend(play);
        requests
    }

    fn edits_from_scene_list(
        &mut self,
        ctx: &egui::Context,
        input: &SceneListInput,
    ) -> Vec<ReviewRequest> {
        if let Some(segment) = input.segment_to_isolate {
            return vec![ReviewRequest::MoveOutSegment(segment)];
        }
        if let Some(cut_index) = input.cut_to_split_again {
            return vec![ReviewRequest::SeparateAt(cut_index)];
        }
        if let Some(scene) = input.join_with_next {
            return vec![ReviewRequest::MergeScenes(scene..=scene.next())];
        }
        if let Some(split) = input.split_to_move {
            return vec![ReviewRequest::NudgeSplit {
                scene: split.scene_after,
                direction: split.direction,
            }];
        }
        match input.fate_wanted {
            Some((scene, SceneFate::Drop)) => return vec![ReviewRequest::DropScene(scene)],
            Some((scene, SceneFate::Keep)) => return vec![ReviewRequest::KeepScene(scene)],
            None => {}
        }
        let mut requests = Vec::new();
        if let Some(scene) = input.clicked_scene {
            let modifiers = ctx.input(|input| input.modifiers);
            requests.push(ReviewRequest::ClickScene {
                scene,
                kind: click_kind(modifiers),
            });
        }
        if let Some(scene) = input.drag_started_on {
            self.dragged_scene = Some(scene);
        }

        let Some(source) = self.dragged_scene else {
            return requests;
        };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let button_released =
            ctx.input(|input| input.pointer.any_released() || !input.pointer.any_down());
        if !button_released {
            return requests;
        }
        if let Some(target) = self.drop_target(ctx) {
            requests.push(ReviewRequest::MergeScenes(span_between(source, target)));
        }
        self.dragged_scene = None;
        requests
    }

    /// Says next to the pointer what releasing the drag would do.
    fn show_drag_hint(&self, ctx: &egui::Context) {
        let (Some(source), Some(pointer)) = (self.dragged_scene, ctx.pointer_latest_pos()) else {
            return;
        };
        let hint = match self.drop_target(ctx) {
            Some(target) => merge_description(&span_between(source, target)),
            None => drag_hint(source),
        };
        egui::Area::new(egui::Id::new("drag_hint"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pointer + DRAG_HINT_OFFSET)
            // The hint must not sit between the pointer and the scene cards.
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(hint));
            });
    }

    /// Draws one preview. Returns true when it was clicked, which plays the
    /// video from that moment.
    fn show_preview(
        &mut self,
        ui: &mut egui::Ui,
        video: &Arc<Path>,
        time: f64,
        size: egui::Vec2,
        mark: PictureMark,
    ) -> bool {
        let key = preview_key(time, self.preview_pixels);
        // Only previews that can be seen are fetched, so opening a video of
        // sixty scenes does not start hundreds of ffmpeg runs at once.
        let about_to_be_drawn_at = egui::Rect::from_min_size(ui.cursor().min, size);
        if !self.previews.contains_key(&key) && ui.is_rect_visible(about_to_be_drawn_at) {
            self.start_loading_preview(ui.ctx(), video, key, time);
        }

        // Previews take clicks but not drags, so a drag that starts on a
        // preview still moves the whole scene card underneath.
        let (response, why_no_preview) = match self.previews.get(&key) {
            Some(Preview::Ready(texture)) => {
                let picture = egui::Image::new((texture.id(), size))
                    .rounding(PREVIEW_ROUNDING)
                    .sense(egui::Sense::click());
                (ui.add(picture), None)
            }
            Some(Preview::Failed(reason)) => (
                show_placeholder(ui, size, "No preview"),
                Some(reason.as_str()),
            ),
            Some(Preview::Loading) | None => (show_placeholder(ui, size, "Loading"), None),
        };
        let clicked = response.clicked();
        if mark == PictureMark::Focused {
            paint_picture_focus(ui.painter(), response.rect);
            if self.bring_focus_into_view {
                ui.scroll_to_rect(response.rect.expand(FOCUSED_PICTURE_MARGIN), None);
            }
        }
        if self.dragged_scene.is_none() {
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(preview_hover_text(time, why_no_preview));
        }
        clicked
    }

    fn start_loading_preview(
        &mut self,
        ctx: &egui::Context,
        video: &Arc<Path>,
        key: PreviewKey,
        time: f64,
    ) {
        self.previews.insert(key, Preview::Loading);

        let video = Arc::clone(video);
        let loaded_previews_sender = self.loaded_previews_sender.clone();
        let ctx = ctx.clone();
        rayon::spawn(move || {
            let thumbnail = thumbnails::grab_thumbnail(&video, time, key.size);
            // The send only fails once this screen is gone, when nobody
            // needs the preview any more.
            if loaded_previews_sender
                .send(LoadedPreview { key, thumbnail })
                .is_ok()
            {
                ctx.request_repaint();
            }
        });
    }

    /// Previews are drawn at a size in points. A sharp screen has more than
    /// one pixel to the point, and a preview decoded for an ordinary screen
    /// looks soft on it. When the window moves to a screen of another
    /// sharpness, or the person zooms, the previews decoded for the old
    /// scale are dropped and those in view are fetched again.
    fn decode_previews_for_the_sharpness_of_the_screen(&mut self, ctx: &egui::Context) {
        let wanted = thumbnails::thumbnail_size_for(MAX_PREVIEW_WIDTH, ctx.pixels_per_point());
        if wanted == self.preview_pixels {
            return;
        }
        self.preview_pixels = wanted;
        self.previews.retain(|key, _| key.size == wanted);
    }

    fn receive_loaded_previews(&mut self, ctx: &egui::Context) {
        for loaded in self.loaded_previews.try_iter() {
            // Asked for before the scale changed, and of no use now.
            if loaded.key.size != self.preview_pixels {
                continue;
            }
            let preview = match loaded.thumbnail {
                Ok(thumbnail) => {
                    let PictureSize { width, height } = thumbnail.size;
                    let image = egui::ColorImage::from_rgb([width, height], &thumbnail.rgb);
                    Preview::Ready(ctx.load_texture(
                        format!("preview-{}-{width}", loaded.key.shows),
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                }
                Err(error) => Preview::Failed(format!("{error:#}")),
            };
            self.previews.insert(loaded.key, preview);
        }
    }
}

/// Leaves room of `height` for something that is not in the part of the
/// list in view, and returns that room. Returns `None`, and leaves nothing,
/// when any of it would be in view: then it has to be drawn.
fn leave_room_if_out_of_sight(ui: &mut egui::Ui, height: f32) -> Option<egui::Rect> {
    let size = egui::vec2(ui.available_width(), height);
    let would_be_drawn_at = egui::Rect::from_min_size(ui.cursor().min, size);
    if ui.is_rect_visible(would_be_drawn_at) {
        return None;
    }
    let (room, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    Some(room)
}

/// How far along the bar `time` of the video is.
fn x_at_time(time: f64, whole_video: TimeSpan, bar: egui::Rangef) -> f32 {
    // A video of a single frame has no length to take a share of.
    let video_length = whole_video.seconds().max(f64::MIN_POSITIVE);
    let share = ((time - whole_video.start) / video_length).clamp(0.0, 1.0);
    bar.min + bar.span() * share as f32
}

/// Where along the bar each scene's block is drawn, in the order of the
/// scenes. A scene too short to see still gets a sliver, so in a video of
/// very many scenes the blocks overlap, each drawn over the one before.
fn timeline_blocks(
    scenes: impl Iterator<Item = TimeSpan>,
    whole_video: TimeSpan,
    bar: egui::Rangef,
) -> Vec<egui::Rangef> {
    scenes
        .map(|scene| {
            let left = x_at_time(scene.start, whole_video, bar);
            let right = (x_at_time(scene.end, whole_video, bar) - TIMELINE_GAP_BETWEEN_SCENES)
                .max(left + TIMELINE_MIN_BLOCK_WIDTH);
            egui::Rangef::new(left, right)
        })
        .collect()
}

/// The scene whose block is under a pointer at `pointer_x`: the last one
/// that begins at or before it, which is also the one drawn on top there.
/// The gap after a block belongs to that block's scene, so no spot on the
/// bar is dead.
fn scene_at_x(blocks: &[egui::Rangef], pointer_x: f32) -> Option<SceneIndex> {
    let blocks_begun = blocks.partition_point(|block| block.min <= pointer_x);
    blocks_begun.checked_sub(1).map(SceneIndex::new)
}

/// Whether text of `text_width` can be written in a block of `block_width`
/// with some room at both ends.
fn label_fits(text_width: f32, block_width: f32) -> bool {
    text_width + 2.0 * TIMELINE_LABEL_PADDING <= block_width
}

fn paint_timeline_block(
    painter: &egui::Painter,
    block: egui::Rect,
    scene: SceneIndex,
    layout: &SceneLayout,
) {
    let palette = theme::palette(painter.ctx());
    painter.rect_filled(
        block,
        TIMELINE_ROUNDING,
        palette.scene_color(scene.position()),
    );
    if is_long_scene(layout.span) {
        let outline = egui::Stroke::new(LONG_SCENE_OUTLINE_WIDTH, palette.danger);
        painter.rect_stroke(block, TIMELINE_ROUNDING, outline);
    }
    if block.width() < TIMELINE_MIN_LABELLED_WIDTH {
        return;
    }
    let on_fill = palette.on_fill;
    let heavier = theme::semibold_font(TIMELINE_LABEL_FONT_SIZE);
    let name = painter.layout_no_wrap(scene_name(scene), heavier.clone(), on_fill);
    let number = painter.layout_no_wrap(scene.number().to_string(), heavier, on_fill);
    // Lighter than the name, so the two read as a name and a figure and not
    // as one run of numbers.
    let lighter = egui::FontId::proportional(TIMELINE_LABEL_FONT_SIZE);
    let seconds = layout.span.seconds();
    let length = painter.layout_no_wrap(format_length(seconds), lighter.clone(), on_fill);
    let short_length = painter.layout_no_wrap(format_clock(seconds), lighter, on_fill);
    let widths = TimelineLabelWidths {
        name: name.size().x,
        length: length.size().x,
        short_length: short_length.size().x,
        number: number.size().x,
    };
    let paint_centred = |text: std::sync::Arc<egui::Galley>| {
        painter.galley(block.center() - text.size() / 2.0, text, on_fill);
    };
    let paint_name_with = |length: std::sync::Arc<egui::Galley>| {
        let both = widths.name + TIMELINE_LABEL_GAP + length.size().x;
        let left = block.center().x - both / 2.0;
        let length_left = left + widths.name + TIMELINE_LABEL_GAP;
        let middle = block.center().y;
        let name_top = middle - name.size().y / 2.0;
        painter.galley(egui::pos2(left, name_top), name.clone(), on_fill);
        let length_top = middle - length.size().y / 2.0;
        painter.galley(egui::pos2(length_left, length_top), length, on_fill);
    };
    match timeline_label(widths, block.width()) {
        TimelineLabel::NameAndLength => paint_name_with(length),
        TimelineLabel::NameAndShortLength => paint_name_with(short_length),
        TimelineLabel::Name => paint_centred(name.clone()),
        TimelineLabel::Number => paint_centred(number),
        TimelineLabel::Nothing => {}
    }
}

/// The block of a scene that is not written: an outline with no colour in
/// it, where every written scene is a block of solid colour, and the word
/// for it. A block too narrow for the word is struck through instead, so
/// the difference never rests on colour alone.
fn paint_dropped_timeline_block(painter: &egui::Painter, block: egui::Rect, scene: SceneIndex) {
    let palette = theme::palette(painter.ctx());
    let outline = egui::Stroke::new(1.0_f32, palette.dropped);
    painter.rect_filled(block, TIMELINE_ROUNDING, palette.dropped_fill);
    painter.rect_stroke(block, TIMELINE_ROUNDING, outline);
    if block.width() >= TIMELINE_MIN_LABELLED_WIDTH {
        let font = theme::semibold_font(TIMELINE_LABEL_FONT_SIZE);
        let labels = [
            format!("{} dropped", scene_name(scene)),
            DROPPED_SHORT_LABEL.to_owned(),
        ];
        for label in labels {
            let text = painter.layout_no_wrap(label, font.clone(), palette.muted_text);
            if label_fits(text.size().x, block.width()) {
                painter.galley(block.center() - text.size() / 2.0, text, palette.muted_text);
                return;
            }
        }
    }
    painter.line_segment([block.left_bottom(), block.right_top()], outline);
}

fn scene_name(scene: SceneIndex) -> String {
    format!("Scene {}", scene.number())
}

/// How much a timeline block says about its scene. A bare number is the
/// last resort: beside a length it reads as part of the length.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TimelineLabel {
    NameAndLength,
    /// The length as a player shows it, "2:40", which takes half the room.
    NameAndShortLength,
    Name,
    Number,
    Nothing,
}

/// How wide each piece of a timeline label is once drawn.
#[derive(Debug, Clone, Copy)]
struct TimelineLabelWidths {
    name: f32,
    length: f32,
    short_length: f32,
    number: f32,
}

impl TimelineLabelWidths {
    fn name_and_length(self) -> f32 {
        self.name + TIMELINE_LABEL_GAP + self.length
    }

    fn name_and_short_length(self) -> f32 {
        self.name + TIMELINE_LABEL_GAP + self.short_length
    }
}

/// The most a block of `block_width` can say without its label hanging over
/// the edge, where it would name the wrong scene.
fn timeline_label(widths: TimelineLabelWidths, block_width: f32) -> TimelineLabel {
    if label_fits(widths.name_and_length(), block_width) {
        return TimelineLabel::NameAndLength;
    }
    if label_fits(widths.name_and_short_length(), block_width) {
        return TimelineLabel::NameAndShortLength;
    }
    if label_fits(widths.name, block_width) {
        return TimelineLabel::Name;
    }
    if label_fits(widths.number, block_width) {
        return TimelineLabel::Number;
    }
    TimelineLabel::Nothing
}

/// What a preview says when the pointer rests on it: what a click does and,
/// for a preview that could not be made, why there is no picture.
fn preview_hover_text(time: f64, why_no_preview: Option<&str>) -> String {
    let play = format!("Play from {}", format_clock(time));
    match why_no_preview {
        Some(reason) => format!("{play}\nThe preview could not be made: {reason}"),
        None => play,
    }
}

/// Whether the part a picture belongs to is among the rows that are drawn.
fn shows_part_of(rows: &[PartRow], picture: PicturePosition) -> bool {
    rows.contains(&PartRow::Part(focus::part_of(picture)))
}

/// Frames what the keys act on, just outside its own edge.
fn paint_focus_ring(painter: &egui::Painter, around: egui::Rect, rounding: f32) {
    let palette = theme::palette(painter.ctx());
    let half_width = FOCUS_RING_WIDTH / 2.0;
    painter.rect_stroke(
        around.expand(half_width),
        rounding + half_width,
        egui::Stroke::new(FOCUS_RING_WIDTH, palette.focus_ring),
    );
}

fn paint_picture_focus(painter: &egui::Painter, picture: egui::Rect) {
    let palette = theme::palette(painter.ctx());
    let half_width = PICTURE_FOCUS_INNER_WIDTH / 2.0;
    painter.rect_stroke(
        picture.shrink(half_width),
        PREVIEW_ROUNDING,
        egui::Stroke::new(PICTURE_FOCUS_INNER_WIDTH, palette.focus_ring_inner),
    );
    paint_focus_ring(painter, picture, PREVIEW_ROUNDING);
}

/// Whether the key that shows or hides the list of keys was pressed.
fn keys_list_key_pressed(ctx: &egui::Context, keyboard: KeyboardOwner) -> bool {
    keyboard.lets_screens_use_keys(ctx) && ctx.input(|input| input.key_pressed(egui::Key::F1))
}

/// What the keys that work on the focused scene ask for: moving the focus,
/// playing, joining, taking apart, moving a split, and starting the split.
fn focus_key_requests(
    ctx: &egui::Context,
    keyboard: KeyboardOwner,
    session: &ReviewSession,
) -> Vec<ReviewRequest> {
    if !keyboard.lets_screens_use_keys(ctx) {
        return Vec::new();
    }
    let a_control_has_the_keys = keyboard::a_control_has_the_keys(ctx);
    ctx.input(|input| {
        let mut requests: Vec<ReviewRequest> = letter_key_request(input).into_iter().collect();
        // Enter and Space press a control that was reached with Tab, and
        // the arrow keys move on from it. They are not also read here.
        if a_control_has_the_keys {
            return requests;
        }
        requests.extend(scene_step_request(input));
        requests.extend(left_or_right_request(input, session));
        requests.extend(enter_or_space_request(input, session));
        requests
    })
}

/// Whether `letter` was pressed by itself. With Ctrl or Alt held a letter
/// is some other shortcut, the system's or a later one of this screen.
fn letter_pressed(input: &egui::InputState, letter: egui::Key) -> bool {
    !input.modifiers.command && !input.modifiers.alt && input.key_pressed(letter)
}

fn letter_key_request(input: &egui::InputState) -> Option<ReviewRequest> {
    if input.modifiers.command || input.modifiers.alt {
        return None;
    }
    if input.key_pressed(egui::Key::J) {
        return Some(ReviewRequest::JoinFocusedWithNext);
    }
    input
        .key_pressed(egui::Key::U)
        .then_some(ReviewRequest::TakeSelectedApart)
}

/// Up, Down, Home and End move the focus. With Shift held the selection
/// reaches from where it began to the scene the focus lands on.
fn scene_step_request(input: &egui::InputState) -> Option<ReviewRequest> {
    if input.modifiers.command || input.modifiers.alt {
        return None;
    }
    let step = [
        (egui::Key::ArrowUp, SceneStep::Previous),
        (egui::Key::ArrowDown, SceneStep::Next),
        (egui::Key::Home, SceneStep::First),
        (egui::Key::End, SceneStep::Last),
    ]
    .into_iter()
    .find(|(key, _)| input.key_pressed(*key))
    .map(|(_, step)| step)?;
    let kind = if input.modifiers.shift {
        ClickKind::ExtendRange
    } else {
        ClickKind::Single
    };
    Some(ReviewRequest::StepFocus { step, kind })
}

/// Left and Right choose a picture of the focused scene. With Alt held they
/// move the split that scene starts at.
fn left_or_right_request(
    input: &egui::InputState,
    session: &ReviewSession,
) -> Option<ReviewRequest> {
    let (picture_step, direction) = if input.key_pressed(egui::Key::ArrowLeft) {
        (PictureStep::Previous, NudgeDirection::Earlier)
    } else if input.key_pressed(egui::Key::ArrowRight) {
        (PictureStep::Next, NudgeDirection::Later)
    } else {
        return None;
    };
    if input.modifiers.is_none() {
        return Some(ReviewRequest::StepPicture(picture_step));
    }
    if input.modifiers != egui::Modifiers::ALT {
        return None;
    }
    let scene = session.focus()?.scene();
    Some(ReviewRequest::NudgeSplit { scene, direction })
}

/// Enter and Space play the focused scene. Ctrl+Enter starts the split
/// and Ctrl+Shift+Enter puts the video with those to split later. Both go
/// through the same questions as their buttons.
fn enter_or_space_request(
    input: &egui::InputState,
    session: &ReviewSession,
) -> Option<ReviewRequest> {
    let enter = input.key_pressed(egui::Key::Enter);
    if enter && input.modifiers.command {
        let request = if input.modifiers.shift {
            ReviewRequest::SplitLater
        } else {
            ReviewRequest::Split
        };
        // One scene is the untouched video, and the buttons are off as well.
        return (session.scene_count() > 1).then_some(request);
    }
    let play = enter || input.key_pressed(egui::Key::Space);
    (play && input.modifiers.is_none()).then_some(ReviewRequest::PlayFocused)
}

/// What the keys pressed in this frame ask for.
fn shortcut_requests(ctx: &egui::Context, keyboard: KeyboardOwner) -> Vec<ReviewRequest> {
    // Behind a dialog the keys are the dialog's, and keys typed into a text
    // box belong to the text box.
    if !keyboard.lets_screens_use_keys(ctx) {
        return Vec::new();
    }
    let (merge, select_all, undo, drop_or_keep) = ctx.input(|input| {
        (
            letter_pressed(input, egui::Key::M),
            input.modifiers.command && input.key_pressed(egui::Key::A),
            input.modifiers.command && input.key_pressed(egui::Key::Z),
            letter_pressed(input, egui::Key::X),
        )
    });
    // This screen is behind everything else, so it is the last to be
    // offered Escape: the player, when open, has taken it already.
    let clear = keyboard::take_escape(ctx);
    [
        (select_all, ReviewRequest::SelectAllScenes),
        (clear, ReviewRequest::ClearSelection),
        (undo, ReviewRequest::Undo),
        (merge, ReviewRequest::MergeSelected),
        (drop_or_keep, ReviewRequest::DropOrKeepSelected),
    ]
    .into_iter()
    .filter(|(pressed, _)| *pressed)
    .map(|(_, request)| request)
    .collect()
}

/// The video's name, cut short if it is long, with the two things that end
/// the review at the right edge where they are always in view.
fn show_title_and_main_action(ui: &mut egui::Ui, session: &ReviewSession) -> Option<ReviewRequest> {
    let mut request = None;
    ui.horizontal(|ui| {
        // Laid out from the right, so the buttons keep their place and
        // the name takes whatever room is left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let scene_count = session.scene_count();
            let dropped_count = session.dropped_count();
            let label = split_button_label(scene_count - dropped_count, dropped_count);
            // One scene is the untouched video, there is nothing to write.
            let export_button = ui
                .add_enabled(scene_count > 1, theme::primary_button(ui, label))
                .on_hover_text(SPLIT_HINT)
                .on_disabled_hover_text("There is only one scene, so there is nothing to split.");
            if export_button.clicked() {
                request = Some(ReviewRequest::Split);
            }
            let later_button = ui
                .add_enabled(scene_count > 1, egui::Button::new(SPLIT_LATER_LABEL))
                .on_hover_text(SPLIT_LATER_EXPLANATION)
                .on_disabled_hover_text("There is only one scene, so there is nothing to split.");
            if later_button.clicked() {
                request = Some(ReviewRequest::SplitLater);
            }
            let skip_button = ui
                .button("Skip this video")
                .on_hover_text("Leaves this video as it is. Nothing is written.");
            if skip_button.clicked() {
                request = Some(ReviewRequest::Skip);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let name = file_name(session.video());
                ui.add(egui::Label::new(theme::semibold(&name, FILE_NAME_FONT_SIZE)).truncate())
                    .on_hover_text(name);
            });
        });
    });
    request
}

/// What the split button says: how many files it writes and, when scenes
/// are dropped, how many scenes it leaves out.
fn split_button_label(file_count: usize, dropped_count: usize) -> String {
    let files = format!("Split into {}", counted(file_count, "file"));
    if dropped_count == 0 {
        return files;
    }
    format!("{files}, {} dropped", counted(dropped_count, "scene"))
}

/// What the pointer learns over the sentence that stands in for the choice
/// about the original: what the check still does, and how to get the choice
/// back.
fn original_always_kept_hint(why: OriginalAlwaysKept) -> String {
    let keep_every_scene = "The check still compares every new file with its part of the original.";
    let get_the_choice_back = format!(
        "Turn off \"{}\" in Settings to choose what happens to the original again.",
        settings_controls::EXACT_CUT_LABEL
    );
    match why {
        OriginalAlwaysKept::ScenesDropped => keep_every_scene.to_owned(),
        OriginalAlwaysKept::CutIsExact => get_the_choice_back,
        // Here turning the setting off is only half of the way back.
        OriginalAlwaysKept::ScenesDroppedAndCutIsExact => format!(
            "{keep_every_scene} You choose what happens to the original again once no scene \
             is dropped and \"{}\" is off in Settings.",
            settings_controls::EXACT_CUT_LABEL
        ),
    }
}

/// What the pointer reads while a scene card is dragged over nothing it
/// can be merged with. "Drag", since to drop a scene means to leave it out
/// of the split on this screen.
fn drag_hint(dragged: SceneIndex) -> String {
    format!(
        "Drag scene {} onto another scene to merge them",
        dragged.number()
    )
}

/// The first of `named`, then how many more there are: "a, b, c and 9
/// more". All of them when they are few.
fn first_of(named: &[String], most: usize) -> String {
    let shown = named.iter().take(most).cloned().collect::<Vec<_>>();
    match named.len().saturating_sub(most) {
        0 => shown.join(", "),
        more => format!("{} and {more} more", shown.join(", ")),
    }
}

/// Which scene files a split would replace, by name, so that a file that
/// only looks like a scene of this video is seen before it is gone.
fn earlier_files_warning(earlier_files: &EarlierSceneFiles) -> String {
    format!(
        "{} from an earlier split would be replaced: {}.",
        counted(earlier_files.file_count(), "scene file"),
        first_of(&earlier_files.file_names, MOST_NAMED_IN_A_WARNING)
    )
}

/// Every file of the warning, one to a line, and where they are.
fn earlier_files_hover(earlier_files: &EarlierSceneFiles) -> String {
    let mut lines = vec![format!("In {}:", earlier_files.scene_dir.display())];
    lines.extend(
        earlier_files
            .file_names
            .iter()
            .take(MOST_NAMED_IN_A_HOVER)
            .cloned(),
    );
    let more = earlier_files
        .file_count()
        .saturating_sub(MOST_NAMED_IN_A_HOVER);
    if more > 0 {
        lines.push(format!("and {more} more"));
    }
    lines.push(format!(
        "Files with names the new split does not use go to the {TRASH_NAME}."
    ));
    lines.join("\n")
}

/// The times of some splits, as a warning names them.
fn split_times_in_words(times: &[f64]) -> String {
    let stamps: Vec<String> = times.iter().map(|&time| format_timestamp(time)).collect();
    first_of(&stamps, MOST_NAMED_IN_A_WARNING)
}

/// Why a copy cannot make the splits at `times`, and the two ways on.
fn splits_off_cut_points_warning(times: &[f64]) -> String {
    let which = match times.len() {
        1 => "1 split is on an exact picture".to_owned(),
        several => format!("{several} splits are on exact pictures"),
    };
    format!(
        "{which}, where the video can only be cut with \"{}\" on: at {}. \
         Move {} to the nearest {} where the video can be cut, or turn that setting back on.",
        settings_controls::EXACT_CUT_LABEL,
        split_times_in_words(times),
        if times.len() == 1 { "it" } else { "them" },
        if times.len() == 1 { "point" } else { "points" },
    )
}

fn move_splits_label(split_count: usize) -> &'static str {
    if split_count == 1 {
        "Move it to the nearest point"
    } else {
        "Move them to the nearest points"
    }
}

/// Why the splits at `times` stayed where they are, and what is left to do.
fn splits_stuck_warning(times: &[f64]) -> String {
    let (which, each) = match times.len() {
        1 => ("1 split cannot move", "it"),
        _ => ("Some splits cannot move", "each of them"),
    };
    format!(
        "{which}, because another split is at the nearest point where the video can be cut: \
         at {}. Merge the scenes on both sides of {each}, or turn \"{}\" back on in Settings.",
        split_times_in_words(times),
        settings_controls::EXACT_CUT_LABEL
    )
}

/// Why the split has not started, with the way past it when there is one.
fn show_export_obstacle(ui: &mut egui::Ui, session: &ReviewSession) -> Option<ReviewRequest> {
    let palette = theme::palette(ui.ctx());
    let mut request = None;
    match session.export_obstacle()? {
        ExportObstacle::EarlierSceneFiles(earlier_files) => {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(palette.warning_text, earlier_files_warning(earlier_files))
                    .on_hover_text(earlier_files_hover(earlier_files));
                let replace = theme::danger_button(ui, REPLACE_EARLIER_FILES_LABEL);
                if ui.add(replace).clicked() {
                    // The answer is for the files in this folder and no other.
                    request = Some(ReviewRequest::ReplaceEarlierFiles {
                        scene_dir: earlier_files.scene_dir.clone(),
                    });
                }
            });
        }
        ExportObstacle::CouldNotCheck(problem) => {
            ui.horizontal_wrapped(|ui| {
                ui.add(theme::selectable(
                    egui::RichText::new(problem).color(palette.danger),
                ));
                theme::copy_details_button(ui, problem);
            });
        }
        ExportObstacle::SplitsOffCutPoints(times) => {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(palette.warning_text, splits_off_cut_points_warning(times));
                // Not a danger button: Undo takes the move back.
                if ui.button(move_splits_label(times.len())).clicked() {
                    request = Some(ReviewRequest::MoveSplitsToCutPoints);
                }
            });
        }
        ExportObstacle::SplitsStuckOffCutPoints(times) => {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(palette.warning_text, splits_stuck_warning(times));
            });
        }
    }
    request
}

/// Says before the split that this video will not be cut on the exact
/// pictures, why, and how it will be cut instead. Nothing is shown for a
/// video that can be, or while exact cuts are off.
fn show_exact_cut_notice(ui: &mut egui::Ui, session: &ReviewSession, cut_precision: CutPrecision) {
    let Some(notice) = session.exact_cut_notice_for(cut_precision) else {
        return;
    };
    let palette = theme::palette(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(palette.warning_text, notice);
    });
}

/// What happens to the original and where the scenes go, right under the
/// button they belong to. `export_options` are the saved ones. A change is
/// returned for the app to save.
///
/// A split that drops scenes never moves the original, whatever is saved.
/// So while scenes are dropped the choice about it gives way to a sentence
/// that says so. The saved choice is left as it is.
fn show_export_options(
    ui: &mut egui::Ui,
    video: &Path,
    export_options: ExportOptions,
    dropped_count: usize,
) -> Option<ReviewRequest> {
    let mut wanted = export_options;
    ui.horizontal_wrapped(|ui| {
        // The original is kept whatever was chosen for it, so the choice
        // gives way to saying so, before the split.
        match OriginalAlwaysKept::for_split(dropped_count, export_options.cut_precision) {
            Some(why) => {
                ui.label(theme::muted(ui, why.in_plain_words()))
                    .on_hover_text(original_always_kept_hint(why));
            }
            None => {
                let recycle_label = format!(
                    "Then move the original to the {TRASH_NAME} (only if the new files pass the check)"
                );
                settings_controls::show_original_policy_checkbox(
                    ui,
                    &mut wanted.original_policy,
                    recycle_label,
                )
                .on_hover_text(format!(
                    "The check compares the new files with the original. If anything is missing \
                     or different, the original stays where it is. You can restore it from the {TRASH_NAME}."
                ));
            }
        }
        settings_controls::show_scene_location_checkbox(ui, &mut wanted.scene_location);
        if let Ok(dir) = split::scene_dir(video, wanted.scene_location) {
            let destination = dir.display().to_string();
            ui.add(
                // Body size, so it lines up with the checkbox text before it.
                egui::Label::new(theme::muted(ui, format!("Scenes go to {destination}"))).truncate(),
            )
            .on_hover_text(destination);
        }
    });
    (wanted != export_options).then_some(ReviewRequest::ChangeSettings(
        SettingsChange::ExportOptions(wanted),
    ))
}

fn show_merge_controls(ui: &mut egui::Ui, session: &ReviewSession) -> Option<ReviewRequest> {
    let mut request = None;
    let span_to_merge = session.selection().span_to_merge();
    let merge_label = match &span_to_merge {
        Some(span) => merge_description(span),
        None => MERGE_SELECTED_LABEL.to_owned(),
    };
    let merge_button = ui
        .add_enabled(span_to_merge.is_some(), egui::Button::new(merge_label))
        .on_hover_text(MERGE_SELECTED_HINT)
        .on_disabled_hover_text(
            "Click a scene, then Shift-click another, to choose the scenes to merge.",
        );
    if merge_button.clicked() {
        request = Some(ReviewRequest::MergeSelected);
    }

    let undo_merge_button = ui
        .add_enabled(
            session.has_merged_scene_selected(),
            egui::Button::new(TAKE_APART_LABEL),
        )
        .on_hover_text(TAKE_APART_HINT)
        .on_disabled_hover_text("Select a merged scene to take it apart.");
    if undo_merge_button.clicked() {
        request = Some(ReviewRequest::TakeSelectedApart);
    }
    request
}

fn show_undo_and_reset(ui: &mut egui::Ui, session: &ReviewSession) -> Option<ReviewRequest> {
    let mut request = None;
    let undo_clicked = ui
        .add_enabled(session.can_undo(), egui::Button::new("Undo"))
        .on_hover_text("Takes back the last change (Ctrl+Z).")
        .on_disabled_hover_text("Nothing to take back yet.")
        .clicked();
    if undo_clicked {
        request = Some(ReviewRequest::Undo);
    }

    let reset_clicked = ui
        .add_enabled(
            !session.plan().is_as_detected(),
            egui::Button::new("Reset scenes"),
        )
        .on_hover_text("Undoes every merge and removes the splits you added.")
        .on_disabled_hover_text("The scenes are as the scan found them.")
        .clicked();
    if reset_clicked {
        request = Some(ReviewRequest::ResetScenes);
    }
    request
}

fn show_new_split_controls(ui: &mut egui::Ui, session: &ReviewSession) -> Vec<ReviewRequest> {
    let palette = theme::palette(ui.ctx());
    let mut requests = Vec::new();
    // One unit, so a narrow window wraps all three together.
    ui.horizontal(|ui| {
        ui.label("Add a split at");
        // A hint that is a real time in this video, not one beyond its end.
        let example_time = format_clock(session.plan().whole_video().seconds() / 2.0);
        // The box edits a copy of what the session holds. What the person
        // types goes back as a request, like everything else they do here.
        let mut typed_time = session.new_split_time().to_owned();
        let field = ui.add(
            egui::TextEdit::singleline(&mut typed_time)
                .hint_text(example_time)
                .desired_width(NEW_CUT_FIELD_WIDTH),
        );
        if field.changed() {
            requests.push(ReviewRequest::TypeNewSplitTime(typed_time));
        }
        let pressed_enter =
            field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if ui.button("Add split").clicked() || pressed_enter {
            requests.push(ReviewRequest::AddTypedSplit);
        }
    });
    if let Some(feedback) = session.new_split_feedback() {
        // A split that was not added is said in the colour of a warning, so
        // it is not taken for one that was.
        let words = if feedback.is_refusal() {
            egui::RichText::new(feedback.in_words()).color(palette.warning_text)
        } else {
            theme::muted(ui, feedback.in_words())
        };
        ui.label(words);
    }
    requests
}

/// The quiet way to save the list of scenes without splitting: a link that
/// opens the three formats, then the system's dialog for where to save.
/// A link that starts the next row when this one has no room for all of it.
/// Broken over two rows, one link reads as two.
fn whole_link(ui: &mut egui::Ui, label: &str) -> egui::Response {
    start_next_row_unless_it_fits(ui, label);
    ui.link(label)
}

fn start_next_row_unless_it_fits(ui: &mut egui::Ui, label: &str) {
    let width = egui::WidgetText::from(label)
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x;
    if ui.available_size_before_wrap().x < width {
        ui.end_row();
    }
}

fn show_save_scene_list_menu(ui: &mut egui::Ui, video: &Path) -> Option<ReviewRequest> {
    let mut request = None;
    start_next_row_unless_it_fits(ui, SAVE_SCENE_LIST_LABEL);
    let link_color = ui.visuals().hyperlink_color;
    ui.scope(|ui| {
        // Drawn without a frame, in the colour of a link and no taller than
        // its text, to sit level with the links beside it. A full button
        // for a rare action would look out of place on this line.
        ui.visuals_mut().button_frame = false;
        ui.spacing_mut().button_padding = egui::Vec2::ZERO;
        ui.spacing_mut().interact_size.y = 0.0;
        let label = egui::RichText::new(SAVE_SCENE_LIST_LABEL).color(link_color);
        ui.menu_button(label, |ui| {
            for format in SceneListFormat::ALL {
                if ui.button(format.label()).clicked() {
                    request = ask_where_to_save_scene_list(video, format)
                        .map(|path| ReviewRequest::SaveSceneList { format, path });
                    ui.close_menu();
                }
            }
        })
        .response
        .on_hover_text(SAVE_SCENE_LIST_HINT);
    });
    request
}

/// Opens the system's dialog for saving a file, which also asks before an
/// existing file is replaced. It blocks until it is closed, which is what a
/// person expects of it. Returns `None` when they backed out.
fn ask_where_to_save_scene_list(video: &Path, format: SceneListFormat) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title(SAVE_SCENE_LIST_DIALOG_TITLE)
        .add_filter(format.label(), &[format.extension()])
        .set_file_name(scene_list_file_name(video, format));
    if let Some(folder) = video
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        dialog = dialog.set_directory(folder);
    }
    dialog.save_file()
}

/// The name offered for a scene list: "<video name> scenes.csv".
fn scene_list_file_name(video: &Path, format: SceneListFormat) -> String {
    let video_name = video
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{video_name} scenes.{}", format.extension())
}

/// How the last saving of the scene list went.
fn show_scene_list_note(ui: &mut egui::Ui, session: &ReviewSession) {
    let palette = theme::palette(ui.ctx());
    match session.scene_list_note() {
        None => {}
        Some(SceneListNote::Saved(path)) => {
            let saved_to = path.display().to_string();
            ui.add(
                egui::Label::new(theme::muted(
                    ui,
                    format!("Saved the scene list to {saved_to}"),
                ))
                .truncate(),
            )
            .on_hover_text(saved_to);
        }
        Some(SceneListNote::NotSaved(reason)) => {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(palette.warning_text, SCENE_LIST_NOT_SAVED);
                ui.add(theme::selectable(theme::caption(ui, reason)));
                theme::copy_details_button(ui, reason);
            });
        }
    }
}

/// Asks before a scan throws away the person's edits.
fn show_rescan_consent(ui: &mut egui::Ui, session: &ReviewSession) -> Option<ReviewRequest> {
    let palette = theme::palette(ui.ctx());
    if !session.is_asking_before_rescan() {
        return None;
    }
    let mut request = None;
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(palette.warning_text, RESCAN_WARNING);
        if ui
            .add(theme::danger_button(ui, "Scan again anyway"))
            .clicked()
        {
            request = Some(ReviewRequest::RescanAnyway);
        }
        if ui.button("Cancel").clicked() {
            request = Some(ReviewRequest::KeepEdits);
        }
    });
    request
}

/// The key numbers and what deserves attention, at the top of the part that
/// scrolls away above the scenes.
fn show_notes(
    ui: &mut egui::Ui,
    session: &ReviewSession,
    detection: &DetectionInUse,
    settings_card: SettingsCard,
) -> Vec<ReviewRequest> {
    let palette = theme::palette(ui.ctx());
    let plan = session.plan();
    let mut requests = Vec::new();
    if settings_card != SettingsCard::Hidden {
        let (card, asked) = show_black_screen_settings_card(ui, session.video(), detection);
        requests.extend(asked);
        if settings_card == SettingsCard::JustOpened {
            ui.scroll_to_rect(card, Some(egui::Align::TOP));
        }
    }
    ui.horizontal_wrapped(|ui| show_key_numbers(ui, plan));
    if session.scene_count() == 1 && show_nothing_found_note(ui) == LookHarder::Asked {
        requests.push(ReviewRequest::Rescan(look_harder_scan(
            detection.detection.hard_cuts,
        )));
    }
    for stretch in session.undecoded() {
        show_note(ui, &undecoded_warning(stretch), palette.rose_tile);
    }
    for (position, span) in plan.scenes().into_iter().enumerate() {
        if is_long_scene(span) {
            let warning = format!(
                "Scene {} is {} long. A scene change may have been missed inside it.",
                SceneIndex::new(position).number(),
                format_length(span.seconds())
            );
            show_note(ui, &warning, palette.rose_tile);
        }
    }
    requests
}

/// The black screen settings this video is scanned with, whose they are,
/// and the way to give the video's folder settings of its own or take them
/// away again. Returns where the card was drawn and what was asked for. A
/// changed setting is asked for with the rest of the detection settings as
/// they are, for the app to store where they belong.
fn show_black_screen_settings_card(
    ui: &mut egui::Ui,
    video: &Path,
    in_use: &DetectionInUse,
) -> (egui::Rect, Vec<ReviewRequest>) {
    let mut requests = Vec::new();
    let mut wanted = in_use.detection.scan;
    let card = theme::card(ui)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::semibold(BLACK_SCREEN_SETTINGS_HEADING, 14.0));
            requests.extend(show_settings_scope(ui, video, &in_use.scope));
            settings_controls::show_black_screen_controls(ui, &mut wanted);
            ui.label(theme::caption(ui, BLACK_SCREEN_SETTINGS_APPLY_NOTE));
        })
        .response
        .rect;
    if wanted != in_use.detection.scan {
        requests.push(ReviewRequest::ChangeDetection(DetectionSettings {
            scan: wanted,
            ..in_use.detection
        }));
    }
    (card, requests)
}

/// Says plainly whose detection settings this video is scanned with, beside
/// the button that changes that.
fn show_settings_scope(
    ui: &mut egui::Ui,
    video: &Path,
    scope: &SettingsScope,
) -> Option<ReviewRequest> {
    let mut request = None;
    ui.horizontal_wrapped(|ui| match scope {
        SettingsScope::General => {
            ui.label(theme::muted(ui, GENERAL_SETTINGS_IN_USE));
            let folder = folder_settings::folder_of(video);
            let remember = ui
                .button(REMEMBER_FOR_FOLDER_LABEL)
                .on_hover_text(remember_for_folder_hint(folder.as_deref()));
            if remember.clicked() {
                request = Some(ReviewRequest::RememberForFolder);
            }
        }
        SettingsScope::Folder(folder) => {
            let folder = folder.display().to_string();
            ui.label(theme::muted(ui, FOLDER_SETTINGS_IN_USE))
                .on_hover_text(&folder);
            let use_general = ui.button(USE_GENERAL_SETTINGS_LABEL).on_hover_text(format!(
                "Forgets the scan settings remembered for {folder}. \
                     Its videos are scanned with the general settings again."
            ));
            if use_general.clicked() {
                request = Some(ReviewRequest::UseGeneralSettings);
            }
        }
    });
    request
}

/// The scan the "Look harder" button starts. The button promises to find
/// even subtle changes, so this one scan is set to do that. The sensitivity
/// the person saved for other scans is left as it is.
fn look_harder_scan(saved: HardCutSettings) -> ScanKind {
    ScanKind::HardCuts(HardCutSettings {
        sensitivity: Sensitivity::High,
        ..saved
    })
}

/// Tells the person about a stretch the scan could not read, where a scene
/// change may have gone unnoticed.
fn undecoded_warning(stretch: &UndecodedFrames) -> String {
    format!(
        "{} of the {} pictures from {} on could not be read. \
         A scene change there may have been missed.",
        stretch.missing_frames,
        stretch.stretch_frames,
        format_clock(stretch.from_seconds)
    )
}

/// The numbers a person wants at a glance: how many scenes the split would
/// write, how long the video is, and how long its longest scene is.
fn show_key_numbers(ui: &mut egui::Ui, plan: &SplitPlan) {
    let palette = theme::palette(ui.ctx());
    let scenes = plan.scenes();
    let piece_count = plan.segments().len();
    let longest_scene = scenes
        .iter()
        .copied()
        .max_by(|a, b| a.seconds().total_cmp(&b.seconds()));

    let scene_noun = if scenes.len() == 1 { "scene" } else { "scenes" };
    theme::stat_tile(
        ui,
        &scenes.len().to_string(),
        scene_noun,
        palette.mint_tile,
        palette.text,
    );
    theme::stat_tile(
        ui,
        &format_length(plan.whole_video().seconds()),
        "whole video",
        palette.lavender_tile,
        palette.text,
    );
    if let Some(longest) = longest_scene {
        let (tint, value_color) = if is_long_scene(longest) {
            (palette.rose_tile, palette.danger)
        } else {
            (palette.peach_tile, palette.text)
        };
        theme::stat_tile(
            ui,
            &format_length(longest.seconds()),
            "longest scene",
            tint,
            value_color,
        );
    }
    let dropped_count = plan.scenes_to_write().dropped_count();
    if dropped_count > 0 {
        theme::stat_tile(
            ui,
            &dropped_count.to_string(),
            DROPPED_TILE_CAPTION,
            palette.dropped_fill,
            palette.muted_text,
        );
    }
    if piece_count > scenes.len() {
        theme::stat_tile(
            ui,
            &piece_count.to_string(),
            "parts, merged into these scenes",
            palette.page,
            palette.muted_text,
        );
    }
}

/// What the person asked for on the tips card.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TipsCard {
    HideAsked,
    /// Show the list of keys, or hide it again.
    KeysListToggled,
    Kept,
}

/// How to use the review screen, for someone seeing it for the first time.
/// It scrolls away with the scenes, can be hidden, and the choice is kept.
fn show_tips_card(ui: &mut egui::Ui, keys_list: KeysList) -> TipsCard {
    let palette = theme::palette(ui.ctx());
    let mut tips_card = TipsCard::Kept;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(theme::semibold("How to use this screen", 14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Hide tips").clicked() {
                    tips_card = TipsCard::HideAsked;
                }
                let keys_label = match keys_list {
                    KeysList::Hidden => SHOW_KEYS_LABEL,
                    KeysList::Shown => HIDE_KEYS_LABEL,
                };
                if ui
                    .button(keys_label)
                    .on_hover_text(KEYS_LINK_HINT)
                    .clicked()
                {
                    tips_card = TipsCard::KeysListToggled;
                }
            });
        });
        // A grid cell does not wrap unless it is told how wide it may be.
        let room_for_a_tip =
            (ui.available_width() - TIP_TOPIC_WIDTH - TIP_COLUMN_GAP).max(MIN_TIP_WIDTH);
        egui::Grid::new("tips")
            .num_columns(2)
            // Rows of text need not be as tall as rows of buttons.
            .min_row_height(0.0)
            .min_col_width(TIP_TOPIC_WIDTH)
            .max_col_width(room_for_a_tip)
            .show(ui, |ui| {
                for (topic, tip) in TIPS {
                    ui.label(theme::semibold(topic, 13.0).color(palette.primary));
                    ui.add(egui::Label::new(tip).wrap());
                    ui.end_row();
                }
            });
    });
    tips_card
}

/// What drawing the list of keys gave.
struct KeysCard {
    hide_asked: bool,
    /// Where the card was drawn, for scrolling it into view.
    area: egui::Rect,
}

/// Every key of the review screen with what it does, for a person who
/// reviews without the mouse.
fn show_keys_card(ui: &mut egui::Ui) -> KeysCard {
    let palette = theme::palette(ui.ctx());
    let mut hide_asked = false;
    let card = theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(theme::semibold(KEYS_HEADING, 14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                hide_asked = ui.button(HIDE_KEYS_LABEL).clicked();
            });
        });
        ui.add(egui::Label::new(theme::muted(ui, KEYS_INTRODUCTION)).wrap());
        if let Some(keys_on_this_system) = KEYS_ON_THIS_SYSTEM {
            ui.add(egui::Label::new(theme::muted(ui, keys_on_this_system)).wrap());
        }
        let room_for_an_action =
            (ui.available_width() - KEY_NAME_WIDTH - TIP_COLUMN_GAP).max(MIN_TIP_WIDTH);
        egui::Grid::new("keys")
            .num_columns(2)
            // Rows of text need not be as tall as rows of buttons.
            .min_row_height(0.0)
            .min_col_width(KEY_NAME_WIDTH)
            .max_col_width(room_for_an_action)
            .show(ui, |ui| {
                for (key, action) in KEYS {
                    ui.label(theme::semibold(key, 13.0).color(palette.primary));
                    ui.add(egui::Label::new(action).wrap());
                    ui.end_row();
                }
            });
    });
    KeysCard {
        hide_asked,
        area: card.response.rect,
    }
}

/// Whether the person pressed the button that scans a one-scene video again,
/// more thoroughly.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LookHarder {
    Asked,
    NotAsked,
}

/// Says that the video is still in one piece, with the one button that is
/// most likely to help.
fn show_nothing_found_note(ui: &mut egui::Ui) -> LookHarder {
    let palette = theme::palette(ui.ctx());
    let mut look_harder = LookHarder::NotAsked;
    egui::Frame::none()
        .fill(palette.peach_tile)
        .rounding(theme::CONTROL_ROUNDING)
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(NOTHING_FOUND_HINT);
            let button = ui
                .add(theme::primary_button(ui, LOOK_HARDER_LABEL))
                .on_hover_text(LOOK_HARDER_EXPLANATION);
            if button.clicked() {
                look_harder = LookHarder::Asked;
            }
            ui.label(theme::muted(ui, NOTHING_FOUND_BY_HAND_HINT));
        });
    look_harder
}

/// A line of advice on a tinted band, as wide as the list.
fn show_note(ui: &mut egui::Ui, text: &str, tint: egui::Color32) {
    egui::Frame::none()
        .fill(tint)
        .rounding(theme::CONTROL_ROUNDING)
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(text);
        });
}

/// What the buttons that move a split say they do, which depends on how
/// far a split moves.
fn move_split_hint(direction: NudgeDirection, step: NudgeStep) -> &'static str {
    match (direction, step) {
        (NudgeDirection::Earlier, NudgeStep::ToNextCutPoint) => MOVE_SPLIT_EARLIER_HINT,
        (NudgeDirection::Later, NudgeStep::ToNextCutPoint) => MOVE_SPLIT_LATER_HINT,
        (NudgeDirection::Earlier, NudgeStep::OnePicture { .. }) => {
            MOVE_SPLIT_ONE_PICTURE_EARLIER_HINT
        }
        (NudgeDirection::Later, NudgeStep::OnePicture { .. }) => MOVE_SPLIT_ONE_PICTURE_LATER_HINT,
    }
}

/// What the person asked for on the row between two scene cards.
#[derive(Debug, Clone, Copy, PartialEq)]
enum BetweenScenes {
    Join,
    MoveSplit(NudgeDirection),
}

/// What drawing the row between two scene cards gave.
struct JoinRow {
    asked: Option<BetweenScenes>,
    /// How much room the row took, for leaving the same room where a row is
    /// out of sight.
    height: f32,
}

/// What "Remember these settings for this folder" keeps, in the words the
/// screen uses for the two sets of settings, and for which folder.
fn remember_for_folder_hint(folder: Option<&Path>) -> String {
    let folder = match folder {
        Some(folder) => folder.display().to_string(),
        None => FOLDER_NOT_KNOWN.to_owned(),
    };
    format!(
        "Keeps the black screen settings and the settings for missed scenes as they are now \
         for the videos in {folder}, whatever the general settings become. \
         What you change here afterwards applies to that folder alone."
    )
}

/// The row between two scene cards, which is where the split between them
/// is: a small button to merge the two, and one at each end to move the
/// split, earlier at the left and later at the right. `nudge_step` is how
/// far those two move it, which their hover texts say.
fn show_join_row(ui: &mut egui::Ui, scene: SceneIndex, nudge_step: NudgeStep) -> JoinRow {
    let earlier_hint = move_split_hint(NudgeDirection::Earlier, nudge_step);
    let later_hint = move_split_hint(NudgeDirection::Later, nudge_step);
    let join_label = format!(
        "Merge scenes {} and {}",
        scene.number(),
        scene.next().number()
    );
    let mut asked = None;
    // One group at the left, where the card headings begin. Spread over the
    // width, the last button ran under the scroll bar and was cut off.
    let row = ui.horizontal_wrapped(|ui| {
        ui.add_space(BETWEEN_SCENES_INDENT);
        if show_quiet_button(ui, &join_label, JOIN_HINT) {
            asked = Some(BetweenScenes::Join);
        }
        ui.label(theme::caption(ui, MOVE_SPLIT_LABEL));
        if show_quiet_button(ui, MOVE_SPLIT_EARLIER_LABEL, earlier_hint) {
            asked = Some(BetweenScenes::MoveSplit(NudgeDirection::Earlier));
        }
        if show_quiet_button(ui, MOVE_SPLIT_LATER_LABEL, later_hint) {
            asked = Some(BetweenScenes::MoveSplit(NudgeDirection::Later));
        }
    });
    JoinRow {
        asked,
        height: row.response.rect.height(),
    }
}

/// A small button for the row between two cards. It has an outline, so it
/// reads as something to press, but it is lower than the buttons on a card.
/// Returns true when clicked.
fn show_quiet_button(ui: &mut egui::Ui, label: &str, hint: &str) -> bool {
    let text = egui::RichText::new(label).size(BETWEEN_SCENES_FONT_SIZE);
    ui.add(egui::Button::new(text).min_size(egui::vec2(0.0, BETWEEN_SCENES_BUTTON_HEIGHT)))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
        .clicked()
}

/// The button after one part of a merged scene that splits the scene in two
/// there again. A scene's nth merged cut follows its nth part.
fn show_separate_button(
    ui: &mut egui::Ui,
    plan: &SplitPlan,
    part: ScenePart,
    input: &mut SceneListInput,
) {
    let Some(&cut_index) = part.layout.merged_cuts.get(part.position_in_scene) else {
        return;
    };
    let label = format!(
        "Take apart here ({})",
        format_clock(plan.cut_time(cut_index))
    );
    let separate = ui.button(label).on_hover_text(TAKE_APART_HERE_HINT);
    if separate.clicked() {
        input.cut_to_split_again = Some(cut_index);
    }
}

/// Stands in for the middle parts of a long merged scene. Returns true when
/// the person asked to see every part.
fn show_hidden_parts_row(ui: &mut egui::Ui, hidden_count: usize, part_count: usize) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::muted(
            ui,
            format!(
                "{} in between are not shown.",
                counted(hidden_count, "part")
            ),
        ));
        ui.button(format!("Show all {part_count} parts"))
            .on_hover_text("Shows every part of this merged scene, with its pictures and buttons.")
            .clicked()
    })
    .inner
}

/// Under the last part of a long merged scene that shows every part.
/// Returns true when clicked.
fn show_fewer_parts_button(ui: &mut egui::Ui) -> bool {
    ui.button("Show fewer parts")
        .on_hover_text("Shows only the first and last parts of this merged scene again.")
        .clicked()
}

/// The options of the slower scan, for videos that change scene without a
/// black screen, and the button that starts it. `in_use` are the detection
/// settings this video is scanned with, as saved. A change to the options is
/// returned for the app to save where those settings belong, and so is the
/// wish to scan, with the options as they stand on screen.
fn show_hard_cut_scan_controls(ui: &mut egui::Ui, in_use: DetectionSettings) -> Vec<ReviewRequest> {
    let saved = in_use.hard_cuts;
    let mut wanted = saved;
    let mut scan_asked = false;
    ui.horizontal_wrapped(|ui| {
        settings_controls::show_hard_cut_controls(ui, &mut wanted, "review_hard_cut_sensitivity");
        scan_asked = ui
            .button("Look for sudden picture changes")
            .on_hover_text(
                "Looks at every picture of the video for places where it changes abruptly. \
                 This takes much longer than the first scan.",
            )
            .clicked();
    });
    // A person changes an option or presses the button, never both in one
    // frame, so at most one of these is asked for.
    let mut requests = Vec::new();
    if wanted != saved {
        requests.push(ReviewRequest::ChangeDetection(DetectionSettings {
            hard_cuts: wanted,
            ..in_use
        }));
    }
    if scan_asked {
        requests.push(ReviewRequest::Rescan(ScanKind::HardCuts(wanted)));
    }
    requests
}

/// How big to draw a preview so that a full row fits the width given.
fn preview_size_for(available_width: f32, gap: f32) -> egui::Vec2 {
    let gaps = gap * (PREVIEWS_PER_ROW - 1.0);
    let width = ((available_width - gaps) / PREVIEWS_PER_ROW)
        .floor()
        .clamp(MIN_PREVIEW_WIDTH, MAX_PREVIEW_WIDTH);
    egui::vec2(width, (width * PREVIEW_ASPECT).round())
}

fn click_kind(modifiers: egui::Modifiers) -> ClickKind {
    if modifiers.shift {
        ClickKind::ExtendRange
    } else if modifiers.command {
        ClickKind::Toggle
    } else {
        ClickKind::Single
    }
}

fn span_between(one_scene: SceneIndex, another_scene: SceneIndex) -> RangeInclusive<SceneIndex> {
    one_scene.min(another_scene)..=one_scene.max(another_scene)
}

/// Each scene's layout with the index of that scene.
fn with_scene_indices(layouts: &[SceneLayout]) -> impl Iterator<Item = (SceneIndex, &SceneLayout)> {
    layouts
        .iter()
        .enumerate()
        .map(|(position, layout)| (SceneIndex::new(position), layout))
}

fn merge_description(scenes: &RangeInclusive<SceneIndex>) -> String {
    format!(
        "Merge scenes {} to {}",
        scenes.start().number(),
        scenes.end().number()
    )
}

fn is_long_scene(span: TimeSpan) -> bool {
    span.seconds() > LONG_SCENE_SECONDS
}

/// What the timeline says about a scene under the pointer. `chapter_title`
/// is the title of the chapter that begins with the scene, if the file
/// names one.
fn scene_description(
    scene: SceneIndex,
    layout: &SceneLayout,
    chapter_title: Option<&str>,
    fate: SceneFate,
) -> String {
    let name = match chapter_title {
        Some(title) => format!("Scene {}   {title}", scene.number()),
        None => format!("Scene {}", scene.number()),
    };
    let description = format!(
        "{name}   {}   ({} to {})",
        format_length(layout.span.seconds()),
        format_timestamp(layout.span.start),
        format_timestamp(layout.span.end)
    );
    match fate {
        SceneFate::Keep => description,
        SceneFate::Drop => format!("{description}   {DROPPED_BADGE}"),
    }
}

/// A chapter title as a scene card shows it: whole when it is short, and
/// otherwise its beginning with a mark that it goes on.
fn title_for_card(title: &str) -> String {
    if title.chars().count() <= MOST_TITLE_CHARACTERS_SHOWN {
        return title.to_owned();
    }
    let beginning: String = title.chars().take(MOST_TITLE_CHARACTERS_SHOWN).collect();
    format!("{}{TITLE_CUT_SHORT_MARK}", beginning.trim_end())
}

/// The first row of a scene's card: which scene it is, how long, and what
/// the split does with it. Returns what the person asked to be done with
/// the scene instead, when they pressed the button for that.
fn show_scene_heading(
    ui: &mut egui::Ui,
    scene_card: SceneCard,
    color: egui::Color32,
    chapter_title: Option<&str>,
) -> Option<SceneFate> {
    let palette = theme::palette(ui.ctx());
    let SceneCard {
        scene,
        layout,
        fate,
    } = scene_card;
    let mut fate_wanted = None;
    ui.horizontal_wrapped(|ui| {
        let badge = theme::semibold(scene_name(scene), 13.0).color(palette.on_fill);
        theme::pill_at_least(ui, badge, color, SCENE_BADGE_MIN_WIDTH);
        if let Some(title) = chapter_title {
            ui.label(theme::semibold(
                title_for_card(title),
                CHAPTER_TITLE_FONT_SIZE,
            ))
            .on_hover_text(format!("Chapter: {title}"));
        }

        let length = theme::semibold(format_length(layout.span.seconds()), SCENE_LENGTH_FONT_SIZE);
        if is_long_scene(layout.span) {
            theme::pill(ui, length.color(palette.on_fill), palette.danger);
            // Said in words too, so the warning does not rest on colour alone.
            ui.label(egui::RichText::new("over 45 min").color(palette.danger));
        } else {
            ui.label(length);
        }
        // To the second is enough here, since cuts land on keyframes anyway.
        // The exact times are one hover away.
        ui.label(theme::muted(
            ui,
            format!(
                "{} to {}",
                format_clock(layout.span.start),
                format_clock(layout.span.end)
            ),
        ))
        .on_hover_text(format!(
            "{} to {}",
            format_timestamp(layout.span.start),
            format_timestamp(layout.span.end)
        ));
        if layout.is_merged() {
            let note = egui::RichText::new(format!("{} parts merged", layout.segments.len()))
                .color(palette.primary);
            theme::pill(ui, note, palette.primary_tint);
        }
        match fate {
            CardFate::Written => {}
            CardFate::WrittenAsFile { number, of } => {
                ui.label(theme::muted(ui, format!("File {number} of {of}")))
                    .on_hover_text(
                        "Dropped scenes are not written, so the files are numbered without them.",
                    );
            }
            CardFate::Dropped => {
                let badge = theme::semibold(DROPPED_BADGE, 13.0).color(palette.on_fill);
                theme::pill(ui, badge, palette.dropped);
            }
        }
        let (label, hint, wanted) = if fate.is_dropped() {
            (KEEP_SCENE_LABEL, KEEP_SCENE_HINT, SceneFate::Keep)
        } else {
            (DROP_SCENE_LABEL, DROP_SCENE_HINT, SceneFate::Drop)
        };
        if ui.button(label).on_hover_text(hint).clicked() {
            fate_wanted = Some(wanted);
        }
    });
    fate_wanted
}

/// Shown beside each part of a merged scene. Returns true when clicked.
fn show_move_out_button(ui: &mut egui::Ui) -> bool {
    ui.button("Move out to its own scene")
        .on_hover_text(
            "Takes this part out and makes it a scene of its own. \
             The parts before and after it stay as they are.",
        )
        .clicked()
}

fn show_placeholder(ui: &mut egui::Ui, size: egui::Vec2, text: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (area, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter()
        .rect_filled(area, PREVIEW_ROUNDING, palette.page);
    ui.painter().text(
        area.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::default(),
        palette.muted_text,
    );
    response
}

pub fn file_name(video: &Path) -> String {
    video
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| video.to_path_buf())
        .display()
        .to_string()
}

/// These tests drive the real review screen with simulated mouse and key
/// input, the way a person would use it, and apply what the screen asks for
/// to the session, the way the app does. The video does not exist, so
/// previews fail to load and show placeholders, which does not matter for
/// merging. What the session does with a request is tested in
/// `review_session`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_session::{test_sessions, FollowUp};

    const SCREEN: egui::Vec2 = egui::vec2(1200.0, 3000.0);
    const SECONDS_PER_FRAME: f64 = 0.1;
    const DRAG_STEPS: usize = 8;

    struct Screen {
        ctx: egui::Context,
        session: ReviewSession,
        view: ReviewScreen,
        settings: Settings,
        /// Whether the keys are this screen's or those of a dialog over it.
        keyboard: KeyboardOwner,
        /// The size of the window the screen is drawn in.
        window: egui::Vec2,
        time: f64,
        last_follow_up: Option<FollowUp>,
        /// What the last frame painted.
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl Screen {
        /// A 100 second video cut into five scenes of 20 seconds each.
        fn with_five_scenes() -> Self {
            Self::showing(test_sessions::five_scenes(), SCREEN)
        }

        fn showing(session: ReviewSession, window: egui::Vec2) -> Self {
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut screen = Self {
                ctx,
                session,
                view: ReviewScreen::default(),
                settings: Settings::default(),
                keyboard: KeyboardOwner::Screens,
                window,
                time: 0.0,
                last_follow_up: None,
                shapes: Vec::new(),
            };
            // Cards take input from their second frame on.
            screen.frame(Vec::new(), egui::Modifiers::NONE);
            screen.frame(Vec::new(), egui::Modifiers::NONE);
            screen
        }

        fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.window)),
                time: Some(self.time),
                modifiers,
                events,
                ..Default::default()
            };
            self.time += SECONDS_PER_FRAME;
            let Self {
                ctx,
                session,
                view,
                settings,
                keyboard,
                last_follow_up,
                shapes,
                ..
            } = self;
            let mut requests = Vec::new();
            let output = ctx.run(input, |ctx| {
                requests = view.show(ctx, session, settings, None, *keyboard);
            });
            *shapes = output.shapes;
            // No scene files of an earlier split are in the way.
            let follow_up = session.apply_frame(requests, &test_sessions::nothing_in_the_way);
            // Settings are the app's to keep, and this stands in for it.
            let video = Arc::clone(session.video());
            match follow_up {
                Some(FollowUp::ChangeSettings(change)) => *settings = settings.changed_by(change),
                Some(FollowUp::ChangeDetection(detection)) => {
                    settings.change_detection_for(&video, detection);
                }
                Some(FollowUp::RememberForFolder) => {
                    settings.remember_detection_for_folder_of(&video);
                }
                Some(FollowUp::UseGeneralSettings) => {
                    settings.use_general_detection_for_folder_of(&video);
                }
                _ => {}
            }
            if follow_up.is_some() {
                *last_follow_up = follow_up;
            }
        }

        /// A spot on a scene's card with nothing else on it: its heading row,
        /// to the right of the text.
        fn empty_part_of_scene(&self, scene: usize) -> egui::Pos2 {
            let card = self.view.scene_cards[scene];
            egui::pos2(card.right() - 40.0, card.top() + CARD_MARGIN.top + 8.0)
        }

        /// The middle of the first preview of a scene that is not merged.
        fn first_preview_of_scene(&self, scene: usize) -> egui::Pos2 {
            let card = self.view.scene_cards[scene];
            // Previews are at least this big, so the spot is inside the
            // first one whatever size the card gives them.
            egui::pos2(
                card.left() + CARD_MARGIN.left + MIN_PREVIEW_WIDTH / 2.0,
                card.bottom() - CARD_MARGIN.bottom - MIN_PREVIEW_WIDTH * PREVIEW_ASPECT / 2.0,
            )
        }

        fn button(pos: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            }
        }

        fn click_at(&mut self, pos: egui::Pos2, modifiers: egui::Modifiers) {
            self.frame(vec![egui::Event::PointerMoved(pos)], modifiers);
            self.frame(vec![Self::button(pos, true, modifiers)], modifiers);
            self.frame(vec![Self::button(pos, false, modifiers)], modifiers);
        }

        fn click_scene(&mut self, scene: usize, modifiers: egui::Modifiers) {
            self.click_at(self.empty_part_of_scene(scene), modifiers);
        }

        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            let none = egui::Modifiers::NONE;
            self.frame(vec![egui::Event::PointerMoved(from)], none);
            self.frame(vec![Self::button(from, true, none)], none);
            for step in 1..=DRAG_STEPS {
                let pos = from + (to - from) * (step as f32 / DRAG_STEPS as f32);
                self.frame(vec![egui::Event::PointerMoved(pos)], none);
            }
            self.frame(vec![Self::button(to, false, none)], none);
            self.frame(Vec::new(), none);
        }

        fn drag_scene_onto(&mut self, source: usize, target: usize) {
            self.drag(
                self.empty_part_of_scene(source),
                self.empty_part_of_scene(target),
            );
        }

        fn press(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
            let event = egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            self.frame(vec![event], modifiers);
            self.frame(Vec::new(), egui::Modifiers::NONE);
        }

        fn press_ctrl_z(&mut self) {
            self.press(egui::Key::Z, egui::Modifiers::COMMAND);
        }

        fn press_plain(&mut self, key: egui::Key) {
            self.press(key, egui::Modifiers::NONE);
        }

        /// The position of the scene the keys act on, counted from zero.
        fn focused_scene(&self) -> Option<usize> {
            self.session.focus().map(|focus| focus.scene().position())
        }

        /// How many focus frames the last frame painted.
        fn focus_rings(&self) -> usize {
            let ring_color = theme::palette(&self.ctx).focus_ring;
            self.shapes
                .iter()
                .map(|clipped| focus_rings_in(&clipped.shape, ring_color))
                .sum()
        }

        /// How far to the right the last frame wrote anything.
        fn right_edge_of_all_text(&self) -> f32 {
            self.shapes
                .iter()
                .map(|clipped| right_edge_of_text_in(&clipped.shape))
                .fold(f32::NEG_INFINITY, f32::max)
        }

        /// The middle of the button that says `label` on the row between a
        /// scene's card and the next card.
        fn button_between_scene_and_next(&self, scene: usize, label: &str) -> egui::Pos2 {
            let above = self.view.scene_cards[scene].bottom();
            let below = self.view.scene_cards[scene + 1].top();
            let mut places = Vec::new();
            for clipped in &self.shapes {
                places_of_text_in(&clipped.shape, label, &mut places);
            }
            places
                .into_iter()
                .map(|place| place.center())
                .find(|middle| above < middle.y && middle.y < below)
                .unwrap_or_else(|| panic!("no \"{label}\" between scenes {scene} and the next"))
        }

        fn split_feedback(&self) -> String {
            self.session
                .new_split_feedback()
                .map(|feedback| feedback.in_words())
                .unwrap_or_default()
        }

        fn scene_ends(&self) -> Vec<f64> {
            self.session
                .scenes()
                .iter()
                .map(|scene| scene.end)
                .collect()
        }

        /// The positions of the selected scenes, counted from zero.
        fn selected_scenes(&self) -> Vec<usize> {
            self.session
                .selection()
                .scenes()
                .map(SceneIndex::position)
                .collect()
        }
    }

    fn focus_rings_in(shape: &egui::Shape, ring_color: egui::Color32) -> usize {
        match shape {
            egui::Shape::Vec(shapes) => shapes
                .iter()
                .map(|shape| focus_rings_in(shape, ring_color))
                .sum(),
            egui::Shape::Rect(rect) => {
                let is_focus_ring =
                    rect.stroke.width == FOCUS_RING_WIDTH && rect.stroke.color == ring_color;
                usize::from(is_focus_ring)
            }
            _ => 0,
        }
    }

    fn places_of_text_in(shape: &egui::Shape, wanted: &str, places: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    places_of_text_in(shape, wanted, places);
                }
            }
            egui::Shape::Text(text) if text.galley.text() == wanted => {
                places.push(text.visual_bounding_rect());
            }
            _ => {}
        }
    }

    fn right_edge_of_text_in(shape: &egui::Shape) -> f32 {
        match shape {
            egui::Shape::Vec(shapes) => shapes
                .iter()
                .map(right_edge_of_text_in)
                .fold(f32::NEG_INFINITY, f32::max),
            egui::Shape::Text(text) => text.visual_bounding_rect().right(),
            _ => f32::NEG_INFINITY,
        }
    }

    #[test]
    fn the_list_of_keys_and_the_word_on_a_moved_split_fit_the_smallest_window() {
        // As wide as what is left of a window 900 points wide beside the
        // queue.
        const NARROW_WINDOW: egui::Vec2 = egui::vec2(600.0, 3000.0);
        let mut screen = Screen::showing(test_sessions::five_scenes(), NARROW_WINDOW);
        assert!(screen.right_edge_of_all_text() <= NARROW_WINDOW.x);

        screen.press_plain(egui::Key::F1);
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::ArrowDown);
        screen.press(egui::Key::ArrowRight, egui::Modifiers::ALT);
        screen.press(egui::Key::ArrowRight, egui::Modifiers::ALT);
        assert!(screen.split_feedback().contains("is in the way"));
        assert_eq!(screen.view.keys_list, KeysList::Shown);
        assert!(screen.right_edge_of_all_text() <= NARROW_WINDOW.x);
    }

    #[test]
    fn the_arrow_keys_move_a_framed_focus_through_the_scenes() {
        let mut screen = Screen::with_five_scenes();
        assert_eq!(screen.focus_rings(), 0);

        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::ArrowDown);
        assert_eq!(screen.focused_scene(), Some(1));
        assert_eq!(screen.selected_scenes(), vec![1]);
        assert_eq!(screen.focus_rings(), 1);

        screen.press_plain(egui::Key::End);
        assert_eq!(screen.focused_scene(), Some(4));
        screen.press_plain(egui::Key::Home);
        screen.press_plain(egui::Key::ArrowUp);
        assert_eq!(screen.focused_scene(), Some(0));
        assert_eq!(screen.focus_rings(), 1);
        // The arrow keys moved through the scenes, and gave no button the
        // keys on the way.
        assert!(!keyboard::a_control_has_the_keys(&screen.ctx));
    }

    #[test]
    fn a_click_moves_the_focus_and_the_keys_go_on_from_there() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(2, egui::Modifiers::NONE);
        assert_eq!(screen.focused_scene(), Some(2));

        screen.press_plain(egui::Key::ArrowDown);
        assert_eq!(screen.focused_scene(), Some(3));
    }

    #[test]
    fn shift_with_the_arrow_keys_selects_a_run_of_scenes_for_m_to_merge() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.press(egui::Key::ArrowDown, egui::Modifiers::SHIFT);
        screen.press(egui::Key::ArrowDown, egui::Modifiers::SHIFT);
        assert_eq!(screen.selected_scenes(), vec![0, 1, 2]);

        screen.press_plain(egui::Key::M);
        assert_eq!(screen.scene_ends(), vec![60.0, 80.0, 100.0]);
        assert_eq!(screen.focused_scene(), Some(0));
    }

    #[test]
    fn a_scene_focused_with_the_keys_is_scrolled_into_view() {
        let mut screen = screen_with_many_scenes(SMALL_WINDOW);
        let last = MANY_SCENES - 1;
        assert!(!card_is_laid_out(&screen, last));

        screen.press_plain(egui::Key::End);
        // The list scrolls over the next frames.
        for _ in 0..40 {
            screen.frame(Vec::new(), egui::Modifiers::NONE);
        }
        assert_eq!(screen.focused_scene(), Some(last));
        assert!(card_is_laid_out(&screen, last));
        let card = screen.view.scene_cards[last];
        assert!(screen.view.list_area.contains_rect(card));
        assert_eq!(screen.focus_rings(), 1);
    }

    #[test]
    fn enter_and_space_play_the_focused_scene_from_its_start() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::Enter);
        assert_eq!(screen.last_follow_up, Some(FollowUp::PlayFrom(20.0)));

        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::Space);
        assert_eq!(screen.last_follow_up, Some(FollowUp::PlayFrom(40.0)));
    }

    #[test]
    fn left_and_right_choose_a_framed_picture_to_play_from() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::ArrowRight);
        screen.press_plain(egui::Key::ArrowRight);
        // The card and the chosen picture each have a frame.
        assert_eq!(screen.focus_rings(), 2);

        screen.press_plain(egui::Key::Enter);
        // The second of four pictures of a scene from 0 to 20 seconds.
        assert_eq!(screen.last_follow_up, Some(FollowUp::PlayFrom(7.5)));

        screen.press_plain(egui::Key::ArrowLeft);
        screen.press_plain(egui::Key::ArrowLeft);
        assert_eq!(screen.focus_rings(), 1);
    }

    #[test]
    fn a_picture_chosen_in_a_part_that_is_not_shown_shows_every_part() {
        let mut screen = screen_with_one_scene_of_ten_parts();
        let layout = screen.session.plan().scene_layouts()[0].clone();
        screen.press_plain(egui::Key::ArrowDown);
        // The first two parts are shown, with four pictures each.
        for _ in 0..8 {
            screen.press_plain(egui::Key::ArrowRight);
        }
        assert_eq!(screen.view.parts_view(&layout), PartsView::Collapsed);

        screen.press_plain(egui::Key::ArrowRight);
        assert_eq!(screen.view.parts_view(&layout), PartsView::Expanded);
        // The frame after the one that asked for every part draws them.
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        assert_eq!(screen.focus_rings(), 2);
    }

    #[test]
    fn j_joins_the_focused_scene_with_the_next_and_u_takes_it_apart() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::J);
        assert_eq!(screen.scene_ends(), vec![20.0, 60.0, 80.0, 100.0]);
        assert_eq!(screen.focused_scene(), Some(1));

        screen.press_plain(egui::Key::U);
        assert_eq!(screen.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
        assert_eq!(screen.focused_scene(), Some(1));
    }

    #[test]
    fn alt_with_left_or_right_moves_the_split_the_focused_scene_starts_at() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::ArrowDown);

        screen.press(egui::Key::ArrowRight, egui::Modifiers::ALT);
        assert_eq!(screen.scene_ends(), vec![30.0, 40.0, 60.0, 80.0, 100.0]);
        assert_eq!(
            screen.split_feedback(),
            "Moved the split from 00:00:20.000 to 00:00:30.000."
        );
        assert_eq!(screen.focused_scene(), Some(1));
        // No picture was chosen by the same press.
        assert_eq!(screen.focus_rings(), 1);

        // The next point where the video can be cut is the split at 40.
        screen.press(egui::Key::ArrowRight, egui::Modifiers::ALT);
        assert_eq!(screen.scene_ends(), vec![30.0, 40.0, 60.0, 80.0, 100.0]);
        assert_eq!(
            screen.split_feedback(),
            "The split cannot move further. The split at 00:00:40.000 is in the way."
        );

        screen.press(egui::Key::ArrowLeft, egui::Modifiers::ALT);
        screen.press(egui::Key::ArrowLeft, egui::Modifiers::ALT);
        assert_eq!(screen.scene_ends(), vec![10.0, 40.0, 60.0, 80.0, 100.0]);

        screen.press_ctrl_z();
        assert_eq!(screen.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn the_buttons_on_the_row_between_two_cards_move_their_split() {
        let mut screen = Screen::with_five_scenes();
        let earlier = screen.button_between_scene_and_next(1, MOVE_SPLIT_EARLIER_LABEL);
        screen.click_at(earlier, egui::Modifiers::NONE);
        assert_eq!(screen.scene_ends(), vec![20.0, 30.0, 60.0, 80.0, 100.0]);
        assert_eq!(
            screen.split_feedback(),
            "Moved the split from 00:00:40.000 to 00:00:30.000."
        );

        let later = screen.button_between_scene_and_next(1, MOVE_SPLIT_LATER_LABEL);
        screen.click_at(later, egui::Modifiers::NONE);
        assert_eq!(screen.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
        // The keys now work on the scene that starts at the moved split.
        assert_eq!(screen.focused_scene(), Some(2));

        screen.press_ctrl_z();
        screen.press_ctrl_z();
        assert!(!screen.session.is_edited());
    }

    #[test]
    fn the_row_between_two_cards_fits_the_smallest_window() {
        // As wide as what is left of a window 900 points wide beside the
        // queue, and tall enough to have the first cards in view.
        const NARROW_WINDOW: egui::Vec2 = egui::vec2(600.0, 3000.0);
        let mut screen = Screen::showing(test_sessions::five_scenes(), NARROW_WINDOW);
        let card = screen.view.scene_cards[0];
        assert!(card.right() <= NARROW_WINDOW.x);
        assert!(screen.right_edge_of_all_text() <= NARROW_WINDOW.x);

        // All three buttons are there and each does its own thing.
        let merge = screen.button_between_scene_and_next(0, "Merge scenes 1 and 2");
        screen.click_at(merge, egui::Modifiers::NONE);
        assert_eq!(screen.scene_ends(), vec![40.0, 60.0, 80.0, 100.0]);
        screen.press_ctrl_z();

        let later = screen.button_between_scene_and_next(0, MOVE_SPLIT_LATER_LABEL);
        screen.click_at(later, egui::Modifiers::NONE);
        assert_eq!(screen.scene_ends(), vec![30.0, 40.0, 60.0, 80.0, 100.0]);
        let earlier = screen.button_between_scene_and_next(0, MOVE_SPLIT_EARLIER_LABEL);
        screen.click_at(earlier, egui::Modifiers::NONE);
        assert_eq!(screen.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn ctrl_enter_asks_for_the_split_like_the_button_does() {
        let mut screen = Screen::with_five_scenes();
        screen.press(egui::Key::Enter, egui::Modifiers::COMMAND);
        assert_eq!(screen.last_follow_up, Some(FollowUp::Export));
    }

    #[test]
    fn ctrl_enter_does_nothing_while_there_is_only_one_scene() {
        let mut screen = Screen::showing(test_sessions::scenes_of_twenty_seconds(1), SCREEN);
        screen.press(egui::Key::Enter, egui::Modifiers::COMMAND);
        assert_eq!(screen.last_follow_up, None);
        screen.press(egui::Key::Enter, CTRL_SHIFT);
        assert_eq!(screen.last_follow_up, None);
    }

    const CTRL_SHIFT: egui::Modifiers = egui::Modifiers {
        shift: true,
        ..egui::Modifiers::COMMAND
    };

    #[test]
    fn ctrl_shift_enter_asks_to_split_later_like_the_button_does() {
        let mut screen = Screen::with_five_scenes();
        screen.press(egui::Key::Enter, CTRL_SHIFT);
        assert_eq!(screen.last_follow_up, Some(FollowUp::MarkReady));
    }

    #[test]
    fn x_drops_the_scene_the_keys_are_on_and_x_again_keeps_it() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::ArrowDown);
        // Escape clears the selection and leaves the focus.
        screen.press_plain(egui::Key::Escape);
        assert_eq!(screen.session.selection().scenes().count(), 0);

        screen.press_plain(egui::Key::X);
        assert_eq!(screen.session.dropped_count(), 1);
        screen.press_plain(egui::Key::X);
        assert_eq!(screen.session.dropped_count(), 0);
    }

    #[test]
    fn x_with_alt_or_ctrl_held_drops_nothing() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.press(egui::Key::X, egui::Modifiers::ALT);
        screen.press(egui::Key::X, egui::Modifiers::COMMAND);
        assert_eq!(screen.session.dropped_count(), 0);
    }

    #[test]
    fn m_with_alt_or_ctrl_held_merges_nothing() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(0, egui::Modifiers::NONE);
        screen.click_scene(2, egui::Modifiers::SHIFT);
        screen.press(egui::Key::M, egui::Modifiers::ALT);
        screen.press(egui::Key::M, egui::Modifiers::COMMAND);
        assert_eq!(screen.session.scene_count(), 5);

        screen.press_plain(egui::Key::M);
        assert_eq!(screen.session.scene_count(), 3);
    }

    #[test]
    fn the_keys_list_names_the_frame_by_its_shape_and_every_key_of_the_window() {
        // The frame is dark in one palette and light in the other.
        for text in [KEYS_INTRODUCTION, DROP_SCENE_HINT, KEEP_SCENE_HINT] {
            assert!(text.contains("thick frame"), "{text}");
            assert!(!text.contains("dark frame"), "{text}");
        }
        let action_of = |wanted: &str| {
            KEYS.iter()
                .find(|(key, _)| *key == wanted)
                .map(|(_, action)| *action)
        };
        assert_eq!(action_of("Ctrl+O"), Some("Add videos."));
        assert_eq!(
            action_of("X"),
            Some("Drop the scene, so no file is written for it. Press X again to keep it.")
        );
        assert_eq!(
            KEYS_ON_THIS_SYSTEM.is_some(),
            cfg!(target_os = "macos"),
            "only a Mac has another name for Ctrl"
        );
    }

    #[test]
    fn putting_scenes_together_is_called_merging_everywhere() {
        let merge_texts = [
            TIPS[0].0,
            TIPS[0].1,
            KEYS[4].1,
            KEYS[5].1,
            MERGE_SELECTED_HINT,
            LEAVE_OUT_CHAPTERS_EXPLANATION,
        ];
        for text in merge_texts {
            assert!(text.contains("Merge") || text.contains("merge"), "{text}");
            assert!(!text.contains("Join") && !text.contains("join"), "{text}");
        }
        assert_eq!(
            drag_hint(SceneIndex::new(2)),
            "Drag scene 3 onto another scene to merge them"
        );
    }

    #[test]
    fn the_warning_about_earlier_scene_files_names_them() {
        let two = EarlierSceneFiles {
            scene_dir: PathBuf::from("videos"),
            file_names: vec!["Rocky 001.mp4".to_owned(), "Rocky 002.mp4".to_owned()],
        };
        assert_eq!(
            earlier_files_warning(&two),
            "2 scene files from an earlier split would be replaced: \
             Rocky 001.mp4, Rocky 002.mp4."
        );
        let one = EarlierSceneFiles {
            scene_dir: PathBuf::from("videos"),
            file_names: vec!["Rocky 2.mp4".to_owned()],
        };
        assert_eq!(
            earlier_files_warning(&one),
            "1 scene file from an earlier split would be replaced: Rocky 2.mp4."
        );

        let many = EarlierSceneFiles {
            scene_dir: PathBuf::from("videos"),
            file_names: EarlierSceneFiles::numbered_names(12),
        };
        let warning = earlier_files_warning(&many);
        assert!(warning.ends_with("no_such_video_scene_003.mp4 and 9 more."));
        // The hover names every one of them, and the folder.
        let hover = earlier_files_hover(&many);
        assert!(hover.starts_with("In videos:"), "{hover}");
        assert!(hover.contains("no_such_video_scene_012.mp4"), "{hover}");
        assert!(hover.contains(TRASH_NAME), "{hover}");
    }

    #[test]
    fn the_warning_about_splits_on_exact_pictures_says_where_and_what_to_do() {
        assert_eq!(
            splits_off_cut_points_warning(&[47.36]),
            "1 split is on an exact picture, where the video can only be cut with \
             \"Cut on the exact picture (slower)\" on: at 00:00:47.360. \
             Move it to the nearest point where the video can be cut, \
             or turn that setting back on."
        );
        let several = splits_off_cut_points_warning(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(
            several.starts_with("5 splits are on exact pictures"),
            "{several}"
        );
        assert!(several.contains("00:00:03.000 and 2 more"), "{several}");
        assert!(
            several.contains("Move them to the nearest points"),
            "{several}"
        );
        assert_eq!(move_splits_label(1), "Move it to the nearest point");
        assert_eq!(move_splits_label(2), "Move them to the nearest points");

        let stuck = splits_stuck_warning(&[41.0]);
        assert!(stuck.starts_with("1 split cannot move"), "{stuck}");
        assert!(stuck.contains("00:00:41.000"), "{stuck}");
    }

    #[test]
    fn the_button_that_moves_exact_splits_asks_for_just_that() {
        let mut screen = Screen::with_five_scenes();
        screen
            .session
            .place_new_splits(crate::exact_cut::CutPrecision::ExactFrame);
        screen.session.split_near(47.36);
        screen
            .session
            .place_new_splits(crate::exact_cut::CutPrecision::NearestKeyframe);
        screen.session.apply(
            ReviewRequest::Split,
            &crate::review_session::test_sessions::nothing_in_the_way,
        );
        assert!(matches!(
            screen.session.export_obstacle(),
            Some(ExportObstacle::SplitsOffCutPoints(_))
        ));

        screen.session.apply(
            ReviewRequest::MoveSplitsToCutPoints,
            &crate::review_session::test_sessions::nothing_in_the_way,
        );
        assert_eq!(screen.session.export_obstacle(), None);
        assert_eq!(
            screen.scene_ends(),
            vec![20.0, 40.0, 50.0, 60.0, 80.0, 100.0]
        );
    }

    #[test]
    fn remembering_for_a_folder_says_what_is_kept_and_for_which_folder() {
        let named = remember_for_folder_hint(Some(Path::new("videos")));
        assert!(named.contains("for the videos in videos,"), "{named}");
        assert!(named.contains("black screen settings"), "{named}");
        assert!(named.contains("settings for missed scenes"), "{named}");
        // A video whose folder is not known still gets a whole sentence.
        let unnamed = remember_for_folder_hint(None);
        assert!(
            unnamed.contains("for the videos in this video's folder,"),
            "{unnamed}"
        );
    }

    #[test]
    fn the_buttons_that_move_a_split_say_how_far_it_moves() {
        let one_picture = NudgeStep::OnePicture { seconds: 0.04 };
        for direction in [NudgeDirection::Earlier, NudgeDirection::Later] {
            let to_cut_point = move_split_hint(direction, NudgeStep::ToNextCutPoint);
            let by_a_picture = move_split_hint(direction, one_picture);
            assert!(to_cut_point.contains("where the video can be cut"));
            assert!(by_a_picture.contains("one picture"));
        }
        assert!(move_split_hint(NudgeDirection::Earlier, one_picture).contains("Alt+Left"));
        assert!(move_split_hint(NudgeDirection::Later, one_picture).contains("Alt+Right"));
    }

    #[test]
    fn no_text_of_the_screen_lost_the_break_between_its_lines() {
        // A line continuation that goes missing leaves the indent of the
        // next line inside the sentence.
        let texts = [
            KEYS_INTRODUCTION,
            JOIN_HINT,
            MOVE_SPLIT_EARLIER_HINT,
            MOVE_SPLIT_LATER_HINT,
            MOVE_SPLIT_ONE_PICTURE_EARLIER_HINT,
            MOVE_SPLIT_ONE_PICTURE_LATER_HINT,
            SPLIT_LATER_EXPLANATION,
            DROP_SCENE_HINT,
            KEEP_SCENE_HINT,
        ];
        for text in texts {
            assert!(!text.contains("  "), "{text}");
        }
    }

    #[test]
    fn behind_a_dialog_the_keys_neither_move_the_focus_nor_act_on_it() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.keyboard = KeyboardOwner::Dialog;

        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::End);
        assert_eq!(screen.focused_scene(), Some(0));
        screen.press_plain(egui::Key::J);
        screen.press(egui::Key::ArrowRight, egui::Modifiers::ALT);
        assert!(!screen.session.is_edited());
        screen.press_plain(egui::Key::Enter);
        screen.press_plain(egui::Key::Space);
        screen.press(egui::Key::Enter, egui::Modifiers::COMMAND);
        assert_eq!(screen.last_follow_up, None);
        screen.press_plain(egui::Key::F1);
        assert_eq!(screen.view.keys_list, KeysList::Hidden);
    }

    #[test]
    fn a_button_reached_with_tab_keeps_enter_space_and_the_arrow_keys() {
        let mut screen = Screen::with_five_scenes();
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::Tab);
        assert!(keyboard::a_control_has_the_keys(&screen.ctx));

        // The arrow keys now move on from that button, and Enter and Space
        // would press it.
        let focus_before = screen.session.focus();
        screen.press_plain(egui::Key::End);
        screen.press_plain(egui::Key::ArrowDown);
        screen.press_plain(egui::Key::ArrowRight);
        assert_eq!(screen.session.focus(), focus_before);

        // Escape gives the keys back to the scene list.
        screen.press_plain(egui::Key::Escape);
        assert!(!keyboard::a_control_has_the_keys(&screen.ctx));
        screen.press_plain(egui::Key::End);
        assert_eq!(screen.focused_scene(), Some(4));
    }

    #[test]
    fn f1_shows_the_list_of_keys_and_hides_it_again() {
        let mut screen = Screen::with_five_scenes();
        let first_card_top = screen.view.scene_cards[0].top();

        screen.press_plain(egui::Key::F1);
        assert_eq!(screen.view.keys_list, KeysList::Shown);
        // The list is drawn above the scenes, which move down for it.
        assert!(screen.view.scene_cards[0].top() > first_card_top);

        screen.press_plain(egui::Key::F1);
        assert_eq!(screen.view.keys_list, KeysList::Hidden);
        assert_eq!(screen.view.scene_cards[0].top(), first_card_top);
    }

    #[test]
    fn every_key_in_the_list_is_named_and_explained() {
        for (key, action) in KEYS {
            assert!(!key.is_empty());
            assert!(action.ends_with('.'), "{action}");
        }
        let names: HashSet<&str> = KEYS.iter().map(|(key, _)| *key).collect();
        assert_eq!(names.len(), KEYS.len());
    }

    #[test]
    fn dragging_a_scene_onto_a_later_one_merges_everything_between() {
        let mut screen = Screen::with_five_scenes();
        screen.drag_scene_onto(1, 3);
        assert_eq!(screen.scene_ends(), vec![20.0, 80.0, 100.0]);
    }

    #[test]
    fn dragging_upwards_merges_too() {
        let mut screen = Screen::with_five_scenes();
        screen.drag_scene_onto(4, 3);
        assert_eq!(screen.scene_ends(), vec![20.0, 40.0, 60.0, 100.0]);
    }

    #[test]
    fn dragging_twice_takes_five_scenes_down_to_two() {
        let mut screen = Screen::with_five_scenes();
        screen.drag_scene_onto(0, 2);
        // The scenes are now 0-60, 60-80 and 80-100.
        screen.drag_scene_onto(1, 2);
        assert_eq!(screen.scene_ends(), vec![60.0, 100.0]);
        assert_eq!(screen.session.scene_count(), 2);
    }

    #[test]
    fn dropping_a_scene_on_itself_changes_nothing() {
        let mut screen = Screen::with_five_scenes();
        screen.drag_scene_onto(2, 2);
        assert_eq!(screen.session.scene_count(), 5);
    }

    #[test]
    fn ctrl_z_takes_back_one_merge_at_a_time() {
        let mut screen = Screen::with_five_scenes();
        screen.drag_scene_onto(0, 2);
        screen.drag_scene_onto(1, 2);
        assert_eq!(screen.session.scene_count(), 2);

        screen.press_ctrl_z();
        assert_eq!(screen.scene_ends(), vec![60.0, 80.0, 100.0]);

        screen.press_ctrl_z();
        assert_eq!(screen.session.scene_count(), 5);

        // With nothing left to undo, another press is harmless.
        screen.press_ctrl_z();
        assert_eq!(screen.session.scene_count(), 5);
    }

    #[test]
    fn a_click_selects_a_scene_and_shift_click_extends_the_range() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(1, egui::Modifiers::NONE);
        assert_eq!(screen.selected_scenes(), vec![1]);

        screen.click_scene(3, egui::Modifiers::SHIFT);
        assert_eq!(screen.selected_scenes(), vec![1, 2, 3]);
    }

    #[test]
    fn ctrl_click_adds_a_scene_to_the_selection() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(0, egui::Modifiers::NONE);
        screen.click_scene(4, egui::Modifiers::COMMAND);
        assert_eq!(screen.selected_scenes(), vec![0, 4]);
    }

    #[test]
    fn m_merges_the_selected_scenes() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(1, egui::Modifiers::NONE);
        screen.click_scene(2, egui::Modifiers::SHIFT);

        screen.press(egui::Key::M, egui::Modifiers::NONE);
        assert_eq!(screen.scene_ends(), vec![20.0, 60.0, 80.0, 100.0]);
        assert_eq!(screen.selected_scenes(), vec![1]);
    }

    const SMALLEST_WINDOW_MAIN_PANEL: egui::Vec2 = egui::vec2(630.0, 600.0);

    fn dropped_scenes(screen: &Screen) -> Vec<usize> {
        screen
            .session
            .scene_fates()
            .iter()
            .enumerate()
            .filter(|(_, &fate)| fate == SceneFate::Drop)
            .map(|(position, _)| position)
            .collect()
    }

    #[test]
    fn x_drops_the_selected_scenes_and_x_again_keeps_them() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(1, egui::Modifiers::NONE);
        screen.click_scene(2, egui::Modifiers::SHIFT);

        screen.press(egui::Key::X, egui::Modifiers::NONE);
        assert_eq!(dropped_scenes(&screen), vec![1, 2]);
        // The scenes are still there and still selected, only not written.
        assert_eq!(screen.session.scene_count(), 5);
        assert_eq!(screen.selected_scenes(), vec![1, 2]);

        screen.press(egui::Key::X, egui::Modifiers::NONE);
        assert_eq!(dropped_scenes(&screen), Vec::<usize>::new());
    }

    #[test]
    fn ctrl_z_brings_a_dropped_scene_back() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(3, egui::Modifiers::NONE);
        screen.press(egui::Key::X, egui::Modifiers::NONE);
        assert_eq!(dropped_scenes(&screen), vec![3]);

        screen.press_ctrl_z();
        assert_eq!(dropped_scenes(&screen), Vec::<usize>::new());
    }

    #[test]
    fn ctrl_x_is_not_taken_for_the_drop_key() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(1, egui::Modifiers::NONE);
        screen.press(egui::Key::X, egui::Modifiers::COMMAND);
        assert_eq!(dropped_scenes(&screen), Vec::<usize>::new());
    }

    #[test]
    fn x_with_every_scene_selected_drops_none_and_says_why() {
        let mut screen = Screen::with_five_scenes();
        screen.press(egui::Key::A, egui::Modifiers::COMMAND);
        screen.press(egui::Key::X, egui::Modifiers::NONE);

        assert_eq!(dropped_scenes(&screen), Vec::<usize>::new());
        assert!(screen.session.drop_refusal().is_some());
    }

    #[test]
    fn the_button_on_a_card_asks_to_drop_or_keep_that_scene() {
        let mut screen = Screen::with_five_scenes();
        let drop_the_third = SceneListInput {
            fate_wanted: Some((SceneIndex::new(2), SceneFate::Drop)),
            ..Default::default()
        };
        assert_eq!(
            screen
                .view
                .edits_from_scene_list(&screen.ctx, &drop_the_third),
            vec![ReviewRequest::DropScene(SceneIndex::new(2))]
        );
        let keep_the_third = SceneListInput {
            fate_wanted: Some((SceneIndex::new(2), SceneFate::Keep)),
            ..Default::default()
        };
        assert_eq!(
            screen
                .view
                .edits_from_scene_list(&screen.ctx, &keep_the_third),
            vec![ReviewRequest::KeepScene(SceneIndex::new(2))]
        );
    }

    #[test]
    fn a_review_with_dropped_scenes_is_drawn_in_the_smallest_window_and_still_works() {
        let mut screen = Screen::showing(test_sessions::five_scenes(), SMALLEST_WINDOW_MAIN_PANEL);
        screen.session.apply(
            ReviewRequest::DropScene(SceneIndex::new(1)),
            &test_sessions::nothing_in_the_way,
        );
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        screen.frame(Vec::new(), egui::Modifiers::NONE);

        // Drawing changes nothing, and the cards are all there.
        assert_eq!(dropped_scenes(&screen), vec![1]);
        assert_eq!(screen.view.scene_cards.len(), 5);
        screen.press_ctrl_z();
        assert_eq!(dropped_scenes(&screen), Vec::<usize>::new());
    }

    /// How often the last frame painted exactly `text`.
    fn times_painted(screen: &Screen, text: &str) -> usize {
        let mut places = Vec::new();
        for clipped in &screen.shapes {
            places_of_text_in(&clipped.shape, text, &mut places);
        }
        places.len()
    }

    #[test]
    fn a_video_that_cannot_be_cut_exactly_says_so_above_the_split_in_the_smallest_window() {
        let why_not_exact = crate::exact_fallback::WhyNotExact::EncoderMissing;
        let notice = why_not_exact.before_the_split_in_plain_words();
        let mut screen = Screen::showing(test_sessions::five_scenes(), SMALLEST_WINDOW_MAIN_PANEL);
        screen
            .session
            .note_exact_cut_look(crate::exact_fallback::ExactCutLook::CannotBeCut(
                why_not_exact,
            ));

        // With exact cuts off nobody asked for one, so nothing is said.
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        assert_eq!(times_painted(&screen, &notice), 0);

        screen.settings.export.cut_precision = CutPrecision::ExactFrame;
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        assert_eq!(times_painted(&screen, &notice), 1);
        // It wraps inside the panel, and the scenes are all still there.
        assert!(
            screen.right_edge_of_all_text() <= SMALLEST_WINDOW_MAIN_PANEL.x,
            "{}",
            screen.right_edge_of_all_text()
        );
        assert_eq!(screen.view.scene_cards.len(), 5);
    }

    #[test]
    fn a_card_laid_out_as_dropped_is_another_shape_than_the_same_card_kept() {
        let screen = Screen::with_five_scenes();
        let layouts = screen.session.plan().scene_layouts();
        let card = |fate| SceneCard {
            scene: SceneIndex::new(0),
            layout: &layouts[0],
            fate,
        };
        assert_ne!(
            screen.view.card_shape(card(CardFate::Written), 600.0),
            screen.view.card_shape(card(CardFate::Dropped), 600.0)
        );
    }

    #[test]
    fn cards_say_nothing_about_files_while_every_scene_is_written() {
        assert_eq!(
            card_fates(&[SceneFate::Keep, SceneFate::Keep]),
            vec![CardFate::Written, CardFate::Written]
        );
    }

    #[test]
    fn once_a_scene_is_dropped_each_kept_card_names_its_file_without_gaps() {
        let fates = [
            SceneFate::Drop,
            SceneFate::Keep,
            SceneFate::Drop,
            SceneFate::Keep,
        ];
        assert_eq!(
            card_fates(&fates),
            vec![
                CardFate::Dropped,
                CardFate::WrittenAsFile { number: 1, of: 2 },
                CardFate::Dropped,
                CardFate::WrittenAsFile { number: 2, of: 2 },
            ]
        );
    }

    #[test]
    fn the_split_button_counts_the_files_and_the_scenes_left_out() {
        assert_eq!(split_button_label(4, 0), "Split into 4 files");
        assert_eq!(
            split_button_label(3, 1),
            "Split into 3 files, 1 scene dropped"
        );
        assert_eq!(
            split_button_label(1, 2),
            "Split into 1 file, 2 scenes dropped"
        );
    }

    #[test]
    fn the_hint_on_the_kept_original_says_how_to_get_the_choice_back() {
        let dropped = original_always_kept_hint(OriginalAlwaysKept::ScenesDropped);
        let exact = original_always_kept_hint(OriginalAlwaysKept::CutIsExact);
        let both = original_always_kept_hint(OriginalAlwaysKept::ScenesDroppedAndCutIsExact);
        assert!(dropped.contains("still compares every new file"));
        assert_eq!(
            exact,
            "Turn off \"Cut on the exact picture (slower)\" in Settings to choose what happens \
             to the original again."
        );
        // With both, turning the setting off does not bring the choice back.
        assert!(both.starts_with(&dropped), "{both}");
        assert!(both.contains("once no scene is dropped"), "{both}");
        assert!(both.contains(settings_controls::EXACT_CUT_LABEL), "{both}");
    }

    #[test]
    fn a_dropped_scene_says_so_when_pointed_at_on_the_timeline() {
        let screen = Screen::with_five_scenes();
        let layouts = screen.session.plan().scene_layouts();
        let second = SceneIndex::new(1);
        let kept = scene_description(second, &layouts[1], None, SceneFate::Keep);
        let dropped = scene_description(second, &layouts[1], None, SceneFate::Drop);
        assert!(!kept.contains("Dropped"));
        assert_eq!(dropped, format!("{kept}   Dropped, not written"));
    }

    #[test]
    fn a_scene_list_is_offered_a_name_made_from_the_video() {
        assert_eq!(
            scene_list_file_name(Path::new("videos/holiday.mp4"), SceneListFormat::Csv),
            "holiday scenes.csv"
        );
        assert_eq!(
            scene_list_file_name(Path::new("holiday.v2.mkv"), SceneListFormat::Edl),
            "holiday.v2 scenes.edl"
        );
    }

    #[test]
    fn a_review_that_saved_or_could_not_save_its_scene_list_is_drawn() {
        let mut screen = Screen::with_five_scenes();
        let notes = [
            SceneListNote::Saved(PathBuf::from("videos/holiday scenes.csv")),
            SceneListNote::NotSaved("access denied".to_owned()),
        ];
        for note in notes {
            screen.session.note_scene_list(note.clone());
            screen.frame(Vec::new(), egui::Modifiers::NONE);
            assert_eq!(screen.session.scene_list_note(), Some(&note));
        }
    }

    #[test]
    fn ctrl_a_selects_every_scene_and_escape_none() {
        let mut screen = Screen::with_five_scenes();
        screen.press(egui::Key::A, egui::Modifiers::COMMAND);
        assert_eq!(screen.selected_scenes(), vec![0, 1, 2, 3, 4]);

        screen.press(egui::Key::Escape, egui::Modifiers::NONE);
        assert!(screen.selected_scenes().is_empty());
    }

    #[test]
    fn behind_a_dialog_the_keys_do_nothing_to_the_review() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(1, egui::Modifiers::NONE);
        screen.click_scene(2, egui::Modifiers::SHIFT);
        screen.keyboard = KeyboardOwner::Dialog;

        screen.press(egui::Key::M, egui::Modifiers::NONE);
        assert_eq!(screen.session.scene_count(), 5);
        screen.press(egui::Key::Escape, egui::Modifiers::NONE);
        assert_eq!(screen.selected_scenes(), vec![1, 2]);
        screen.press(egui::Key::A, egui::Modifiers::COMMAND);
        assert_eq!(screen.selected_scenes(), vec![1, 2]);

        // With the dialog gone the same key works again.
        screen.keyboard = KeyboardOwner::Screens;
        screen.press(egui::Key::M, egui::Modifiers::NONE);
        assert_eq!(screen.session.scene_count(), 4);
    }

    #[test]
    fn an_escape_something_in_front_has_taken_does_not_clear_the_selection() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(1, egui::Modifiers::NONE);

        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            time: Some(screen.time),
            events: vec![escape],
            ..Default::default()
        };
        let mut requests = Vec::new();
        let _ = screen.ctx.run(input, |ctx| {
            // The player, which is in front, closes on this Escape.
            assert!(keyboard::take_escape(ctx));
            requests = screen.view.show(
                ctx,
                &screen.session,
                &screen.settings,
                None,
                KeyboardOwner::Screens,
            );
        });
        assert!(!requests.contains(&ReviewRequest::ClearSelection));
    }

    #[test]
    fn looking_harder_scans_for_subtle_changes_without_changing_what_is_saved() {
        let saved = HardCutSettings {
            sensitivity: Sensitivity::Low,
            min_scene_seconds: 300.0,
        };
        assert_eq!(
            look_harder_scan(saved),
            ScanKind::HardCuts(HardCutSettings {
                sensitivity: Sensitivity::High,
                min_scene_seconds: 300.0,
            })
        );
    }

    #[test]
    fn a_stretch_the_scan_could_not_read_is_told_in_plain_words() {
        let stretch = UndecodedFrames {
            from_seconds: 3723.4,
            missing_frames: 7,
            stretch_frames: 250,
        };
        assert_eq!(
            undecoded_warning(&stretch),
            "7 of the 250 pictures from 1:02:03 on could not be read. \
             A scene change there may have been missed."
        );
    }

    #[test]
    fn a_review_with_an_unread_stretch_is_drawn_and_still_works() {
        let scan = crate::cache::SourcedDetection {
            detection: test_sessions::scenes_of_twenty_seconds_detected(5),
            source: crate::cache::DetectionSource::FreshScan,
            undecoded: vec![UndecodedFrames {
                from_seconds: 42.0,
                missing_frames: 7,
                stretch_frames: 250,
            }],
        };
        let mut screen = Screen::showing(test_sessions::review_of(scan), SCREEN);
        screen.drag_scene_onto(0, 1);
        assert_eq!(screen.scene_ends(), vec![40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn a_preview_that_could_not_be_made_says_why_when_pointed_at() {
        assert_eq!(preview_hover_text(62.0, None), "Play from 1:02");
        assert_eq!(
            preview_hover_text(62.0, Some("ffmpeg gave no picture")),
            "Play from 1:02\nThe preview could not be made: ffmpeg gave no picture"
        );
    }

    #[test]
    fn the_screen_draws_the_settings_without_changing_them() {
        let mut screen = Screen::with_five_scenes();
        screen.view.missed_scenes_open = true;
        let before = screen.settings.clone();
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        screen.click_scene(1, egui::Modifiers::NONE);
        assert_eq!(screen.settings, before);
    }

    #[test]
    fn clicking_a_preview_plays_the_video_from_that_moment() {
        let mut screen = Screen::with_five_scenes();
        let preview = screen.first_preview_of_scene(2);
        screen.click_at(preview, egui::Modifiers::NONE);

        // Scene 2 runs from 40 to 60 seconds, and its first preview is taken
        // 2.5 seconds in.
        assert_eq!(screen.last_follow_up, Some(FollowUp::PlayFrom(42.5)));
        assert_eq!(screen.selected_scenes(), vec![2]);
        assert_eq!(screen.session.scene_count(), 5);
    }

    #[test]
    fn shift_clicking_a_preview_selects_without_playing() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(0, egui::Modifiers::NONE);
        let preview = screen.first_preview_of_scene(2);
        screen.click_at(preview, egui::Modifiers::SHIFT);

        assert_eq!(screen.selected_scenes(), vec![0, 1, 2]);
        assert!(screen.last_follow_up.is_none());
    }

    #[test]
    fn a_drag_that_starts_on_a_preview_still_merges_scenes() {
        let mut screen = Screen::with_five_scenes();
        let from = screen.first_preview_of_scene(0);
        let to = screen.first_preview_of_scene(1);
        screen.drag(from, to);

        assert_eq!(screen.scene_ends(), vec![40.0, 60.0, 80.0, 100.0]);
        assert!(screen.last_follow_up.is_none());
    }

    #[test]
    fn only_scenes_over_forty_five_minutes_are_long() {
        let scene_of = |seconds: f64| TimeSpan {
            start: 100.0,
            end: 100.0 + seconds,
        };
        assert!(!is_long_scene(scene_of(45.0 * 60.0)));
        assert!(is_long_scene(scene_of(45.0 * 60.0 + 1.0)));
    }

    #[test]
    fn the_tips_can_be_hidden_and_merging_still_works() {
        let mut screen = Screen::with_five_scenes();
        screen.settings.show_tips = false;
        // The screen is laid out afresh without the tips before it is used.
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        screen.frame(Vec::new(), egui::Modifiers::NONE);

        screen.drag_scene_onto(0, 1);
        assert_eq!(screen.scene_ends(), vec![40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn the_join_button_between_two_cards_merges_them() {
        let mut screen = Screen::with_five_scenes();
        let merge = screen.button_between_scene_and_next(0, "Merge scenes 1 and 2");
        screen.click_at(merge, egui::Modifiers::NONE);

        assert_eq!(screen.scene_ends(), vec![40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn previews_fill_the_card_within_their_limits() {
        const GAP: f32 = 10.0;
        // Four previews and three gaps fit exactly.
        assert_eq!(
            preview_size_for(4.0 * 200.0 + 3.0 * GAP, GAP),
            egui::vec2(200.0, 113.0)
        );
        assert_eq!(preview_size_for(5000.0, GAP).x, MAX_PREVIEW_WIDTH);
        assert_eq!(preview_size_for(100.0, GAP).x, MIN_PREVIEW_WIDTH);
    }

    #[test]
    fn every_tip_says_something() {
        for (topic, tip) in TIPS {
            assert!(!topic.is_empty());
            assert!(tip.ends_with('.'), "{tip}");
        }
    }

    #[test]
    fn ctrl_z_after_a_split_from_the_player_removes_that_split() {
        let mut screen = Screen::with_five_scenes();
        screen.session.split_near(47.0);
        assert_eq!(screen.session.scene_count(), 6);

        screen.press_ctrl_z();
        assert_eq!(screen.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
        assert!(!screen.session.is_edited());
    }

    #[test]
    fn the_screen_itself_never_changes_the_session() {
        let mut screen = Screen::with_five_scenes();
        let from = screen.empty_part_of_scene(0);
        let to = screen.empty_part_of_scene(1);
        let none = egui::Modifiers::NONE;
        screen.frame(vec![egui::Event::PointerMoved(from)], none);
        screen.frame(vec![Screen::button(from, true, none)], none);
        screen.frame(vec![egui::Event::PointerMoved(to)], none);

        // The last frame of the drag is drawn without applying its requests.
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            time: Some(screen.time),
            events: vec![Screen::button(to, false, none)],
            ..Default::default()
        };
        let mut requests = Vec::new();
        let _ = screen.ctx.run(input, |ctx| {
            requests = screen.view.show(
                ctx,
                &screen.session,
                &screen.settings,
                None,
                KeyboardOwner::Screens,
            );
        });

        assert_eq!(
            requests,
            vec![ReviewRequest::MergeScenes(
                SceneIndex::new(0)..=SceneIndex::new(1)
            )]
        );
        assert_eq!(screen.session.scene_count(), 5);
    }

    const BAR: egui::Rangef = egui::Rangef {
        min: 100.0,
        max: 1100.0,
    };

    fn spans_of_twenty_seconds(scene_count: usize) -> Vec<TimeSpan> {
        (0..scene_count)
            .map(|scene| TimeSpan {
                start: scene as f64 * 20.0,
                end: (scene + 1) as f64 * 20.0,
            })
            .collect()
    }

    fn blocks_of(scene_count: usize) -> Vec<egui::Rangef> {
        let whole_video = TimeSpan {
            start: 0.0,
            end: scene_count as f64 * 20.0,
        };
        timeline_blocks(
            spans_of_twenty_seconds(scene_count).into_iter(),
            whole_video,
            BAR,
        )
    }

    #[test]
    fn each_scene_gets_its_share_of_the_timeline_less_a_gap() {
        let blocks = blocks_of(5);
        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[0], egui::Rangef::new(100.0, 297.0));
        assert_eq!(blocks[4], egui::Rangef::new(900.0, 1097.0));
    }

    #[test]
    fn the_pointer_on_the_timeline_is_over_the_scene_whose_block_it_is_in() {
        let blocks = blocks_of(5);
        let scene_at = |x: f32| scene_at_x(&blocks, x).map(SceneIndex::position);
        assert_eq!(scene_at(100.0), Some(0));
        assert_eq!(scene_at(250.0), Some(0));
        assert_eq!(scene_at(300.0), Some(1));
        assert_eq!(scene_at(650.0), Some(2));
        assert_eq!(scene_at(1099.0), Some(4));
    }

    #[test]
    fn the_gap_after_a_block_still_points_at_its_scene() {
        let blocks = blocks_of(5);
        // Scene 1 ends at 297, and scene 2 begins at 300.
        assert_eq!(scene_at_x(&blocks, 298.5), Some(SceneIndex::new(0)));
    }

    #[test]
    fn a_pointer_before_the_first_block_is_over_no_scene() {
        assert_eq!(scene_at_x(&blocks_of(5), 99.0), None);
        assert_eq!(scene_at_x(&[], 500.0), None);
    }

    #[test]
    fn every_spot_of_a_crowded_timeline_points_at_the_scene_drawn_on_top_there() {
        // Two thousand scenes on a bar a thousand points wide: each block is
        // a sliver that the next one is drawn over.
        let blocks = blocks_of(2000);
        assert!(blocks
            .iter()
            .all(|block| block.span() >= TIMELINE_MIN_BLOCK_WIDTH));

        let mut scene_before = 0;
        for tenth_of_a_point in 0..10_000 {
            let x = BAR.min + tenth_of_a_point as f32 / 10.0;
            let scene = scene_at_x(&blocks, x).expect("a scene everywhere on the bar");
            let block = blocks[scene.position()];
            assert!(block.min <= x, "{x} is before {block:?}");
            // No later block begins at or before this spot.
            let next_begins_later = blocks
                .get(scene.position() + 1)
                .is_none_or(|next| next.min > x);
            assert!(next_begins_later, "{x}");
            assert!(scene.position() >= scene_before);
            scene_before = scene.position();
        }
        assert_eq!(scene_before, 1999);
    }

    #[test]
    fn a_video_with_no_length_still_has_a_block() {
        let instant = TimeSpan {
            start: 0.0,
            end: 0.0,
        };
        let blocks = timeline_blocks([instant].into_iter(), instant, BAR);
        assert_eq!(
            blocks,
            vec![egui::Rangef::new(100.0, 100.0 + TIMELINE_MIN_BLOCK_WIDTH)]
        );
    }

    #[test]
    fn a_label_is_only_written_in_a_block_it_fits_in() {
        let widths = TimelineLabelWidths {
            name: 50.0,
            length: 70.0,
            short_length: 30.0,
            number: 8.0,
        };
        let room_for = |text_width: f32| text_width + 2.0 * TIMELINE_LABEL_PADDING;
        let both = room_for(widths.name_and_length());
        assert_eq!(timeline_label(widths, both), TimelineLabel::NameAndLength);
        let short = room_for(widths.name_and_short_length());
        assert_eq!(
            timeline_label(widths, both - 1.0),
            TimelineLabel::NameAndShortLength
        );
        assert_eq!(
            timeline_label(widths, short),
            TimelineLabel::NameAndShortLength
        );
        assert_eq!(timeline_label(widths, short - 1.0), TimelineLabel::Name);
        assert_eq!(timeline_label(widths, room_for(50.0)), TimelineLabel::Name);
        assert_eq!(
            timeline_label(widths, room_for(49.0)),
            TimelineLabel::Number
        );
        assert_eq!(timeline_label(widths, room_for(8.0)), TimelineLabel::Number);
        assert_eq!(
            timeline_label(widths, room_for(7.0)),
            TimelineLabel::Nothing
        );
        assert!(label_fits(20.0, 20.0 + 2.0 * TIMELINE_LABEL_PADDING));
        assert!(!label_fits(20.0, 19.0 + 2.0 * TIMELINE_LABEL_PADDING));
        assert!(!label_fits(80.0, 40.0));
    }

    #[test]
    fn clicking_the_timeline_selects_the_scene_under_the_pointer() {
        let mut screen = Screen::with_five_scenes();
        let bar = screen.ctx.read_response(timeline_id()).unwrap().rect;
        // Half way along the video is the middle of the third scene.
        screen.click_at(bar.center(), egui::Modifiers::NONE);
        assert_eq!(screen.selected_scenes(), vec![2]);

        let near_the_end = egui::pos2(bar.right() - 5.0, bar.center().y);
        screen.click_at(near_the_end, egui::Modifiers::NONE);
        assert_eq!(screen.selected_scenes(), vec![4]);
    }

    #[test]
    fn a_scene_of_few_parts_shows_them_all() {
        let all_six: Vec<PartRow> = (0..6).map(PartRow::Part).collect();
        assert_eq!(part_rows(6, PartsView::Collapsed), all_six);
        assert_eq!(part_rows(1, PartsView::Collapsed), vec![PartRow::Part(0)]);
    }

    #[test]
    fn a_scene_of_many_parts_shows_its_first_and_last_few() {
        assert_eq!(
            part_rows(7, PartsView::Collapsed),
            vec![
                PartRow::Part(0),
                PartRow::Part(1),
                PartRow::Hidden { part_count: 3 },
                PartRow::Part(5),
                PartRow::Part(6),
            ]
        );
        assert_eq!(
            part_rows(40, PartsView::Collapsed)[2],
            PartRow::Hidden { part_count: 36 }
        );
    }

    #[test]
    fn a_scene_the_person_expanded_shows_every_part() {
        let all_forty: Vec<PartRow> = (0..40).map(PartRow::Part).collect();
        assert_eq!(part_rows(40, PartsView::Expanded), all_forty);
    }

    #[test]
    fn collapsing_never_hides_or_repeats_a_part() {
        for part_count in 0..30 {
            let rows = part_rows(part_count, PartsView::Collapsed);
            let shown = rows
                .iter()
                .filter(|row| matches!(row, PartRow::Part(_)))
                .count();
            let hidden: usize = rows
                .iter()
                .map(|row| match row {
                    PartRow::Hidden { part_count } => *part_count,
                    PartRow::Part(_) => 0,
                })
                .sum();
            assert_eq!(shown + hidden, part_count);
        }
    }

    /// One scene made of ten merged parts of twenty seconds each.
    fn screen_with_one_scene_of_ten_parts() -> Screen {
        let mut session = test_sessions::scenes_of_twenty_seconds(10);
        let all_scenes = SceneIndex::new(0)..=SceneIndex::new(9);
        session.apply(
            ReviewRequest::MergeScenes(all_scenes),
            &test_sessions::nothing_in_the_way,
        );
        Screen::showing(session, SCREEN)
    }

    #[test]
    fn a_long_merged_scene_grows_when_expanded_and_stays_expanded() {
        let mut screen = screen_with_one_scene_of_ten_parts();
        let layout = screen.session.plan().scene_layouts()[0].clone();
        assert_eq!(screen.view.parts_view(&layout), PartsView::Collapsed);
        let collapsed_height = screen.view.scene_cards[0].height();

        screen.view.set_parts_view(&layout, PartsView::Expanded);
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        assert_eq!(screen.view.parts_view(&layout), PartsView::Expanded);
        assert!(screen.view.scene_cards[0].height() > collapsed_height * 1.5);

        screen.view.set_parts_view(&layout, PartsView::Collapsed);
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        assert_eq!(screen.view.scene_cards[0].height(), collapsed_height);
    }

    #[test]
    fn an_expanded_scene_is_remembered_by_where_it_begins() {
        let mut screen = screen_with_one_scene_of_ten_parts();
        let layout = screen.session.plan().scene_layouts()[0].clone();
        screen.view.set_parts_view(&layout, PartsView::Expanded);

        // Splitting the last part off leaves a scene of nine parts that
        // begins where the scene of ten did.
        let last_merged_cut = *layout.merged_cuts.last().unwrap();
        screen.session.apply(
            ReviewRequest::SeparateAt(last_merged_cut),
            &test_sessions::nothing_in_the_way,
        );
        let layouts = screen.session.plan().scene_layouts();
        assert_eq!(layouts[0].segments.len(), 9);
        assert_eq!(screen.view.parts_view(&layouts[0]), PartsView::Expanded);
        assert_eq!(screen.view.parts_view(&layouts[1]), PartsView::Collapsed);
    }

    const SMALL_WINDOW: egui::Vec2 = egui::vec2(1200.0, 700.0);
    /// Tall enough to show sixty scene cards at once.
    const VERY_TALL_WINDOW: egui::Vec2 = egui::vec2(1200.0, 40_000.0);
    const MANY_SCENES: usize = 60;

    fn screen_with_many_scenes(window: egui::Vec2) -> Screen {
        Screen::showing(test_sessions::scenes_of_twenty_seconds(MANY_SCENES), window)
    }

    fn card_is_laid_out(screen: &Screen, scene: usize) -> bool {
        screen
            .ctx
            .read_response(scene_card_id(SceneIndex::new(scene)))
            .is_some()
    }

    /// Where each card begins and ends, counted from the top of the first.
    fn card_places(screen: &Screen) -> Vec<(f32, f32)> {
        let top = screen.view.scene_cards[0].top();
        screen
            .view
            .scene_cards
            .iter()
            .map(|card| (card.top() - top, card.bottom() - top))
            .collect()
    }

    #[test]
    fn cards_out_of_sight_are_not_laid_out() {
        let screen = screen_with_many_scenes(SMALL_WINDOW);
        assert_eq!(screen.view.scene_cards.len(), MANY_SCENES);
        assert!(card_is_laid_out(&screen, 0));
        assert!(!card_is_laid_out(&screen, MANY_SCENES - 1));
        assert!(!card_is_laid_out(&screen, MANY_SCENES / 2));
    }

    #[test]
    fn cards_out_of_sight_keep_exactly_the_room_they_take_when_drawn() {
        let small = screen_with_many_scenes(SMALL_WINDOW);
        let tall = screen_with_many_scenes(VERY_TALL_WINDOW);
        assert!(card_is_laid_out(&tall, MANY_SCENES - 1));
        assert_eq!(card_places(&small), card_places(&tall));
    }

    #[test]
    fn clicking_the_timeline_brings_a_card_that_was_out_of_sight_into_view() {
        let mut screen = screen_with_many_scenes(SMALL_WINDOW);
        let places_before = card_places(&screen);
        let bar = screen.ctx.read_response(timeline_id()).unwrap().rect;
        let on_the_last_scene = egui::pos2(bar.right() - 5.0, bar.center().y);

        screen.click_at(on_the_last_scene, egui::Modifiers::NONE);
        // The list scrolls over the next frames.
        for _ in 0..40 {
            screen.frame(Vec::new(), egui::Modifiers::NONE);
        }
        let last = MANY_SCENES - 1;
        assert_eq!(screen.selected_scenes(), vec![last]);
        assert!(card_is_laid_out(&screen, last));
        assert!(screen
            .view
            .list_area
            .intersects(screen.view.scene_cards[last]));
        assert!(!card_is_laid_out(&screen, 0));
        // Scrolling moved nothing within the list.
        assert_eq!(card_places(&screen), places_before);
    }

    #[test]
    fn only_previews_in_view_are_asked_for() {
        let screen = screen_with_many_scenes(SMALL_WINDOW);
        let size = screen.view.preview_pixels;
        // The first scene runs from 0 to 20 seconds, the last ends at 1200.
        assert!(screen.view.previews.contains_key(&preview_key(2.5, size)));
        assert!(!screen
            .view
            .previews
            .contains_key(&preview_key(1197.5, size)));
    }

    #[test]
    fn previews_are_decoded_for_the_sharpness_of_the_screen() {
        let mut screen = Screen::with_five_scenes();
        assert_eq!(screen.view.preview_pixels, thumbnails::SMALLEST_THUMBNAIL);
        assert!(!screen.view.previews.is_empty());

        screen.ctx.set_pixels_per_point(2.0);
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        screen.frame(Vec::new(), egui::Modifiers::NONE);
        let sharp = PictureSize {
            width: 480,
            height: 270,
        };
        assert_eq!(screen.view.preview_pixels, sharp);
        // Every preview is asked for again at the new size, and none decoded
        // for the old scale is kept.
        assert!(screen.view.previews.contains_key(&preview_key(2.5, sharp)));
        assert!(screen.view.previews.keys().all(|key| key.size == sharp));
    }

    #[test]
    fn the_same_moment_at_another_size_is_another_preview() {
        let sharp = PictureSize {
            width: 480,
            height: 270,
        };
        assert_ne!(
            preview_key(12.5, thumbnails::SMALLEST_THUMBNAIL),
            preview_key(12.5, sharp)
        );
        assert_eq!(preview_key(12.5004, sharp), preview_key(12.5, sharp));
    }

    #[test]
    fn a_merge_is_described_with_the_scene_numbers_people_see() {
        let first_and_third = span_between(SceneIndex::new(2), SceneIndex::new(0));
        assert_eq!(merge_description(&first_and_third), "Merge scenes 1 to 3");
    }

    #[test]
    fn a_short_chapter_title_is_shown_whole_and_a_long_one_is_cut_short() {
        assert_eq!(title_for_card("The heist"), "The heist");

        let exactly_fits = "x".repeat(MOST_TITLE_CHARACTERS_SHOWN);
        assert_eq!(title_for_card(&exactly_fits), exactly_fits);

        let long = format!("{exactly_fits} and then some more");
        let shown = title_for_card(&long);
        assert_eq!(shown, format!("{exactly_fits}{TITLE_CUT_SHORT_MARK}"));
    }

    #[test]
    fn a_title_in_letters_of_several_bytes_is_cut_between_letters() {
        let long = "é".repeat(MOST_TITLE_CHARACTERS_SHOWN + 5);
        let shown = title_for_card(&long);
        assert_eq!(
            shown.chars().count(),
            MOST_TITLE_CHARACTERS_SHOWN + TITLE_CUT_SHORT_MARK.len()
        );
    }

    #[test]
    fn the_timeline_names_the_chapter_that_begins_with_the_scene_under_the_pointer() {
        let session = test_sessions::five_scenes();
        let layouts = session.plan().scene_layouts();
        let second = SceneIndex::new(1);

        assert_eq!(
            scene_description(second, &layouts[1], Some("The heist"), SceneFate::Keep),
            "Scene 2   The heist   20 s   (00:00:20.000 to 00:00:40.000)"
        );
        assert_eq!(
            scene_description(second, &layouts[1], None, SceneFate::Keep),
            "Scene 2   20 s   (00:00:20.000 to 00:00:40.000)"
        );
    }

    /// The main panel of the smallest window: 900 points less the sidebar.
    const SMALLEST_MAIN_PANEL: egui::Vec2 = egui::vec2(630.0, 600.0);

    /// Five scenes from the scan, in a file whose chapters name two of them
    /// and add a split of their own.
    fn session_with_chapters() -> ReviewSession {
        let mut detection = test_sessions::scenes_of_twenty_seconds_detected(5);
        detection.chapters = crate::detect::ChapterMarks::Read(vec![
            crate::chapters::Chapter {
                start: 0.0,
                title: Some("Opening".to_owned()),
            },
            crate::chapters::Chapter {
                start: 31.0,
                title: Some("A chapter with a title far too long to fit a card".to_owned()),
            },
        ]);
        test_sessions::fresh_scan_of(detection)
    }

    #[test]
    fn a_review_with_chapters_and_every_settings_part_open_is_drawn_at_the_smallest_size() {
        let mut screen = Screen::showing(session_with_chapters(), SMALLEST_MAIN_PANEL);
        assert_eq!(screen.session.scene_count(), 6);
        screen.view.missed_scenes_open = true;
        screen.view.black_screen_settings = SettingsCard::JustOpened;
        let settings_before = screen.settings.clone();

        screen.frame(Vec::new(), egui::Modifiers::NONE);
        screen.frame(Vec::new(), egui::Modifiers::NONE);

        // Drawing asked for nothing: no setting changed, no scene changed.
        assert_eq!(screen.settings, settings_before);
        assert_eq!(screen.session.scene_count(), 6);
        assert_eq!(screen.last_follow_up, None);
        // The card was brought into view once and then left where it is.
        assert_eq!(screen.view.black_screen_settings, SettingsCard::Open);
        // Every card got its place, the one with the long title included.
        assert_eq!(screen.view.scene_cards.len(), 6);
    }

    #[test]
    fn a_folders_own_settings_are_drawn_without_changing_them() {
        let mut screen = Screen::with_five_scenes();
        let video = Arc::clone(screen.session.video());
        screen.settings.remember_detection_for_folder_of(&video);
        screen.view.missed_scenes_open = true;
        screen.view.black_screen_settings = SettingsCard::Open;
        let settings_before = screen.settings.clone();

        screen.frame(Vec::new(), egui::Modifiers::NONE);
        screen.click_scene(1, egui::Modifiers::NONE);
        assert_eq!(screen.settings, settings_before);
    }
}
