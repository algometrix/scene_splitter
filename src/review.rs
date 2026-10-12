//! The review screen of one video: its scenes as colour-coded cards with
//! previews. The person drags one scene onto another to merge them, before
//! anything is written.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use anyhow::Result;
use eframe::egui;

use crate::black::TimeSpan;
use crate::detect::Detection;
use crate::export::{ExportOptions, OriginalPolicy, TRASH_NAME};
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::plan::{AddCutError, CutChoice, ProposedCut, SceneLayout, SplitPlan};
use crate::selection::{ClickKind, SceneSelection};
use crate::settings::Settings;
use crate::split::{self, SceneLocation};
use crate::theme::{self, scene_color};
use crate::thumbnails::{self, Thumbnail, THUMBNAIL_HEIGHT, THUMBNAIL_WIDTH};
use crate::timecode::{format_clock, format_duration, format_timestamp, parse_timestamp};

const NEW_CUT_FIELD_WIDTH: f32 = 90.0;
/// The room the "Add a split at" label, box and button take together.
const NEW_CUT_CONTROLS_WIDTH: f32 = 340.0;
const PREVIEW_ROUNDING: f32 = 6.0;
const SPACE_BETWEEN_SCENES: f32 = 10.0;
const FILE_NAME_FONT_SIZE: f32 = 16.0;
const SECONDS_PER_MINUTE: f64 = 60.0;
/// The range offered for the shortest scene a hard-cut scan may produce.
const MIN_SCENE_MINUTES: std::ops::RangeInclusive<f64> = 0.5..=60.0;
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
/// Narrower blocks have no room for their scene number.
const TIMELINE_MIN_LABELLED_WIDTH: f32 = 22.0;
/// Blocks at least this wide also show how long their scene is.
const TIMELINE_MIN_WIDTH_FOR_LENGTH: f32 = 120.0;
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
     If this video changes scene without a black screen, press \"Scenes missed?\" above. \
     You can also click a picture and press S in the player to split by hand.";
const RESCAN_WARNING: &str = "Scanning again removes the merges and splits you made on this video.";

/// Previews are drawn as wide as the card allows, within these limits.
const PREVIEWS_PER_ROW: f32 = 4.0;
const MIN_PREVIEW_WIDTH: f32 = 96.0;
const MAX_PREVIEW_WIDTH: f32 = 240.0;
const PREVIEW_ASPECT: f32 = 9.0 / 16.0;
/// Room kept beside the previews of a merged scene for its length and button.
const MERGED_PART_CONTROLS_WIDTH: f32 = 200.0;
const SCENE_BADGE_MIN_WIDTH: f32 = 56.0;

/// What the person asked for on the review screen.
pub enum ReviewRequest {
    Export,
    Skip,
    /// Throw away the stored scan of this video and scan it again.
    ScanAgain,
    /// Scan for scene changes without a black screen as well.
    FindHardCuts,
    /// Play the video from this time.
    PlayFrom(f64),
}

/// Previews are looked up by their time in whole milliseconds.
type PreviewKey = u64;

fn preview_key(time: f64) -> PreviewKey {
    (time * 1000.0).round() as PreviewKey
}

enum Preview {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
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
        first_scene: usize,
    },
}

/// Why the split has not started after the person asked for it.
enum ExportObstacle {
    /// Scene files of the same names exist and would be replaced.
    ExistingFiles(String),
    Error(String),
}

/// What the person did to the scene list during one frame.
#[derive(Default)]
struct SceneListInput {
    clicked_scene: Option<usize>,
    drag_started_on: Option<usize>,
    cut_to_split_again: Option<usize>,
    /// A strip the person wants out of its merged scene, as an index among
    /// all the plan's segments.
    segment_to_isolate: Option<usize>,
    /// The time of a preview the person clicked, to play the video from.
    play_from: Option<f64>,
    /// The scene to join with the one after it.
    join_with_next: Option<usize>,
}

impl SceneListInput {
    /// A click on a preview selects its scene, like a click anywhere else on
    /// the card. A plain click also plays the video from that preview. With
    /// Shift or Ctrl held the person is building a selection, so it does not.
    fn note_preview_click(&mut self, scene: usize, preview_time: f64, ui: &egui::Ui) {
        self.clicked_scene = Some(scene);
        let modifiers = ui.input(|input| input.modifiers);
        if click_kind(modifiers) == ClickKind::Single {
            self.play_from = Some(preview_time);
        }
    }
}

pub struct Review {
    video: Arc<Path>,
    plan: SplitPlan,
    /// The cuts as they were before each change, newest last, for undo.
    undo_history: Vec<Vec<ProposedCut>>,
    scan_summary: String,
    /// How long one frame of the video is shown, on average.
    frame_seconds: f64,
    previews: HashMap<PreviewKey, Preview>,
    loaded_previews_sender: Sender<LoadedPreview>,
    loaded_previews: Receiver<LoadedPreview>,
    selection: SceneSelection,
    /// Where each scene's card was drawn in the last frame. The whole card
    /// takes clicks and drags, and a drag ends on the card under the pointer.
    scene_cards: Vec<egui::Rect>,
    /// The visible part of the scene list, which cards can scroll out of.
    list_area: egui::Rect,
    /// The scene being dragged onto another, while the mouse button is down.
    dragged_scene: Option<usize>,
    /// A scene clicked on the timeline, to bring its card into view.
    scene_to_scroll_to: Option<usize>,
    new_cut_text: String,
    new_cut_feedback: String,
    export_obstacle: Option<ExportObstacle>,
    /// Whether the options for finding missed scenes are shown.
    missed_scenes_open: bool,
    /// A scan the person asked for that would throw away their edits, until
    /// they confirm or cancel it.
    rescan_awaiting_consent: Option<Rescan>,
    /// How the scan read the video, for the curious.
    scan_details: String,
}

/// The two ways of scanning a video again.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Rescan {
    BlackScreens,
    WithSuddenChanges,
}

impl Rescan {
    fn request(self) -> ReviewRequest {
        match self {
            Self::BlackScreens => ReviewRequest::ScanAgain,
            Self::WithSuddenChanges => ReviewRequest::FindHardCuts,
        }
    }
}

impl Review {
    /// `cuts` are the cuts to start from, which the caller picks from the
    /// detection: its black-screen cuts, or those plus its hard cuts.
    pub fn new(video: Arc<Path>, detection: Detection, cuts: Vec<f64>, scan_note: &str) -> Self {
        let frame_seconds = detection.video.seconds() / detection.frame_count.max(1) as f64;
        let black_screens = counted(detection.black_screens.len(), "black screen");
        let sudden_changes = cuts.len().saturating_sub(detection.cuts.len());
        let found = if sudden_changes > 0 {
            format!(
                "Found {}: {black_screens}, {}.",
                counted(cuts.len(), "scene change"),
                counted(sudden_changes, "sudden picture change")
            )
        } else {
            format!("Found {black_screens}.")
        };
        let scan_summary = format!("{found} {scan_note}");
        let scan_details = format!(
            "{} of {} frames were decoded to find them.",
            detection.decoded_frames, detection.frame_count
        );
        let plan = SplitPlan::from_detected_cuts(&cuts, detection.keyframe_times, detection.video);
        let (loaded_previews_sender, loaded_previews) = mpsc::channel();
        Self {
            video,
            plan,
            undo_history: Vec::new(),
            scan_summary,
            frame_seconds,
            previews: HashMap::new(),
            loaded_previews_sender,
            loaded_previews,
            selection: SceneSelection::default(),
            scene_cards: Vec::new(),
            list_area: egui::Rect::NOTHING,
            dragged_scene: None,
            scene_to_scroll_to: None,
            new_cut_text: String::new(),
            new_cut_feedback: String::new(),
            export_obstacle: None,
            missed_scenes_open: false,
            rescan_awaiting_consent: None,
            scan_details,
        }
    }

    pub fn scene_count(&self) -> usize {
        self.plan.scenes().len()
    }

    pub fn video_length(&self) -> f64 {
        self.plan.video().end
    }

    pub fn frame_seconds(&self) -> f64 {
        self.frame_seconds
    }

    /// The keyframe a split asked for at `time` would land on, or `None`
    /// where no split is possible.
    pub fn split_time_near(&self, time: f64) -> Option<f64> {
        self.plan.cut_time_near(time).ok()
    }

    pub fn accepted_cuts(&self) -> Vec<f64> {
        self.plan.accepted_cuts()
    }

    /// `playhead` is where the player is in this video, if it is playing it.
    ///
    /// The header stays put and holds only what is needed all the time: the
    /// name, the main action, the timeline and the editing buttons. The
    /// numbers, hints and tips scroll with the scenes, so the scenes always
    /// have room however small the window is.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        playhead: Option<f64>,
    ) -> Option<ReviewRequest> {
        self.receive_loaded_previews(ctx);
        self.handle_shortcuts(ctx);

        let request = egui::TopBottomPanel::top("review_header")
            .frame(theme::header_panel())
            .show_separator_line(false)
            .show(ctx, |ui| self.show_header(ui, settings, playhead))
            .inner;
        let input = egui::CentralPanel::default()
            .frame(theme::panel(theme::BACKGROUND))
            .show(ctx, |ui| {
                self.list_area = ui.max_rect();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    // Dragging moves scenes, the wheel and scroll bar scroll.
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        self.show_overview(ui, &mut settings.show_tips);
                        self.show_scenes(ui)
                    })
                    .inner
            })
            .inner;
        let play_request = input.play_from.map(ReviewRequest::PlayFrom);
        self.apply_scene_list_input(ctx, input);
        self.show_drag_hint(ctx);
        request.or(play_request)
    }

    fn show_header(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut Settings,
        playhead: Option<f64>,
    ) -> Option<ReviewRequest> {
        let mut request = self.show_title_and_main_action(ui, &settings.export);
        if self.show_export_obstacle(ui) {
            request = Some(ReviewRequest::Export);
        }
        if self.show_export_options(ui, &mut settings.export) {
            self.export_obstacle = None;
        }
        self.show_timeline(ui, playhead);
        ui.horizontal_wrapped(|ui| {
            self.show_merge_controls(ui);
            self.show_undo_and_reset(ui);
            ui.separator();
            // The label, box and button stay together: when they do not all
            // fit on this row they start the next one.
            if ui.available_size_before_wrap().x < NEW_CUT_CONTROLS_WIDTH {
                ui.end_row();
            }
            self.show_new_cut_controls(ui);
            if !settings.show_tips && ui.button("Show tips").clicked() {
                settings.show_tips = true;
            }
        });
        request.or(self.show_scan_row(ui, &mut settings.hard_cuts))
    }

    /// The video's name, cut short if it is long, with the two things that
    /// end the review at the right edge where they are always in view.
    fn show_title_and_main_action(
        &mut self,
        ui: &mut egui::Ui,
        export_options: &ExportOptions,
    ) -> Option<ReviewRequest> {
        let mut request = None;
        ui.horizontal(|ui| {
            // Laid out from the right, so the buttons keep their place and
            // the name takes whatever room is left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let scene_count = self.scene_count();
                let label = format!("Split into {}", counted(scene_count, "file"));
                // One scene is the untouched video, there is nothing to write.
                let export_button = ui
                    .add_enabled(scene_count > 1, theme::primary_button(label))
                    .on_disabled_hover_text(
                        "There is only one scene, so there is nothing to split.",
                    );
                if export_button.clicked() {
                    request = self.request_export(export_options);
                }
                let skip_button = ui
                    .button("Skip this video")
                    .on_hover_text("Leaves this video as it is. Nothing is written.");
                if skip_button.clicked() {
                    request = Some(ReviewRequest::Skip);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let name = file_name(&self.video);
                    ui.add(
                        egui::Label::new(theme::semibold(&name, FILE_NAME_FONT_SIZE)).truncate(),
                    )
                    .on_hover_text(name);
                });
            });
        });
        request
    }

    /// What the scan found, and the ways to scan again.
    fn show_scan_row(
        &mut self,
        ui: &mut egui::Ui,
        hard_cuts: &mut HardCutSettings,
    ) -> Option<ReviewRequest> {
        let mut wanted = None;
        ui.horizontal_wrapped(|ui| {
            // Same text size as the links beside it, so the line reads level.
            ui.label(theme::muted(&self.scan_summary))
                .on_hover_text(&self.scan_details);
            let scan_again = ui
                .link("Scan again")
                .on_hover_text("Looks for black screens again, with the current settings.");
            if scan_again.clicked() {
                wanted = Some(Rescan::BlackScreens);
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
        if self.missed_scenes_open && show_sudden_change_controls(ui, hard_cuts) {
            wanted = Some(Rescan::WithSuddenChanges);
        }

        // A scan replaces the scenes, so edits are not thrown away unasked.
        if let Some(rescan) = wanted {
            if self.is_edited() {
                self.rescan_awaiting_consent = Some(rescan);
            } else {
                return Some(rescan.request());
            }
        }
        self.show_rescan_consent(ui)
    }

    fn show_rescan_consent(&mut self, ui: &mut egui::Ui) -> Option<ReviewRequest> {
        let rescan = self.rescan_awaiting_consent?;
        let mut request = None;
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(theme::WARNING_TEXT, RESCAN_WARNING);
            if ui.add(theme::danger_button("Scan again anyway")).clicked() {
                self.rescan_awaiting_consent = None;
                request = Some(rescan.request());
            }
            if ui.button("Cancel").clicked() {
                self.rescan_awaiting_consent = None;
            }
        });
        request
    }

    fn is_edited(&self) -> bool {
        !self.undo_history.is_empty() || !self.plan.is_as_detected()
    }

    /// The part that scrolls away above the scenes: the key numbers, what
    /// deserves attention, and the tips.
    fn show_overview(&self, ui: &mut egui::Ui, show_tips: &mut bool) {
        ui.horizontal_wrapped(|ui| self.show_key_numbers(ui));
        if self.scene_count() == 1 {
            show_note(ui, NOTHING_FOUND_HINT, theme::PEACH_TILE);
        }
        for (scene, span) in self.plan.scenes().into_iter().enumerate() {
            if is_long_scene(span) {
                let warning = format!(
                    "Scene {} is {} long. A scene change may have been missed inside it.",
                    scene + 1,
                    format_duration(span.seconds())
                );
                show_note(ui, &warning, theme::ROSE_TILE);
            }
        }
        if *show_tips {
            show_tips_card(ui, show_tips);
        }
        ui.add_space(SPACE_BETWEEN_SCENES);
    }

    /// The numbers a person wants at a glance: how many scenes the split
    /// would write, how long the video is, and how long its longest scene is.
    fn show_key_numbers(&self, ui: &mut egui::Ui) {
        let scenes = self.plan.scenes();
        let piece_count = self.plan.segments().len();
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
            &format_duration(self.plan.video().seconds()),
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
                &format_duration(longest.seconds()),
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

    /// The whole video as one bar, with a coloured block per scene. Clicking
    /// a block brings that scene's card into view.
    fn show_timeline(&mut self, ui: &mut egui::Ui, playhead: Option<f64>) {
        let video = self.plan.video();
        let size = egui::vec2(ui.available_width(), TIMELINE_HEIGHT);
        let (bar, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let x_at = |time: f64| {
            // A video of a single frame has no length to take a share of.
            let length = video.seconds().max(f64::MIN_POSITIVE);
            let share = ((time - video.start) / length).clamp(0.0, 1.0);
            bar.left() + bar.width() * share as f32
        };

        for (scene, layout) in self.plan.scene_layouts().iter().enumerate() {
            let left = x_at(layout.span.start);
            let right = (x_at(layout.span.end) - TIMELINE_GAP_BETWEEN_SCENES)
                .max(left + TIMELINE_MIN_BLOCK_WIDTH);
            let block = egui::Rect::from_min_max(
                egui::pos2(left, bar.top()),
                egui::pos2(right, bar.bottom()),
            );
            let block_id = ui.id().with(("timeline_scene", scene));
            let response = ui.interact(block, block_id, egui::Sense::click());

            let painter = ui.painter();
            painter.rect_filled(block, TIMELINE_ROUNDING, scene_color(scene));
            if is_long_scene(layout.span) {
                let outline = egui::Stroke::new(LONG_SCENE_OUTLINE_WIDTH, theme::DANGER);
                painter.rect_stroke(block, TIMELINE_ROUNDING, outline);
            }
            // A faint line where two detected pieces were merged.
            for &cut_index in &layout.merged_cuts {
                let cut_time = self.plan.cuts()[cut_index].time;
                let line = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(140));
                painter.vline(x_at(cut_time), block.y_range(), line);
            }
            if block.width() >= TIMELINE_MIN_LABELLED_WIDTH {
                let label = if block.width() >= TIMELINE_MIN_WIDTH_FOR_LENGTH {
                    format!("{}   {}", scene + 1, format_duration(layout.span.seconds()))
                } else {
                    (scene + 1).to_string()
                };
                painter.text(
                    block.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(13.0),
                    egui::Color32::WHITE,
                );
            }

            if response.clicked() {
                self.selection = SceneSelection::only(scene);
                self.scene_to_scroll_to = Some(scene);
            }
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(scene_description(scene, layout));
        }

        if let Some(playhead) = playhead {
            let line = egui::Stroke::new(PLAYHEAD_WIDTH, theme::TEXT);
            ui.painter().vline(x_at(playhead), bar.y_range(), line);
        }
    }

    /// Splitting replaces files of the same name, so existing scene files
    /// need a second, explicit confirmation.
    fn request_export(&mut self, export_options: &ExportOptions) -> Option<ReviewRequest> {
        let existing_files = split::scene_dir(&self.video, export_options.scene_location)
            .and_then(|dir| split::scene_files_from_earlier_splits(&self.video, &dir));
        match existing_files {
            Ok(files) if files.is_empty() => Some(ReviewRequest::Export),
            Ok(files) => {
                self.export_obstacle = Some(ExportObstacle::ExistingFiles(format!(
                    "{} from an earlier split would be replaced.",
                    counted(files.len(), "scene file")
                )));
                None
            }
            Err(error) => {
                self.export_obstacle = Some(ExportObstacle::Error(format!("{error:#}")));
                None
            }
        }
    }

    /// Returns true when the person confirmed replacing the existing files.
    fn show_export_obstacle(&self, ui: &mut egui::Ui) -> bool {
        let mut confirmed = false;
        match &self.export_obstacle {
            Some(ExportObstacle::ExistingFiles(warning)) => {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(theme::WARNING_TEXT, warning);
                    confirmed = ui.add(theme::danger_button("Replace them")).clicked();
                });
            }
            Some(ExportObstacle::Error(error)) => {
                ui.colored_label(theme::DANGER, error);
            }
            None => {}
        }
        confirmed
    }

    /// What happens to the original and where the scenes go, right under
    /// the button they belong to. Returns true when the place changed.
    fn show_export_options(&self, ui: &mut egui::Ui, export_options: &mut ExportOptions) -> bool {
        let mut location_changed = false;
        ui.horizontal_wrapped(|ui| {
            let mut recycle = export_options.original_policy == OriginalPolicy::RecycleOnceVerified;
            let recycle_label = format!(
                "Then move the original to the {TRASH_NAME} (only if the new files pass the check)"
            );
            let recycle_box = ui.checkbox(&mut recycle, recycle_label).on_hover_text(format!(
                "The check compares the new files with the original. If anything is missing \
                 or different, the original stays where it is. You can restore it from the {TRASH_NAME}."
            ));
            if recycle_box.changed() {
                export_options.original_policy = if recycle {
                    OriginalPolicy::RecycleOnceVerified
                } else {
                    OriginalPolicy::Keep
                };
            }

            let mut new_folder = export_options.scene_location == SceneLocation::NewFolder;
            if ui
                .checkbox(&mut new_folder, "Put the scenes in a new folder")
                .changed()
            {
                export_options.scene_location = if new_folder {
                    SceneLocation::NewFolder
                } else {
                    SceneLocation::BesideVideo
                };
                location_changed = true;
            }
            if let Ok(dir) = split::scene_dir(&self.video, export_options.scene_location) {
                let destination = dir.display().to_string();
                ui.add(
                    // Body size, so it lines up with the checkbox text before it.
                    egui::Label::new(theme::muted(format!("Scenes go to {destination}")))
                        .truncate(),
                )
                .on_hover_text(destination);
            }
        });
        location_changed
    }

    fn show_merge_controls(&mut self, ui: &mut egui::Ui) {
        let span_to_merge = self.selection.span_to_merge();
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
            if let Some(span) = span_to_merge {
                self.merge_scenes(span);
            }
        }

        let merged_scenes_selected = self.selected_layouts().iter().any(is_merged);
        let undo_merge_button = ui
            .add_enabled(merged_scenes_selected, egui::Button::new("Undo merge"))
            .on_hover_text("Takes the selected merged scenes apart again.")
            .on_disabled_hover_text("Select a merged scene to take it apart again.");
        if undo_merge_button.clicked() {
            self.split_selected_apart();
        }
    }

    fn show_undo_and_reset(&mut self, ui: &mut egui::Ui) {
        let undo_clicked = ui
            .add_enabled(!self.undo_history.is_empty(), egui::Button::new("Undo"))
            .on_hover_text("Takes back the last change (Ctrl+Z).")
            .on_disabled_hover_text("Nothing to take back yet.")
            .clicked();
        if undo_clicked {
            self.undo();
        }

        let reset_clicked = ui
            .add_enabled(
                !self.plan.is_as_detected(),
                egui::Button::new("Reset scenes"),
            )
            .on_hover_text("Undoes every merge and removes the splits you added.")
            .on_disabled_hover_text("The scenes are as the scan found them.")
            .clicked();
        if reset_clicked {
            self.edit_plan(SplitPlan::reset_to_detected);
        }
    }

    fn show_new_cut_controls(&mut self, ui: &mut egui::Ui) {
        // One unit, so a narrow window wraps all three together.
        ui.horizontal(|ui| {
            ui.label("Add a split at");
            // A hint that is a real time in this video, not one beyond its end.
            let example_time = format_clock(self.plan.video().seconds() / 2.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.new_cut_text)
                    .hint_text(example_time)
                    .desired_width(NEW_CUT_FIELD_WIDTH),
            );
            let pressed_enter =
                field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui.button("Add split").clicked() || pressed_enter {
                self.add_requested_cut();
            }
        });
        ui.label(theme::muted(&self.new_cut_feedback));
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // Keys typed into a text box belong to the text box.
        if ctx.wants_keyboard_input() {
            return;
        }
        let (merge, select_all, clear, undo) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::M),
                input.modifiers.command && input.key_pressed(egui::Key::A),
                input.key_pressed(egui::Key::Escape),
                input.modifiers.command && input.key_pressed(egui::Key::Z),
            )
        });
        if select_all {
            self.selection.select_all(self.scene_count());
        }
        if clear {
            self.selection.clear();
        }
        if undo {
            self.undo();
        }
        if merge {
            if let Some(span) = self.selection.span_to_merge() {
                self.merge_scenes(span);
            }
        }
    }

    /// Every change to the plan goes through here, so each one can be undone
    /// and none leaves stale state behind.
    fn edit_plan<Outcome>(&mut self, edit: impl FnOnce(&mut SplitPlan) -> Outcome) -> Outcome {
        let cuts_before = self.plan.cuts().to_vec();
        let outcome = edit(&mut self.plan);
        if self.plan.cuts() != cuts_before {
            self.undo_history.push(cuts_before);
            self.forget_what_depended_on_the_old_plan();
        }
        outcome
    }

    fn undo(&mut self) {
        let Some(earlier_cuts) = self.undo_history.pop() else {
            return;
        };
        self.plan.restore_cuts(earlier_cuts);
        self.forget_what_depended_on_the_old_plan();
    }

    /// Scene numbers shift when the plan changes, so a selection made
    /// before would point at other scenes. A warning about existing files
    /// was likewise given for the plan as it was.
    fn forget_what_depended_on_the_old_plan(&mut self) {
        self.selection.clear();
        self.export_obstacle = None;
    }

    fn merge_scenes(&mut self, scenes: RangeInclusive<usize>) {
        let merged_scene = *scenes.start();
        self.edit_plan(|plan| plan.merge_scenes(scenes));
        self.selection = SceneSelection::only(merged_scene);
    }

    fn split_selected_apart(&mut self) {
        let selected_spans: Vec<TimeSpan> = self
            .selected_layouts()
            .iter()
            .map(|layout| layout.span)
            .collect();
        self.edit_plan(|plan| {
            for span in selected_spans {
                plan.restore_splits_within(span);
            }
        });
    }

    fn add_requested_cut(&mut self) {
        let Some(requested_time) = parse_timestamp(&self.new_cut_text) else {
            self.new_cut_feedback = "Type a time like 1:03:12.".to_owned();
            return;
        };
        self.new_cut_feedback = match self.add_split_near(requested_time) {
            Ok(time) => {
                self.new_cut_text.clear();
                split_confirmation(time)
            }
            Err(error) => error.to_string(),
        };
    }

    /// Splits the scene playing at `time` in two, on the keyframe nearest to
    /// that time, and says what happened. This is what the player's split
    /// button does.
    pub fn split_near(&mut self, time: f64) -> String {
        match self.add_split_near(time) {
            Ok(cut_time) => split_confirmation(cut_time),
            Err(error) => error.to_string(),
        }
    }

    fn add_split_near(&mut self, time: f64) -> Result<f64, AddCutError> {
        self.edit_plan(|plan| plan.add_cut_near(time))
    }

    fn selected_layouts(&self) -> Vec<SceneLayout> {
        let layouts = self.plan.scene_layouts();
        self.selection
            .scenes()
            .filter_map(|scene| layouts.get(scene).cloned())
            .collect()
    }

    fn show_scenes(&mut self, ui: &mut egui::Ui) -> SceneListInput {
        self.scroll_while_dragging_near_an_edge(ui);

        let drop_target = self.drop_target(ui.ctx());
        let cards_last_frame = std::mem::take(&mut self.scene_cards);
        let mut input = SceneListInput::default();
        let layouts = self.plan.scene_layouts();
        for (scene, layout) in layouts.iter().enumerate() {
            let card_last_frame = cards_last_frame.get(scene).copied();
            let card = self.show_scene(ui, scene, layout, card_last_frame, drop_target, &mut input);
            if self.scene_to_scroll_to == Some(scene) {
                ui.scroll_to_rect(card, Some(egui::Align::TOP));
            }
            self.scene_cards.push(card);

            let is_last_scene = scene + 1 == layouts.len();
            if is_last_scene {
                ui.add_space(SPACE_BETWEEN_SCENES);
            } else if show_join_button(ui, scene) {
                input.join_with_next = Some(scene);
            }
        }
        self.scene_to_scroll_to = None;
        input
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

    /// Draws one scene card and returns where it ended up.
    fn show_scene(
        &mut self,
        ui: &mut egui::Ui,
        scene: usize,
        layout: &SceneLayout,
        card_last_frame: Option<egui::Rect>,
        drop_target: Option<usize>,
        input: &mut SceneListInput,
    ) -> egui::Rect {
        // The card's own click and drag area is registered before its
        // contents, so the buttons and previews on top of it still work.
        // Its size is only known after drawing, hence last frame's rectangle.
        if let Some(card) = card_last_frame {
            let card_id = ui.id().with(("scene_card", scene));
            let response = ui.interact(card, card_id, egui::Sense::click_and_drag());
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

        let (color, fill, outline) = match self.highlight(scene, drop_target) {
            Highlight::None => {
                let color = scene_color(scene);
                (
                    color,
                    theme::CARD,
                    egui::Stroke::new(1.0_f32, theme::CARD_BORDER),
                )
            }
            Highlight::Selected => {
                let color = scene_color(scene);
                (
                    color,
                    tinted_card(color, SELECTED_TINT_STRENGTH),
                    egui::Stroke::new(2.0_f32, color),
                )
            }
            Highlight::WillMerge { first_scene } => {
                let color = scene_color(first_scene);
                (
                    color,
                    tinted_card(color, WILL_MERGE_TINT_STRENGTH),
                    egui::Stroke::new(2.0_f32, color),
                )
            }
        };

        let card = egui::Frame::none()
            .fill(fill)
            .rounding(theme::CARD_ROUNDING)
            .stroke(outline)
            .shadow(theme::soft_shadow())
            .inner_margin(CARD_MARGIN)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                show_scene_heading(ui, scene, layout, color);
                let room_for_previews = if is_merged(layout) {
                    ui.available_width() - MERGED_PART_CONTROLS_WIDTH
                } else {
                    ui.available_width()
                };
                let preview_size = preview_size_for(room_for_previews, ui.spacing().item_spacing.x);
                for (segment_index, segment) in layout.segments.iter().enumerate() {
                    // Wrapped, so a narrow card shows two rows of pictures
                    // instead of cutting the last ones off.
                    ui.horizontal_wrapped(|ui| {
                        for time in thumbnails::preview_times(*segment) {
                            if self.show_preview(ui, time, preview_size) {
                                input.note_preview_click(scene, time, ui);
                            }
                        }
                        if is_merged(layout) {
                            ui.vertical(|ui| {
                                ui.label(theme::muted(format_duration(segment.seconds())));
                                if show_move_out_button(ui) {
                                    input.segment_to_isolate =
                                        Some(layout.first_segment + segment_index);
                                }
                            });
                        }
                    });
                    if let Some(&cut_index) = layout.merged_cuts.get(segment_index) {
                        let cut_time = self.plan.cuts()[cut_index].time;
                        let label = format!("Separate here ({})", format_clock(cut_time));
                        let separate = ui
                            .button(label)
                            .on_hover_text("Splits the merged scene in two at this point again.");
                        if separate.clicked() {
                            input.cut_to_split_again = Some(cut_index);
                        }
                    }
                }
            })
            .response
            .rect;

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

    /// The scene the dragged scene would be merged with if released now.
    fn drop_target(&self, ctx: &egui::Context) -> Option<usize> {
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
            .filter(|&target| target != source)
    }

    fn highlight(&self, scene: usize, drop_target: Option<usize>) -> Highlight {
        if let (Some(source), Some(target)) = (self.dragged_scene, drop_target) {
            let will_merge = span_between(source, target);
            if will_merge.contains(&scene) {
                return Highlight::WillMerge {
                    first_scene: *will_merge.start(),
                };
            }
        }
        if self.selection.contains(scene) || self.dragged_scene == Some(scene) {
            Highlight::Selected
        } else {
            Highlight::None
        }
    }

    fn apply_scene_list_input(&mut self, ctx: &egui::Context, input: SceneListInput) {
        if let Some(segment) = input.segment_to_isolate {
            self.edit_plan(|plan| plan.isolate_segment(segment));
            return;
        }
        if let Some(cut_index) = input.cut_to_split_again {
            self.edit_plan(|plan| plan.set_choice(cut_index, CutChoice::Split));
            return;
        }
        if let Some(scene) = input.join_with_next {
            self.merge_scenes(scene..=scene + 1);
            return;
        }
        if let Some(scene) = input.clicked_scene {
            let modifiers = ctx.input(|input| input.modifiers);
            self.selection.click(scene, click_kind(modifiers));
        }
        if let Some(scene) = input.drag_started_on {
            self.dragged_scene = Some(scene);
        }

        let Some(source) = self.dragged_scene else {
            return;
        };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let button_released =
            ctx.input(|input| input.pointer.any_released() || !input.pointer.any_down());
        if !button_released {
            return;
        }
        if let Some(target) = self.drop_target(ctx) {
            self.merge_scenes(span_between(source, target));
        }
        self.dragged_scene = None;
    }

    /// Says next to the pointer what releasing the drag would do.
    fn show_drag_hint(&self, ctx: &egui::Context) {
        let (Some(source), Some(pointer)) = (self.dragged_scene, ctx.pointer_latest_pos()) else {
            return;
        };
        let hint = match self.drop_target(ctx) {
            Some(target) => merge_description(&span_between(source, target)),
            None => format!("Drop scene {} on another scene to merge them", source + 1),
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
    fn show_preview(&mut self, ui: &mut egui::Ui, time: f64, size: egui::Vec2) -> bool {
        let key = preview_key(time);
        // Only previews that can be seen are fetched, so opening a video of
        // sixty scenes does not start hundreds of ffmpeg runs at once.
        let about_to_be_drawn_at = egui::Rect::from_min_size(ui.cursor().min, size);
        if !self.previews.contains_key(&key) && ui.is_rect_visible(about_to_be_drawn_at) {
            self.start_loading_preview(ui.ctx(), key, time);
        }

        // Previews take clicks but not drags, so a drag that starts on a
        // preview still moves the whole scene card underneath.
        let response = match self.previews.get(&key) {
            Some(Preview::Ready(texture)) => ui.add(
                egui::Image::new((texture.id(), size))
                    .rounding(PREVIEW_ROUNDING)
                    .sense(egui::Sense::click()),
            ),
            Some(Preview::Failed) => show_placeholder(ui, size, "No preview"),
            Some(Preview::Loading) | None => show_placeholder(ui, size, "Loading"),
        };
        let clicked = response.clicked();
        if self.dragged_scene.is_none() {
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("Play from {}", format_clock(time)));
        }
        clicked
    }

    fn start_loading_preview(&mut self, ctx: &egui::Context, key: PreviewKey, time: f64) {
        self.previews.insert(key, Preview::Loading);

        let video = Arc::clone(&self.video);
        let loaded_previews_sender = self.loaded_previews_sender.clone();
        let ctx = ctx.clone();
        rayon::spawn(move || {
            let thumbnail = thumbnails::grab_thumbnail(&video, time);
            // The send only fails once this review is gone, when nobody
            // needs the preview any more.
            if loaded_previews_sender
                .send(LoadedPreview { key, thumbnail })
                .is_ok()
            {
                ctx.request_repaint();
            }
        });
    }

    fn receive_loaded_previews(&mut self, ctx: &egui::Context) {
        for loaded in self.loaded_previews.try_iter() {
            let preview = match loaded.thumbnail {
                Ok(thumbnail) => {
                    let image = egui::ColorImage::from_rgb(
                        [THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT],
                        &thumbnail.rgb,
                    );
                    Preview::Ready(ctx.load_texture(
                        format!("preview-{}", loaded.key),
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                }
                Err(error) => {
                    eprintln!("Preview failed: {error:#}");
                    Preview::Failed
                }
            };
            self.previews.insert(loaded.key, preview);
        }
    }
}

/// How to use the review screen, for someone seeing it for the first time.
/// It scrolls away with the scenes, can be hidden, and the choice is kept.
fn show_tips_card(ui: &mut egui::Ui, show_tips: &mut bool) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(theme::semibold("How to use this screen", 14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Hide tips").clicked() {
                    *show_tips = false;
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

/// The small button between two scene cards. Returns true when clicked.
fn show_join_button(ui: &mut egui::Ui, scene: usize) -> bool {
    let label = format!("Join scenes {} and {}", scene + 1, scene + 2);
    ui.vertical_centered(|ui| {
        ui.add(egui::Button::new(theme::caption(label)).frame(false))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Makes these two scenes one. You can also drag one onto the other.")
            .clicked()
    })
    .inner
}

/// The options of the slower scan, for videos that change scene without a
/// black screen. Returns true when the person asked for that scan.
fn show_sudden_change_controls(ui: &mut egui::Ui, settings: &mut HardCutSettings) -> bool {
    let mut requested = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Detect");
        egui::ComboBox::from_id_salt("sudden_change_sensitivity")
            .selected_text(settings.sensitivity.label())
            .show_ui(ui, |ui| {
                for sensitivity in Sensitivity::ALL {
                    ui.selectable_value(
                        &mut settings.sensitivity,
                        sensitivity,
                        sensitivity.label(),
                    );
                }
            });

        ui.label("and ignore scenes shorter than");
        let mut minutes = settings.min_scene_seconds / SECONDS_PER_MINUTE;
        let shortest_scene = egui::DragValue::new(&mut minutes)
            .range(MIN_SCENE_MINUTES)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" min");
        if ui.add(shortest_scene).changed() {
            settings.min_scene_seconds = minutes * SECONDS_PER_MINUTE;
        }

        requested = ui
            .button("Look for sudden picture changes")
            .on_hover_text(
                "Looks at every picture of the video for places where it changes abruptly. \
                 This takes much longer than the first scan.",
            )
            .clicked();
    });
    requested
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

fn span_between(one_scene: usize, another_scene: usize) -> RangeInclusive<usize> {
    one_scene.min(another_scene)..=one_scene.max(another_scene)
}

fn merge_description(scenes: &RangeInclusive<usize>) -> String {
    format!(
        "Merge scenes {} to {}",
        scenes.start() + 1,
        scenes.end() + 1
    )
}

/// "1 scene", "3 scenes".
pub fn counted(count: usize, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

fn is_merged(layout: &SceneLayout) -> bool {
    !layout.merged_cuts.is_empty()
}

fn is_long_scene(span: TimeSpan) -> bool {
    span.seconds() > LONG_SCENE_SECONDS
}

fn split_confirmation(cut_time: f64) -> String {
    format!(
        "Split at {}, the nearest keyframe.",
        format_timestamp(cut_time)
    )
}

fn scene_description(scene: usize, layout: &SceneLayout) -> String {
    format!(
        "Scene {}   {}   ({} to {})",
        scene + 1,
        format_duration(layout.span.seconds()),
        format_timestamp(layout.span.start),
        format_timestamp(layout.span.end)
    )
}

fn show_scene_heading(ui: &mut egui::Ui, scene: usize, layout: &SceneLayout, color: egui::Color32) {
    ui.horizontal_wrapped(|ui| {
        let badge =
            theme::semibold(format!("Scene {}", scene + 1), 13.0).color(egui::Color32::WHITE);
        theme::pill_at_least(ui, badge, color, SCENE_BADGE_MIN_WIDTH);

        let length = theme::semibold(
            format_duration(layout.span.seconds()),
            SCENE_LENGTH_FONT_SIZE,
        );
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
        if is_merged(layout) {
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

/// These tests drive the real review screen with simulated mouse input, the
/// way a person would use it. The video does not exist, so previews fail to
/// load and show placeholders, which does not matter for merging.
#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: egui::Vec2 = egui::vec2(1200.0, 3000.0);
    const SECONDS_PER_FRAME: f64 = 0.1;
    const DRAG_STEPS: usize = 8;

    struct Screen {
        ctx: egui::Context,
        review: Review,
        settings: Settings,
        time: f64,
        last_request: Option<ReviewRequest>,
    }

    impl Screen {
        /// A 100 second video cut into five scenes of 20 seconds each.
        fn with_five_scenes() -> Self {
            let detection = Detection {
                video: TimeSpan {
                    start: 0.0,
                    end: 100.0,
                },
                keyframe_times: (0..10).map(|keyframe| keyframe as f64 * 10.0).collect(),
                frame_count: 3000,
                black_screens: Vec::new(),
                cuts: vec![20.0, 40.0, 60.0, 80.0],
                decoded_frames: 0,
                decoded_windows: 0,
                hard_cut_candidates: None,
            };
            let cuts = detection.cuts.clone();
            let video: Arc<Path> = Arc::from(Path::new("no_such_video.mp4"));
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut screen = Self {
                ctx,
                review: Review::new(video, detection, cuts, ""),
                settings: Settings::default(),
                time: 0.0,
                last_request: None,
            };
            // Cards take input from their second frame on.
            screen.frame(Vec::new(), egui::Modifiers::NONE);
            screen.frame(Vec::new(), egui::Modifiers::NONE);
            screen
        }

        fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
                time: Some(self.time),
                modifiers,
                events,
                ..Default::default()
            };
            self.time += SECONDS_PER_FRAME;
            let Self {
                ctx,
                review,
                settings,
                last_request,
                ..
            } = self;
            let _ = ctx.run(input, |ctx| {
                if let Some(request) = review.show(ctx, settings, None) {
                    *last_request = Some(request);
                }
            });
        }

        /// A spot on a scene's card with nothing else on it: its heading row,
        /// to the right of the text.
        fn empty_part_of_scene(&self, scene: usize) -> egui::Pos2 {
            let card = self.review.scene_cards[scene];
            egui::pos2(card.right() - 40.0, card.top() + CARD_MARGIN.top + 8.0)
        }

        /// The middle of the first preview of a scene that is not merged.
        fn first_preview_of_scene(&self, scene: usize) -> egui::Pos2 {
            let card = self.review.scene_cards[scene];
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

        fn press_ctrl_z(&mut self) {
            let event = egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            };
            self.frame(vec![event], egui::Modifiers::COMMAND);
            self.frame(Vec::new(), egui::Modifiers::NONE);
        }

        fn scene_ends(&self) -> Vec<f64> {
            self.review
                .plan
                .scenes()
                .iter()
                .map(|scene| scene.end)
                .collect()
        }

        fn selected_scenes(&self) -> Vec<usize> {
            self.review.selection.scenes().collect()
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
        assert_eq!(screen.review.scene_count(), 2);
    }

    #[test]
    fn dropping_a_scene_on_itself_changes_nothing() {
        let mut screen = Screen::with_five_scenes();
        screen.drag_scene_onto(2, 2);
        assert_eq!(screen.review.scene_count(), 5);
    }

    #[test]
    fn ctrl_z_takes_back_one_merge_at_a_time() {
        let mut screen = Screen::with_five_scenes();
        screen.drag_scene_onto(0, 2);
        screen.drag_scene_onto(1, 2);
        assert_eq!(screen.review.scene_count(), 2);

        screen.press_ctrl_z();
        assert_eq!(screen.scene_ends(), vec![60.0, 80.0, 100.0]);

        screen.press_ctrl_z();
        assert_eq!(screen.review.scene_count(), 5);

        // With nothing left to undo, another press is harmless.
        screen.press_ctrl_z();
        assert_eq!(screen.review.scene_count(), 5);
    }

    #[test]
    fn a_split_that_is_refused_leaves_nothing_to_undo() {
        let mut screen = Screen::with_five_scenes();
        screen.review.split_near(41.0);
        assert!(screen.review.undo_history.is_empty());
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
    fn clicking_a_preview_plays_the_video_from_that_moment() {
        let mut screen = Screen::with_five_scenes();
        let preview = screen.first_preview_of_scene(2);
        screen.click_at(preview, egui::Modifiers::NONE);

        // Scene 2 runs from 40 to 60 seconds, and its first preview is taken
        // 2.5 seconds in.
        assert!(matches!(
            screen.last_request,
            Some(ReviewRequest::PlayFrom(time)) if time == 42.5
        ));
        assert_eq!(screen.selected_scenes(), vec![2]);
        assert_eq!(screen.review.scene_count(), 5);
    }

    #[test]
    fn shift_clicking_a_preview_selects_without_playing() {
        let mut screen = Screen::with_five_scenes();
        screen.click_scene(0, egui::Modifiers::NONE);
        let preview = screen.first_preview_of_scene(2);
        screen.click_at(preview, egui::Modifiers::SHIFT);

        assert_eq!(screen.selected_scenes(), vec![0, 1, 2]);
        assert!(screen.last_request.is_none());
    }

    #[test]
    fn a_drag_that_starts_on_a_preview_still_merges_scenes() {
        let mut screen = Screen::with_five_scenes();
        let from = screen.first_preview_of_scene(0);
        let to = screen.first_preview_of_scene(1);
        screen.drag(from, to);

        assert_eq!(screen.scene_ends(), vec![40.0, 60.0, 80.0, 100.0]);
        assert!(screen.last_request.is_none());
    }

    #[test]
    fn splitting_from_the_player_cuts_the_playing_scene_in_two() {
        let mut screen = Screen::with_five_scenes();
        // 47 seconds is inside the scene from 40 to 60, nearest keyframe 50.
        let feedback = screen.review.split_near(47.0);

        assert_eq!(feedback, "Split at 00:00:50.000, the nearest keyframe.");
        assert_eq!(
            screen.scene_ends(),
            vec![20.0, 40.0, 50.0, 60.0, 80.0, 100.0]
        );
    }

    #[test]
    fn splitting_where_a_split_already_is_says_so() {
        let mut screen = Screen::with_five_scenes();
        let feedback = screen.review.split_near(41.0);

        assert_eq!(feedback, "There is already a split point at 00:00:40.000.");
        assert_eq!(screen.review.scene_count(), 5);
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
        let first = screen.review.scene_cards[0];
        let second = screen.review.scene_cards[1];
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
    fn a_fresh_review_has_no_edits_and_a_merge_is_one() {
        let mut screen = Screen::with_five_scenes();
        assert!(!screen.review.is_edited());

        screen.drag_scene_onto(0, 1);
        assert!(screen.review.is_edited());
    }

    #[test]
    fn every_tip_says_something() {
        for (topic, tip) in TIPS {
            assert!(!topic.is_empty());
            assert!(tip.ends_with('.'), "{tip}");
        }
    }

    #[test]
    fn counts_read_naturally() {
        assert_eq!(counted(1, "scene"), "1 scene");
        assert_eq!(counted(3, "scene"), "3 scenes");
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
