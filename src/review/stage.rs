//! The review screen of the Studio style, laid out as a stage: the picture
//! as large as the window allows, the tools as square buttons down the
//! left, and the scenes along the bottom as a tall film strip with a row of
//! scene buttons.
//!
//! ```text
//! [Undo]    name of the video                                 [Focus]
//! [Merge]  ┌─────────────────────────────────────────┐
//! [Drop]   │               the picture               │
//! [Rescan] │      transport: cuts, play, pictures    │
//!          └─────────────────────────────────────────┘
//!          ruler, film strip, cut markers, playhead
//!          [01  2:40] [02  2:08] ...                   [Split at 0:42]
//!          the latest answer, when there is one
//! [Recycle the original] [Save beside the video]  ...  [Split into 4 files]
//! ```
//!
//! It is the same review as the editor's, built from the same parts, so
//! what can be asked here is what can be asked there.

use super::editor::{
    card_filling, show_export_bar, show_footage_bar, show_room_button, show_split_at_button,
    ButtonWords, EditorOutput,
};
use super::*;
use crate::keyboard::{DROP_OR_KEEP_SHORTCUT, MERGE_SHORTCUT, UNDO_SHORTCUT};
use crate::player::PlayerAsk;
use crate::settings::ReviewRoom;

/// Narrow enough that, in the smallest window, the export choices and the
/// split still share one row.
const TOOLS_WIDTH: f32 = 52.0;
const TOOL_GAP: f32 = 10.0;
/// Between the title row and the stage under it.
const TITLE_GAP: f32 = 6.0;
/// The film strip is the stage's timeline, so its pictures are large
/// enough to tell one shot from the next.
const STAGE_STRIP_HEIGHT: f32 = 84.0;
const UNDO_NAME: &str = "Undo";
const MERGE_NAME: &str = "Merge the chosen scenes";
const MERGE_DISABLED_HINT: &str = "Click a scene, then Shift-click another, to choose them.";
const DROP_NAME: &str = "Drop or keep the chosen scenes";
const RESCAN_NAME: &str = "Scan again";
const DETECTION_NAME: &str = "Settings for black screens";

impl ReviewScreen {
    /// Draws the review as a stage. `show_player` draws the player into the
    /// middle of the window.
    pub fn show_stage(
        &mut self,
        ctx: &egui::Context,
        session: &ReviewSession,
        settings: &Settings,
        playhead: Option<f64>,
        keyboard: KeyboardOwner,
        show_player: &mut dyn FnMut(&mut egui::Ui),
    ) -> EditorOutput {
        let palette = theme::palette(ctx);
        self.decode_previews_for_the_sharpness_of_the_screen(ctx);
        self.receive_loaded_previews(ctx);
        let mut output = EditorOutput::default();
        output.requests.extend(shortcut_requests(ctx, keyboard));
        let asked_with_focus_keys = focus_key_requests(ctx, keyboard, session);
        self.follow_the_focus_after_its_keys(ctx, &asked_with_focus_keys);
        output.requests.extend(asked_with_focus_keys);
        let detection = settings.detection_for(session.video());

        let asked_in_the_tools = egui::SidePanel::left("stage_tools")
            .frame(theme::panel(palette.page))
            .show_separator_line(false)
            .resizable(false)
            .exact_width(TOOLS_WIDTH)
            .show(ctx, |ui| self.show_tools(ui, session))
            .inner;
        output.requests.extend(asked_in_the_tools);

        let footage_only = settings.room == ReviewRoom::FootageOnly;
        if !footage_only {
            let asked_at_the_bottom = egui::TopBottomPanel::bottom("stage_split")
                .frame(theme::panel(palette.sidebar))
                .show_separator_line(false)
                .show(ctx, |ui| show_export_bar(ui, session, settings))
                .inner;
            output.requests.extend(asked_at_the_bottom);
        }

        let settings_cards = SettingsCards {
            black_screens: self.black_screen_settings.for_this_frame(),
            scene_lengths: self.scene_length_settings.for_this_frame(),
        };
        // No margin above the strip: the stage above already ends in one.
        let strip_frame = theme::panel(palette.page).inner_margin(egui::Margin {
            top: 0.0,
            ..egui::Margin::same(theme::PANEL_MARGIN)
        });
        let asked_on_the_strip = egui::TopBottomPanel::bottom("stage_strip")
            .frame(strip_frame)
            .show_separator_line(false)
            .show(ctx, |ui| {
                let mut asked = EditorOutput::default();
                asked
                    .requests
                    .extend(show_notes(ui, session, &detection, settings_cards));
                asked.extend(self.show_film_strip(ui, session, playhead, STAGE_STRIP_HEIGHT));
                ui.horizontal(|ui| {
                    // The split at the playhead keeps its place at the right
                    // edge, and the scenes take the rest of the row. Undo is
                    // among the tools on the left.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        asked.player.extend(show_split_at_button(ui, playhead));
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            asked.extend(show_scene_buttons(ui, session));
                        });
                    });
                });
                if let Some(answer) = latest_review_event(ui, session) {
                    ui.add(egui::Label::new(answer).wrap());
                }
                asked.requests.extend(show_start_over_consent(ui, session));
                asked
            })
            .inner;
        output.extend(asked_on_the_strip);

        egui::CentralPanel::default()
            .frame(theme::panel(palette.page))
            .show(ctx, |ui| {
                let asked = if footage_only {
                    show_footage_bar(ui, session)
                } else {
                    show_stage_title(ui, session)
                };
                output.requests.extend(asked);
                card_filling(ui, show_player);
            });
        output
    }

    /// The tools that change the scenes, as a column of square buttons.
    fn show_tools(&mut self, ui: &mut egui::Ui, session: &ReviewSession) -> Vec<ReviewRequest> {
        let mut requests = Vec::new();
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = TOOL_GAP;
            ui.add_space(TOOL_GAP);
            let undo = theme::ToolButton::new(Icon::Undo, UNDO_NAME, ButtonKind::Plain);
            if ui
                .add_enabled(session.can_undo(), undo)
                .on_hover_text(theme::hint_with_key(ui.ctx(), UNDO_HINT, &UNDO_SHORTCUT))
                .on_disabled_hover_text(NOTHING_TO_UNDO_HINT)
                .clicked()
            {
                requests.push(ReviewRequest::Undo);
            }
            let can_merge = session.selection().span_to_merge().is_some();
            let merge = theme::ToolButton::new(Icon::Merge, MERGE_NAME, ButtonKind::Plain);
            if ui
                .add_enabled(can_merge, merge)
                .on_hover_text(theme::hint_with_key(ui.ctx(), MERGE_NAME, &MERGE_SHORTCUT))
                .on_disabled_hover_text(MERGE_DISABLED_HINT)
                .clicked()
            {
                requests.push(ReviewRequest::MergeSelected);
            }
            let any_chosen = session.selection().scenes().next().is_some();
            let drop = theme::ToolButton::new(Icon::Trash, DROP_NAME, ButtonKind::Plain);
            if ui
                .add_enabled(any_chosen, drop)
                .on_hover_text(theme::hint_with_key(
                    ui.ctx(),
                    DROP_NAME,
                    &DROP_OR_KEEP_SHORTCUT,
                ))
                .clicked()
            {
                requests.push(ReviewRequest::DropOrKeepSelected);
            }
            ui.separator();
            let rescan = theme::ToolButton::new(Icon::Refresh, RESCAN_NAME, ButtonKind::Quiet);
            if ui
                .add(rescan)
                .on_hover_text(SCAN_AGAIN_EXPLANATION)
                .clicked()
            {
                requests.push(ReviewRequest::Rescan(ScanKind::BlackScreens));
            }
            let detection = theme::ToolButton::new(Icon::Gear, DETECTION_NAME, ButtonKind::Quiet);
            if ui
                .add(detection)
                .on_hover_text(BLACK_SCREEN_SETTINGS_EXPLANATION)
                .clicked()
            {
                self.black_screen_settings = self.black_screen_settings.toggled();
            }
        });
        requests
    }
}

/// The name of the video and the way to give the footage the window. Split
/// later and Skip are in the bar along the bottom, beside the split.
fn show_stage_title(ui: &mut egui::Ui, session: &ReviewSession) -> Vec<ReviewRequest> {
    let mut requests = Vec::new();
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            requests.extend(show_room_button(
                ui,
                ReviewRoom::Everything,
                ButtonWords::Shown,
            ));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let name = file_name(session.video());
                let facts = format!(
                    "{}  ·  {}",
                    format_clock(session.plan().whole_video().seconds()),
                    counted(session.scene_count(), "scene")
                );
                ui.add(egui::Label::new(theme::semibold(&name, FILE_NAME_FONT_SIZE)).truncate())
                    .on_hover_text(format!("{name}\n{facts}\n{}", session.scan_summary()));
            });
        });
    });
    ui.add_space(TITLE_GAP);
    requests
}

/// Every scene as a button with its number, as on the strip, and its
/// length. The chosen ones are filled. A click chooses a scene and
/// moves the player to its start.
fn show_scene_buttons(ui: &mut egui::Ui, session: &ReviewSession) -> EditorOutput {
    let mut output = EditorOutput::default();
    egui::ScrollArea::horizontal()
        .id_salt("stage_scene_buttons")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (scene, layout) in with_scene_indices(&session.plan().scene_layouts()) {
                    let words = format!(
                        "{:02}  {}",
                        scene.number(),
                        format_clock(layout.span.seconds())
                    );
                    let button =
                        theme::SceneButton::new(words).chosen(session.selection().contains(scene));
                    let response = ui.add(button).on_hover_text(format!(
                        "{}, {} to {}",
                        scene_name(scene),
                        format_clock(layout.span.start),
                        format_clock(layout.span.end)
                    ));
                    if response.clicked() {
                        let kind = click_kind(ui.input(|input| input.modifiers));
                        output
                            .requests
                            .push(ReviewRequest::ClickScene { scene, kind });
                        output.player.push(PlayerAsk::GoTo(layout.span.start));
                    }
                }
            });
        });
    output
}
