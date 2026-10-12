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
use crate::export::{ExportOptions, TRASH_NAME};
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::keyboard::{self, KeyboardOwner};
use crate::plan::{CutIndex, SceneIndex, SceneLayout, SegmentIndex, SplitPlan};
use crate::review_session::{counted, ExportObstacle, ReviewRequest, ReviewSession};
use crate::scan::UndecodedFrames;
use crate::selection::ClickKind;
use crate::settings::{Settings, SettingsChange};
use crate::settings_controls;
use crate::split;
use crate::theme::{self, scene_color};
use crate::thumbnails::{self, PictureSize, Thumbnail};
use crate::timecode::{format_clock, format_length, format_timestamp, TimeSpan};

const NEW_CUT_FIELD_WIDTH: f32 = 90.0;
/// The room the "Add a split at" label, box and button take together.
const NEW_CUT_CONTROLS_WIDTH: f32 = 340.0;
const PREVIEW_ROUNDING: f32 = 6.0;
const SPACE_BETWEEN_SCENES: f32 = 10.0;
const FILE_NAME_FONT_SIZE: f32 = 16.0;
const SCENE_LENGTH_FONT_SIZE: f32 = 16.0;

const SELECTED_TINT_STRENGTH: f32 = 0.12;
const WILL_MERGE_TINT_STRENGTH: f32 = 0.30;
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
        "Join",
        "Drag one scene onto another, or press Join between two scenes.",
    ),
    (
        "Cut",
        "Click a picture to play from there, then press S where the scene should end.",
    ),
    (
        "Undo",
        "Ctrl+Z takes back the last change. Reset scenes goes back to the scenes as found.",
    ),
];
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
    /// The size previews are decoded at, which suits the sharpness of the
    /// screen the window is on.
    preview_pixels: PictureSize,
    /// The long merged scenes the person asked to see every part of.
    expanded_scenes: HashSet<SceneKey>,
    /// How tall each scene's card was when it was last drawn, for leaving
    /// the right room for it while it is out of sight.
    card_heights: HashMap<CardShape, f32>,
    /// How tall the "Join scenes" button between two cards is, once one was
    /// drawn. They are all alike.
    join_button_height: Option<f32>,
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
            preview_pixels: thumbnails::SMALLEST_THUMBNAIL,
            expanded_scenes: HashSet::new(),
            card_heights: HashMap::new(),
            join_button_height: None,
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
        self.decode_previews_for_the_sharpness_of_the_screen(ctx);
        self.receive_loaded_previews(ctx);
        let mut requests = shortcut_requests(ctx, keyboard);

        let asked_in_header = egui::TopBottomPanel::top("review_header")
            .frame(theme::header_panel())
            .show_separator_line(false)
            .show(ctx, |ui| self.show_header(ui, session, settings, playhead))
            .inner;
        requests.extend(asked_in_header);

        let (asked_in_overview, input) = egui::CentralPanel::default()
            .frame(theme::panel(theme::BACKGROUND))
            .show(ctx, |ui| {
                self.list_area = ui.max_rect();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    // Dragging moves scenes, the wheel and scroll bar scroll.
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        let asked_in_overview = show_overview(ui, session, settings);
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

    fn show_header(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        settings: &Settings,
        playhead: Option<f64>,
    ) -> Vec<ReviewRequest> {
        let mut requests = Vec::new();
        requests.extend(show_title_and_main_action(ui, session));
        requests.extend(show_export_obstacle(ui, session));
        requests.extend(show_export_options(ui, session.video(), settings.export));
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
        requests.extend(self.show_scan_row(ui, session, settings.hard_cuts));
        requests.extend(show_rescan_consent(ui, session));
        requests
    }

    /// What the scan found, and the ways to scan again.
    fn show_scan_row(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        hard_cuts: HardCutSettings,
    ) -> Vec<ReviewRequest> {
        let mut requests = Vec::new();
        ui.horizontal_wrapped(|ui| {
            // Same text size as the links beside it, so the line reads level.
            ui.label(theme::muted(session.scan_summary()))
                .on_hover_text(session.scan_details());
            let scan_again = ui
                .link("Scan again")
                .on_hover_text("Looks for black screens again, with the current settings.");
            if scan_again.clicked() {
                requests.push(ReviewRequest::Rescan(ScanKind::BlackScreens));
            }
            let missed_label = if self.missed_scenes_open {
                "Hide the options for missed scenes"
            } else {
                "Scenes missed?"
            };
            let missed = ui
                .link(missed_label)
                .on_hover_text("For videos that change scene without a black screen.");
            if missed.clicked() {
                self.missed_scenes_open = !self.missed_scenes_open;
            }
        });
        if self.missed_scenes_open {
            requests.extend(show_hard_cut_scan_controls(ui, hard_cuts));
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
        let plan = session.plan();
        let whole_video = plan.whole_video();
        let layouts = plan.scene_layouts();
        let size = egui::vec2(ui.available_width(), TIMELINE_HEIGHT);
        let (_, bar) = ui.allocate_space(size);
        let response = ui.interact(bar, timeline_id(), egui::Sense::click());
        let blocks = timeline_blocks(
            layouts.iter().map(|layout| layout.span),
            whole_video,
            bar.x_range(),
        );

        let painter = ui.painter();
        for ((scene, layout), &across) in with_scene_indices(&layouts).zip(&blocks) {
            let block = egui::Rect::from_x_y_ranges(across, bar.y_range());
            paint_timeline_block(painter, block, scene, layout);
            // A faint line where two detected pieces were merged.
            for &cut_index in &layout.merged_cuts {
                let cut_x = x_at_time(plan.cut_time(cut_index), whole_video, bar.x_range());
                let line = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(140));
                painter.vline(cut_x, block.y_range(), line);
            }
        }
        if let Some(playhead) = playhead {
            let line = egui::Stroke::new(PLAYHEAD_WIDTH, theme::TEXT);
            let playhead_x = x_at_time(playhead, whole_video, bar.x_range());
            painter.vline(playhead_x, bar.y_range(), line);
        }

        let scene = response
            .hover_pos()
            .and_then(|pointer| scene_at_x(&blocks, pointer.x))?;
        let layout = layouts.get(scene.position())?;
        let clicked = response.clicked();
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text_at_pointer(scene_description(scene, layout));
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
        for (scene, layout) in with_scene_indices(&layouts) {
            let shape = self.card_shape(layout, ui.available_width());
            // A card far from the part of the list in view is not laid out.
            // It only keeps its room, so the scroll bar and every card after
            // it stay where they are. A card of a shape not drawn before is
            // laid out once, to learn its height.
            let room_out_of_sight = heights_last_frame
                .get(&shape)
                .and_then(|&height| leave_room_if_out_of_sight(ui, height));
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
                    self.show_scene(ui, session, scene, layout, drop_target, &mut input)
                }
            };
            self.card_heights.insert(shape, card.height());
            if self.scene_to_scroll_to == Some(scene) {
                ui.scroll_to_rect(card, Some(egui::Align::TOP));
            }
            self.scene_cards.push(card);

            let is_last_scene = scene.next().position() == layouts.len();
            if is_last_scene {
                ui.add_space(SPACE_BETWEEN_SCENES);
            } else if self.show_join_button_if_in_sight(ui, scene) {
                input.join_with_next = Some(scene);
            }
        }
        self.scene_to_scroll_to = None;
        input
    }

    /// Draws the button that joins `scene` with the next one, or only leaves
    /// room for it while it is out of sight. Returns true when it was clicked.
    fn show_join_button_if_in_sight(&mut self, ui: &mut egui::Ui, scene: SceneIndex) -> bool {
        let room_out_of_sight = self
            .join_button_height
            .and_then(|height| leave_room_if_out_of_sight(ui, height));
        if room_out_of_sight.is_some() {
            return false;
        }
        let button = show_join_button(ui, scene);
        self.join_button_height = Some(button.height);
        button.clicked
    }

    fn card_shape(&self, layout: &SceneLayout, width: f32) -> CardShape {
        CardShape {
            scene_starts: millisecond(layout.span.start),
            scene_ends: millisecond(layout.span.end),
            rows: part_rows(layout.segments.len(), self.parts_view(layout)).len(),
            width: width.round() as u32,
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
        scene: SceneIndex,
        layout: &SceneLayout,
        drop_target: Option<SceneIndex>,
        input: &mut SceneListInput,
    ) -> egui::Rect {
        let (color, fill, outline) = match self.highlight(session, scene, drop_target) {
            Highlight::None => {
                let color = scene_color(scene.position());
                (
                    color,
                    theme::CARD,
                    egui::Stroke::new(1.0_f32, theme::CARD_BORDER),
                )
            }
            Highlight::Selected => {
                let color = scene_color(scene.position());
                (
                    color,
                    tinted_card(color, SELECTED_TINT_STRENGTH),
                    egui::Stroke::new(2.0_f32, color),
                )
            }
            Highlight::WillMerge { first_scene } => {
                let color = scene_color(first_scene.position());
                (
                    color,
                    tinted_card(color, WILL_MERGE_TINT_STRENGTH),
                    egui::Stroke::new(2.0_f32, color),
                )
            }
        };

        let parts_view = self.parts_view(layout);
        let mut wanted_parts_view = None;
        let card = egui::Frame::none()
            .fill(fill)
            .rounding(theme::CARD_ROUNDING)
            .stroke(outline)
            .shadow(theme::soft_shadow())
            .inner_margin(CARD_MARGIN)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                show_scene_heading(ui, scene, layout, color);
                let room_for_previews = if layout.is_merged() {
                    ui.available_width() - MERGED_PART_CONTROLS_WIDTH
                } else {
                    ui.available_width()
                };
                let preview_size = preview_size_for(room_for_previews, ui.spacing().item_spacing.x);
                let part_count = layout.segments.len();
                let rows = part_rows(part_count, parts_view);
                for (row_position, row) in rows.iter().enumerate() {
                    match *row {
                        PartRow::Part(position_in_scene) => {
                            let part = ScenePart {
                                scene,
                                layout,
                                position_in_scene,
                            };
                            self.show_part(ui, session.video(), part, preview_size, input);
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
            for time in thumbnails::preview_times(segment) {
                if self.show_preview(ui, video, time, preview_size) {
                    input.note_preview_click(scene, time, ui);
                }
            }
            if layout.is_merged() {
                ui.vertical(|ui| {
                    ui.label(theme::muted(format_length(segment.seconds())));
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
            None => format!(
                "Drop scene {} on another scene to merge them",
                source.number()
            ),
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
    painter.rect_filled(block, TIMELINE_ROUNDING, scene_color(scene.position()));
    if is_long_scene(layout.span) {
        let outline = egui::Stroke::new(LONG_SCENE_OUTLINE_WIDTH, theme::DANGER);
        painter.rect_stroke(block, TIMELINE_ROUNDING, outline);
    }
    if block.width() < TIMELINE_MIN_LABELLED_WIDTH {
        return;
    }
    let white = egui::Color32::WHITE;
    let heavier = theme::semibold_font(TIMELINE_LABEL_FONT_SIZE);
    let name = painter.layout_no_wrap(scene_name(scene), heavier.clone(), white);
    let number = painter.layout_no_wrap(scene.number().to_string(), heavier, white);
    // Lighter than the name, so the two read as a name and a figure and not
    // as one run of numbers.
    let lighter = egui::FontId::proportional(TIMELINE_LABEL_FONT_SIZE);
    let seconds = layout.span.seconds();
    let length = painter.layout_no_wrap(format_length(seconds), lighter.clone(), white);
    let short_length = painter.layout_no_wrap(format_clock(seconds), lighter, white);
    let widths = TimelineLabelWidths {
        name: name.size().x,
        length: length.size().x,
        short_length: short_length.size().x,
        number: number.size().x,
    };
    let paint_centred = |text: std::sync::Arc<egui::Galley>| {
        painter.galley(block.center() - text.size() / 2.0, text, white);
    };
    let paint_name_with = |length: std::sync::Arc<egui::Galley>| {
        let both = widths.name + TIMELINE_LABEL_GAP + length.size().x;
        let left = block.center().x - both / 2.0;
        let length_left = left + widths.name + TIMELINE_LABEL_GAP;
        let middle = block.center().y;
        let name_top = middle - name.size().y / 2.0;
        painter.galley(egui::pos2(left, name_top), name.clone(), white);
        let length_top = middle - length.size().y / 2.0;
        painter.galley(egui::pos2(length_left, length_top), length, white);
    };
    match timeline_label(widths, block.width()) {
        TimelineLabel::NameAndLength => paint_name_with(length),
        TimelineLabel::NameAndShortLength => paint_name_with(short_length),
        TimelineLabel::Name => paint_centred(name.clone()),
        TimelineLabel::Number => paint_centred(number),
        TimelineLabel::Nothing => {}
    }
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

/// What the keys pressed in this frame ask for.
fn shortcut_requests(ctx: &egui::Context, keyboard: KeyboardOwner) -> Vec<ReviewRequest> {
    // Behind a dialog the keys are the dialog's, and keys typed into a text
    // box belong to the text box.
    if !keyboard.lets_screens_use_keys(ctx) {
        return Vec::new();
    }
    let (merge, select_all, undo) = ctx.input(|input| {
        (
            input.key_pressed(egui::Key::M),
            input.modifiers.command && input.key_pressed(egui::Key::A),
            input.modifiers.command && input.key_pressed(egui::Key::Z),
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
            let label = format!("Split into {}", counted(scene_count, "file"));
            // One scene is the untouched video, there is nothing to write.
            let export_button = ui
                .add_enabled(scene_count > 1, theme::primary_button(label))
                .on_disabled_hover_text("There is only one scene, so there is nothing to split.");
            if export_button.clicked() {
                request = Some(ReviewRequest::Split);
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

/// Why the split has not started, with the way past it when there is one.
fn show_export_obstacle(ui: &mut egui::Ui, session: &ReviewSession) -> Option<ReviewRequest> {
    let mut request = None;
    match session.export_obstacle()? {
        ExportObstacle::EarlierSceneFiles(earlier_files) => {
            let warning = format!(
                "{} from an earlier split would be replaced.",
                counted(earlier_files.file_count, "scene file")
            );
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(theme::WARNING_TEXT, warning)
                    .on_hover_text(format!("They are in {}", earlier_files.scene_dir.display()));
                if ui.add(theme::danger_button("Replace them")).clicked() {
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
                    egui::RichText::new(problem).color(theme::DANGER),
                ));
                theme::copy_details_button(ui, problem);
            });
        }
    }
    request
}

/// What happens to the original and where the scenes go, right under the
/// button they belong to. `export_options` are the saved ones. A change is
/// returned for the app to save.
fn show_export_options(
    ui: &mut egui::Ui,
    video: &Path,
    export_options: ExportOptions,
) -> Option<ReviewRequest> {
    let mut wanted = export_options;
    ui.horizontal_wrapped(|ui| {
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
        settings_controls::show_scene_location_checkbox(ui, &mut wanted.scene_location);
        if let Ok(dir) = split::scene_dir(video, wanted.scene_location) {
            let destination = dir.display().to_string();
            ui.add(
                // Body size, so it lines up with the checkbox text before it.
                egui::Label::new(theme::muted(format!("Scenes go to {destination}"))).truncate(),
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
        None => "Merge selected".to_owned(),
    };
    let merge_button = ui
        .add_enabled(span_to_merge.is_some(), egui::Button::new(merge_label))
        .on_hover_text("Joins the selected scenes into one (M).")
        .on_disabled_hover_text(
            "Click a scene, then Shift-click another, to choose the scenes to merge.",
        );
    if merge_button.clicked() {
        request = Some(ReviewRequest::MergeSelected);
    }

    let undo_merge_button = ui
        .add_enabled(
            session.has_merged_scene_selected(),
            egui::Button::new("Undo merge"),
        )
        .on_hover_text("Takes the selected merged scenes apart again.")
        .on_disabled_hover_text("Select a merged scene to take it apart again.");
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
            egui::RichText::new(feedback.in_words()).color(theme::WARNING_TEXT)
        } else {
            theme::muted(feedback.in_words())
        };
        ui.label(words);
    }
    requests
}

/// Asks before a scan throws away the person's edits.
fn show_rescan_consent(ui: &mut egui::Ui, session: &ReviewSession) -> Option<ReviewRequest> {
    if !session.is_asking_before_rescan() {
        return None;
    }
    let mut request = None;
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(theme::WARNING_TEXT, RESCAN_WARNING);
        if ui.add(theme::danger_button("Scan again anyway")).clicked() {
            request = Some(ReviewRequest::RescanAnyway);
        }
        if ui.button("Cancel").clicked() {
            request = Some(ReviewRequest::KeepEdits);
        }
    });
    request
}

/// The part that scrolls away above the scenes: the key numbers, what
/// deserves attention, and the tips.
fn show_overview(
    ui: &mut egui::Ui,
    session: &ReviewSession,
    settings: &Settings,
) -> Vec<ReviewRequest> {
    let plan = session.plan();
    let mut requests = Vec::new();
    ui.horizontal_wrapped(|ui| show_key_numbers(ui, plan));
    if session.scene_count() == 1 && show_nothing_found_note(ui) == LookHarder::Asked {
        requests.push(ReviewRequest::Rescan(look_harder_scan(settings.hard_cuts)));
    }
    for stretch in session.undecoded() {
        show_note(ui, &undecoded_warning(stretch), theme::ROSE_TILE);
    }
    for (position, span) in plan.scenes().into_iter().enumerate() {
        if is_long_scene(span) {
            let warning = format!(
                "Scene {} is {} long. A scene change may have been missed inside it.",
                SceneIndex::new(position).number(),
                format_length(span.seconds())
            );
            show_note(ui, &warning, theme::ROSE_TILE);
        }
    }
    if settings.show_tips && show_tips_card(ui) == TipsCard::HideAsked {
        requests.push(ReviewRequest::ChangeSettings(SettingsChange::HideTips));
    }
    ui.add_space(SPACE_BETWEEN_SCENES);
    requests
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
        theme::MINT_TILE,
        theme::TEXT,
    );
    theme::stat_tile(
        ui,
        &format_length(plan.whole_video().seconds()),
        "whole video",
        theme::LAVENDER_TILE,
        theme::TEXT,
    );
    if let Some(longest) = longest_scene {
        let (tint, value_color) = if is_long_scene(longest) {
            (theme::ROSE_TILE, theme::DANGER)
        } else {
            (theme::PEACH_TILE, theme::TEXT)
        };
        theme::stat_tile(
            ui,
            &format_length(longest.seconds()),
            "longest scene",
            tint,
            value_color,
        );
    }
    if piece_count > scenes.len() {
        theme::stat_tile(
            ui,
            &piece_count.to_string(),
            "parts, merged into these scenes",
            theme::BACKGROUND,
            theme::MUTED_TEXT,
        );
    }
}

/// Whether the person asked for the tips to go away.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TipsCard {
    HideAsked,
    Kept,
}

/// How to use the review screen, for someone seeing it for the first time.
/// It scrolls away with the scenes, can be hidden, and the choice is kept.
fn show_tips_card(ui: &mut egui::Ui) -> TipsCard {
    let mut tips_card = TipsCard::Kept;
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(theme::semibold("How to use this screen", 14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Hide tips").clicked() {
                    tips_card = TipsCard::HideAsked;
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
                    ui.label(theme::semibold(topic, 13.0).color(theme::PRIMARY));
                    ui.add(egui::Label::new(tip).wrap());
                    ui.end_row();
                }
            });
    });
    tips_card
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
    let mut look_harder = LookHarder::NotAsked;
    egui::Frame::none()
        .fill(theme::PEACH_TILE)
        .rounding(theme::CONTROL_ROUNDING)
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(NOTHING_FOUND_HINT);
            let button = ui
                .add(theme::primary_button(LOOK_HARDER_LABEL))
                .on_hover_text(LOOK_HARDER_EXPLANATION);
            if button.clicked() {
                look_harder = LookHarder::Asked;
            }
            ui.label(theme::muted(NOTHING_FOUND_BY_HAND_HINT));
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

/// What drawing the button between two scene cards gave.
struct JoinButton {
    clicked: bool,
    /// How much room the button took, for leaving the same room where a
    /// button is out of sight.
    height: f32,
}

/// The small button between two scene cards.
fn show_join_button(ui: &mut egui::Ui, scene: SceneIndex) -> JoinButton {
    let label = format!(
        "Join scenes {} and {}",
        scene.number(),
        scene.next().number()
    );
    let row = ui.vertical_centered(|ui| {
        ui.add(egui::Button::new(theme::caption(label)).frame(false))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Makes these two scenes one. You can also drag one onto the other.")
            .clicked()
    });
    JoinButton {
        clicked: row.inner,
        height: row.response.rect.height(),
    }
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
    let label = format!("Separate here ({})", format_clock(plan.cut_time(cut_index)));
    let separate = ui
        .button(label)
        .on_hover_text("Splits the merged scene in two at this point again.");
    if separate.clicked() {
        input.cut_to_split_again = Some(cut_index);
    }
}

/// Stands in for the middle parts of a long merged scene. Returns true when
/// the person asked to see every part.
fn show_hidden_parts_row(ui: &mut egui::Ui, hidden_count: usize, part_count: usize) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::muted(format!(
            "{} in between are not shown.",
            counted(hidden_count, "part")
        )));
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
/// black screen, and the button that starts it. `saved` are the options as
/// saved. A change to them is returned for the app to save, and so is the
/// wish to scan, with the options as they stand on screen.
fn show_hard_cut_scan_controls(ui: &mut egui::Ui, saved: HardCutSettings) -> Vec<ReviewRequest> {
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
        requests.push(ReviewRequest::ChangeSettings(SettingsChange::HardCuts(
            wanted,
        )));
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

/// A white card with a wash of the scene's colour over it.
fn tinted_card(color: egui::Color32, strength: f32) -> egui::Color32 {
    let wash = |card: u8, scene: u8| {
        (f32::from(card) + (f32::from(scene) - f32::from(card)) * strength).round() as u8
    };
    egui::Color32::from_rgb(
        wash(theme::CARD.r(), color.r()),
        wash(theme::CARD.g(), color.g()),
        wash(theme::CARD.b(), color.b()),
    )
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

fn scene_description(scene: SceneIndex, layout: &SceneLayout) -> String {
    format!(
        "Scene {}   {}   ({} to {})",
        scene.number(),
        format_length(layout.span.seconds()),
        format_timestamp(layout.span.start),
        format_timestamp(layout.span.end)
    )
}

fn show_scene_heading(
    ui: &mut egui::Ui,
    scene: SceneIndex,
    layout: &SceneLayout,
    color: egui::Color32,
) {
    ui.horizontal_wrapped(|ui| {
        let badge = theme::semibold(scene_name(scene), 13.0).color(egui::Color32::WHITE);
        theme::pill_at_least(ui, badge, color, SCENE_BADGE_MIN_WIDTH);

        let length = theme::semibold(format_length(layout.span.seconds()), SCENE_LENGTH_FONT_SIZE);
        if is_long_scene(layout.span) {
            theme::pill(ui, length.color(egui::Color32::WHITE), theme::DANGER);
            // Said in words too, so the warning does not rest on colour alone.
            ui.label(egui::RichText::new("over 45 min").color(theme::DANGER));
        } else {
            ui.label(length);
        }
        // To the second is enough here, since cuts land on keyframes anyway.
        // The exact times are one hover away.
        ui.label(theme::muted(format!(
            "{} to {}",
            format_clock(layout.span.start),
            format_clock(layout.span.end)
        )))
        .on_hover_text(format!(
            "{} to {}",
            format_timestamp(layout.span.start),
            format_timestamp(layout.span.end)
        ));
        if layout.is_merged() {
            let note = egui::RichText::new(format!("{} parts merged", layout.segments.len()))
                .color(theme::PRIMARY);
            theme::pill(ui, note, theme::PRIMARY_TINT);
        }
    });
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
    let (area, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter()
        .rect_filled(area, PREVIEW_ROUNDING, theme::BACKGROUND);
    ui.painter().text(
        area.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::default(),
        theme::MUTED_TEXT,
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
                ..
            } = self;
            let mut requests = Vec::new();
            let _ = ctx.run(input, |ctx| {
                requests = view.show(ctx, session, settings, None, *keyboard);
            });
            // No scene files of an earlier split are in the way.
            let follow_up = session.apply_frame(requests, &test_sessions::nothing_in_the_way);
            // Settings are the app's to keep, and this stands in for it.
            if let Some(FollowUp::ChangeSettings(change)) = follow_up {
                *settings = settings.changed_by(change);
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
        let before = screen.settings;
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
        let first = screen.view.scene_cards[0];
        let second = screen.view.scene_cards[1];
        let between = egui::pos2(first.center().x, (first.bottom() + second.top()) / 2.0);
        screen.click_at(between, egui::Modifiers::NONE);

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
    fn a_tinted_card_sits_between_white_and_the_scene_colour() {
        let red = egui::Color32::from_rgb(255, 0, 0);
        assert_eq!(tinted_card(red, 0.0), theme::CARD);
        assert_eq!(tinted_card(red, 1.0), red);
        assert_eq!(
            tinted_card(red, 0.2),
            egui::Color32::from_rgb(255, 204, 204)
        );
    }
}
