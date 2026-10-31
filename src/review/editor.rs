//! The review screen laid out like a video editor:
//!
//! ```text
//! name of the video  ✓ 4 scenes found  last answer      Focus  Detection  Rescan  ⋯
//! ┌──────────────────────────────┐ ┌ Scenes ──────────────────── Merge selected ┐
//! │           picture            │ │ 01 [pic][pic][pic]   2:40  0:00-2:40     ⋯ │
//! │                              │ │ 02 [pic][pic][pic]   2:08  2:40-4:48     ⋯ │
//! │ 0:00.00 / 8:53 |< < (>) > >| 🔈 │ │ ...                                        │
//! └──────────────────────────────┘ └────────────────────────────────────────────┘
//! ┌──────────────────────── drag the top edge for a taller timeline ───────────┐
//! │ ruler, film strip with the scenes, cut markers, playhead                    │
//! │ Undo                 [Split at 0:42  S]                Add a split at [   ] │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! [Recycle the original]  [Save beside the video]   Skip  Split later  [Split into 4 files]
//! ```
//!
//! The edges between the picture and the scenes, and above the timeline,
//! can be dragged. In a very narrow window the three go one under the
//! other. The player itself belongs to the app, which draws it where
//! `show_player` is called and carries out the `PlayerAsk`s returned here.
//! Everything that changes the review is a `ReviewRequest`, so every rule
//! stays in `ReviewSession::apply`.

use super::*;
use crate::export::OriginalPolicy;
use crate::player::PlayerAsk;
use crate::settings::ReviewRoom;
use crate::split::SceneLocation;

/// Below this width, left for the preview and the scenes together, the
/// preview goes above the scenes instead of beside them.
const SIDE_BY_SIDE_WIDTH: f32 = 560.0;
/// How much of a narrow window's height the preview takes above the scenes.
const STACKED_PREVIEW_SHARE: f32 = 0.55;
/// The scenes card, as a share of the width beside the preview, and how far
/// it may be dragged.
const SCENES_CARD_SHARE: f32 = 0.45;
const SCENES_CARD_NARROWEST: f32 = 250.0;
const SCENES_PANEL: &str = "editor_scenes";
const SCENES_CARD_LARGEST_SHARE: f32 = 0.7;
const GAP: f32 = 12.0;
const CARD_PADDING: f32 = 12.0;
const CARD_HEADING_SIZE: f32 = 18.0;

const SCENES_HEADING: &str = "Scenes";
const SCENES_HINT: &str =
    "Drag a scene onto another to merge them, or Shift-click to choose several.";
const KEEP_ORIGINAL_CHOICE: &str = "Keep the original";
const RECYCLE_ORIGINAL_CHOICE: &str = "Recycle the original";
const BESIDE_VIDEO_CHOICE: &str = "Save beside the video";
const NEW_FOLDER_CHOICE: &str = "Save in a new folder";
const RESCAN_LABEL: &str = "Rescan";
const DETECTION_LABEL: &str = "Detection settings";
const MISSED_LABEL: &str = "Scenes missed?";
const MISSED_HINT: &str = "For videos that change scene without a black screen.";
const FOCUS_LABEL: &str = "Focus";
/// Narrower than this, the buttons of the title row show only their icons.
const TITLE_ROW_WITH_WORDS: f32 = 1000.0;
/// The most of the title row the name of the video takes.
const TITLE_NAME_SHARE: f32 = 0.45;
const FOCUS_HINT: &str = "Gives the whole window to the picture and the timeline. \
     Hides the list of videos, the scan and the choices about the split.";
const SHOW_EVERYTHING_LABEL: &str = "Show everything";
const SHOW_EVERYTHING_HINT: &str =
    "Brings back the list of videos, the scan and the choices about the split.";
const SPLIT_AT_HINT: &str =
    "Cuts the scene at the playhead, at the nearest point where the video can be cut.";
const PLAY_SCENE_LABEL: &str = "Play this scene";
const MERGE_LABEL: &str = "Merge selected";
const LONG_SCENE_FACT: &str = "Long, may hide a cut";
const LONG_SCENE_HINT: &str = "Scenes this long often hold a cut the scan missed. \
     Look at its three pictures, or play it.";
const ROW_MENU_HINT: &str = "Play, drop or keep this scene";
const LATER_OR_SKIP_HINT: &str = "Split this video later, or skip it";
const MORE_HINT: &str = "Scenes missed, scene lengths, the keys, and saving the list of scenes.";
const TIMELINE_GRIP_HINT: &str = "Drag to make the timeline taller or shorter";
const SCENES_GRIP_HINT: &str = "Drag to give the picture or the scenes more room";
/// Three dots, in a character every font has.
/// Rows in the table on a page that scrolls, before the table scrolls too.
const ROWS_ON_A_PAGE: f32 = 6.0;
/// The space around the pictures of a row, and between them.
const ROW_PICTURE_MARGIN: f32 = 6.0;
const ROW_PICTURE_GAP: f32 = 4.0;
const ROW_INSET: f32 = 8.0;
const NUMBER_COLUMN: f32 = 30.0;
/// The length, the start and end, and what is worth knowing of a scene.
const FACTS_COLUMN: f32 = 118.0;
const FACTS_GAP: f32 = 12.0;
const MENU_COLUMN: f32 = 32.0;
/// Narrower than this, the pictures go under the number and the length.
const NARROWEST_PICTURE_BESIDE: f32 = 76.0;
/// The scenes' pictures are decoded this wide, so they are never drawn wider.
const WIDEST_ROW_PICTURE: f32 = MAX_PREVIEW_WIDTH;
/// The line above the pictures, when they go under it.
const ROW_FACTS_LINE: f32 = 26.0;
const FACT_SIZE: f32 = 12.0;

const RULER_HEIGHT: f32 = 22.0;
/// The film strip's height when it is not in a timeline that can be made
/// taller, and the least it shrinks to in one that can.
pub(super) const STRIP_HEIGHT: f32 = 50.0;
/// The timeline's own height, which the person can drag, and its limits:
/// never so short that the strip goes, never more than half the window.
const TIMELINE_PANEL: &str = "editor_timeline";
const TIMELINE_HEIGHT: f32 = 190.0;
const SHORTEST_TIMELINE: f32 = 170.0;
/// The picture and the scenes keep at least this much height.
const LEAST_ROOM_ABOVE_THE_TIMELINE: f32 = 240.0;
const TALLEST_STRIP: f32 = 200.0;
/// What the timeline card holds besides the strip, before it is measured:
/// the ruler, the cut times and the row of controls.
const AROUND_THE_STRIP: f32 = RULER_HEIGHT + MARKER_LABEL_HEIGHT + theme::CONTROL_HEIGHT + 16.0;
const GRIP_LENGTH: f32 = 44.0;
const GRIP_THICKNESS: f32 = 4.0;
/// How far outside its panel the pointer still counts as on a grip's edge.
const GRIP_REACH: f32 = 6.0;
/// The thin strip of the whole video under a close-up of chosen scenes.
const OVERVIEW_HEIGHT: f32 = 22.0;
/// The band of a scene's own colour along the bottom of its block.
const SCENE_BAND_HEIGHT: f32 = 5.0;
const OVERVIEW_BAND_HEIGHT: f32 = 3.0;
const CHOSEN_EDGE_WIDTH: f32 = 3.0;
const MARKER_LABEL_HEIGHT: f32 = 18.0;
const LEAST_PIXELS_BETWEEN_TICK_LABELS: f32 = 64.0;
/// Seconds between the labelled ticks of the ruler: the first that leaves
/// room between their labels.
const TICK_STEPS: [f64; 12] = [
    1.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0,
];
const MINOR_TICKS_PER_STEP: f64 = 5.0;
/// Pictures on the strip are taken at whole half seconds, so a strip drawn
/// a little wider asks for pictures it already has.
const STRIP_PICTURE_STEP: f64 = 0.5;
const MOST_PICTURES_PER_SCENE: usize = 12;
const CUT_MARKER_WIDTH: f32 = 2.0;
/// The width and height of the triangle on top of the playhead.
const PLAYHEAD_HEAD: egui::Vec2 = egui::vec2(10.0, 8.0);
const SCENE_LABEL_ROUNDING: f32 = 4.0;
/// The width the export choices, the menu and the split button need side
/// by side, as measured in the smallest window. Narrower, the choices
/// take a row of their own.
const EXPORT_BAR_ONE_ROW_WIDTH: f32 = 535.0;
/// Narrower than this, skip and split later show only their icons.
const EXPORT_BAR_WITH_WORDS: f32 = 1000.0;
/// Room for "Add a split at", its box and its button.
const TYPED_SPLIT_FIELD_WIDTH: f32 = 64.0;
const TYPED_SPLIT_LABEL: &str = "Add a split at";
const TYPED_SPLIT_HINT: &str = "Type a time and press Enter to add a split there.";
const ADD_SPLIT_LABEL: &str = "Add a split at the time in the box";
/// Room for "Split at 1:23:45" and its icon.
const SPLIT_AT_WIDTH: f32 = 160.0;
const SPLIT_AT_LABEL: &str = "Split at";
/// The share of a picture's width its height is.
const PICTURE_ASPECT: f32 = 9.0 / 16.0;

/// What the editor asks for in a frame: changes to the review, and things
/// for the player to do.
#[derive(Debug, Default)]
pub struct EditorOutput {
    pub requests: Vec<ReviewRequest>,
    pub player: Vec<PlayerAsk>,
}

impl EditorOutput {
    pub(super) fn extend(&mut self, other: EditorOutput) {
        self.requests.extend(other.requests);
        self.player.extend(other.player);
    }
}

/// What the scenes card is drawn from.
#[derive(Clone, Copy)]
struct ScenesShown<'a> {
    session: &'a ReviewSession,
    detection: &'a DetectionInUse,
    settings_cards: SettingsCards,
}

impl ReviewScreen {
    /// Draws the review as a video editor. `show_player` draws the player
    /// into the preview card.
    pub fn show_editor(
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

        // With only the footage on screen, the title, the scan and the
        // choices about the split give way to one slim bar.
        let footage_only = settings.room == ReviewRoom::FootageOnly;
        let asked_at_the_top = egui::TopBottomPanel::top("editor_title")
            .frame(theme::header_panel(ctx))
            .show_separator_line(false)
            .show(ctx, |ui| {
                if footage_only {
                    show_footage_bar(ui, session)
                } else {
                    self.show_title_and_status(ui, session, &detection)
                }
            })
            .inner;
        output.requests.extend(asked_at_the_top);

        if !footage_only {
            let asked_at_the_bottom = egui::TopBottomPanel::bottom("editor_export")
                .frame(theme::panel(palette.sidebar))
                .show_separator_line(false)
                .show(ctx, |ui| show_export_bar(ui, session, settings))
                .inner;
            output.requests.extend(asked_at_the_bottom);
        }

        let shown = ScenesShown {
            session,
            detection: &detection,
            settings_cards: SettingsCards {
                black_screens: self.black_screen_settings.for_this_frame(),
                scene_lengths: self.scene_length_settings.for_this_frame(),
            },
        };
        let wide_enough = ctx.available_rect().width() >= SIDE_BY_SIDE_WIDTH;
        if wide_enough {
            let tallest = (ctx.available_rect().height() - LEAST_ROOM_ABOVE_THE_TIMELINE)
                .max(SHORTEST_TIMELINE);
            // The top edge of the timeline can be dragged, for larger
            // pictures in the strip. egui keeps the height it was dragged to.
            let timeline_id = egui::Id::new(TIMELINE_PANEL);
            let panel_height = egui::panel::PanelState::load(ctx, timeline_id)
                .map_or(TIMELINE_HEIGHT, |panel| panel.rect.height())
                .clamp(SHORTEST_TIMELINE, tallest);
            let card_inside = panel_height - theme::PANEL_MARGIN - 2.0 * (CARD_PADDING + 1.0);
            // The strip takes what the rest of the card leaves. That rest is
            // measured, not reckoned: a card taller than the panel would make
            // the panel grow, and the strip with it, a little every frame.
            let around_id = timeline_id.with("around_the_strip");
            let around = ctx
                .data(|data| data.get_temp(around_id))
                .unwrap_or(AROUND_THE_STRIP);
            let strip = strip_height_within(card_inside, around);
            let timeline = egui::TopBottomPanel::bottom(timeline_id)
                .frame(theme::panel(palette.page).inner_margin(egui::Margin {
                    top: 0.0,
                    ..egui::Margin::same(theme::PANEL_MARGIN)
                }))
                .show_separator_line(false)
                .resizable(true)
                .default_height(TIMELINE_HEIGHT)
                .height_range(SHORTEST_TIMELINE..=tallest)
                .show(ctx, |ui| {
                    card(ui, |ui| {
                        let asked = self.show_timeline_card(ui, session, playhead, strip);
                        let measured = ui.min_rect().height() - strip;
                        ui.data_mut(|data| data.insert_temp(around_id, measured));
                        asked
                    })
                });
            paint_grip(ctx, timeline.response.rect, GripEdge::Top);
            output.extend(timeline.inner);
            let asked_in_the_middle = self.show_preview_beside_the_scenes(ctx, shown, show_player);
            output.requests.extend(asked_in_the_middle);
        } else {
            output.extend(self.show_review_as_one_page(ctx, shown, playhead, show_player));
        }
        output
    }

    /// One row: the name of the video, what the scan found, and the ways
    /// to look again. The slower scan for missed scenes opens under it.
    fn show_title_and_status(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        detection: &DetectionInUse,
    ) -> Vec<ReviewRequest> {
        let palette = theme::palette(ui.ctx());
        let mut requests = Vec::new();
        ui.horizontal(|ui| {
            // Short of room, the buttons keep their icons and say the rest
            // on hover, so the name of the video keeps some of the row.
            let words = ButtonWords::for_room(ui.available_width(), TITLE_ROW_WITH_WORDS);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                requests.extend(self.show_scan_buttons(ui, session, words));
                requests.extend(show_room_button(ui, ReviewRoom::Everything, words));
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let name = file_name(session.video());
                    let name_room = egui::vec2(
                        ui.available_width() * TITLE_NAME_SHARE,
                        theme::CONTROL_HEIGHT,
                    );
                    ui.allocate_ui(name_room, |ui| {
                        let title = theme::semibold(&name, FILE_NAME_FONT_SIZE);
                        ui.add(egui::Label::new(title).truncate())
                            .on_hover_text(&name);
                    });
                    ui.add_space(GAP);
                    let tick_size = egui::vec2(18.0, 18.0);
                    let (tick, _) = ui.allocate_exact_size(tick_size, egui::Sense::hover());
                    ui.painter()
                        .circle_filled(tick.center(), 9.0, palette.primary_fill);
                    Icon::Check.paint(ui.painter(), tick.shrink(4.0), palette.on_primary);
                    let found = format!("{} found", counted(session.scene_count(), "scene"));
                    let found = theme::semibold(found, 14.0).color(palette.primary);
                    ui.add(egui::Label::new(found).truncate())
                        .on_hover_text(format!(
                            "{}\n{}",
                            session.scan_summary(),
                            session.scan_details()
                        ));
                    if let Some(event) = latest_review_event(ui, session) {
                        ui.add_space(GAP);
                        let whole = event.text().to_owned();
                        ui.add(egui::Label::new(event).truncate())
                            .on_hover_text(whole);
                    }
                });
            });
        });
        if self.missed_scenes_open {
            ui.add_space(6.0);
            card(ui, |ui| {
                requests.extend(show_hard_cut_scan_controls(ui, detection.detection));
                requests.extend(show_settings_scope(ui, session.video(), &detection.scope));
            });
        }
        requests.extend(show_start_over_consent(ui, session));
        requests
    }

    /// Rescan, the settings for black screens, the slower scan for missed
    /// scenes, and the rest in a menu. Laid out from the right.
    fn show_scan_buttons(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        words: ButtonWords,
    ) -> Vec<ReviewRequest> {
        let mut requests = Vec::new();
        theme::more_menu(ui, MORE_HINT, |ui| {
            if ui.button(MISSED_LABEL).on_hover_text(MISSED_HINT).clicked() {
                self.missed_scenes_open = !self.missed_scenes_open;
                ui.close_menu();
            }
            if ui.button(SCENE_LENGTHS_LABEL).clicked() {
                self.scene_length_settings = self.scene_length_settings.toggled();
                ui.close_menu();
            }
            if session.chapter_split_count() > 0 && ui.button(LEAVE_OUT_CHAPTERS_LABEL).clicked() {
                requests.push(ReviewRequest::LeaveOutChapterSplits);
                ui.close_menu();
            }
            if ui.button(KEYS_HEADING).clicked() {
                self.toggle_keys_list();
                ui.close_menu();
            }
            ui.separator();
            if let Some(reset) = show_reset_button(ui, session) {
                requests.push(reset);
                ui.close_menu();
            }
            requests.extend(show_save_scene_list_menu(ui, session.video()));
        });
        let rescan = add_worded_button(ui, Icon::Refresh, RESCAN_LABEL, words)
            .on_hover_text(SCAN_AGAIN_EXPLANATION);
        if rescan.clicked() {
            requests.push(ReviewRequest::Rescan(ScanKind::BlackScreens));
        }
        let detection_clicked = add_worded_button(ui, Icon::Gear, DETECTION_LABEL, words)
            .on_hover_text(BLACK_SCREEN_SETTINGS_EXPLANATION)
            .clicked();
        if detection_clicked {
            self.black_screen_settings = self.black_screen_settings.toggled();
        }
        requests
    }

    fn show_preview_beside_the_scenes(
        &mut self,
        ctx: &egui::Context,
        shown: ScenesShown,
        show_player: &mut dyn FnMut(&mut egui::Ui),
    ) -> Vec<ReviewRequest> {
        let palette = theme::palette(ctx);
        let room = ctx.available_rect().width();
        let widest = (room * SCENES_CARD_LARGEST_SHARE).max(SCENES_CARD_NARROWEST);
        // The edge between the preview and the scenes can be dragged: to
        // the left for more room for the scenes, to the right for a larger
        // picture. egui keeps the width it was dragged to.
        let scenes_id = egui::Id::new(SCENES_PANEL);
        keep_share_of_width(ctx, scenes_id, room);
        let scenes = egui::SidePanel::right(scenes_id)
            .frame(theme::panel(palette.page))
            .show_separator_line(false)
            .resizable(true)
            .default_width(room * SCENES_CARD_SHARE)
            .width_range(SCENES_CARD_NARROWEST..=widest)
            .show(ctx, |ui| {
                card_filling(ui, |ui| self.show_scenes_card(ui, shown, TableRoom::Fill))
            });
        paint_grip(ctx, scenes.response.rect, GripEdge::Left);
        let asked = scenes.inner;
        egui::CentralPanel::default()
            .frame(theme::panel(palette.page))
            .show(ctx, |ui| card_filling(ui, show_player));
        asked
    }

    /// In a narrow window: the preview, the scenes and the timeline one
    /// under the other, scrolling together.
    fn show_review_as_one_page(
        &mut self,
        ctx: &egui::Context,
        shown: ScenesShown,
        playhead: Option<f64>,
        show_player: &mut dyn FnMut(&mut egui::Ui),
    ) -> EditorOutput {
        let palette = theme::palette(ctx);
        egui::CentralPanel::default()
            .frame(theme::panel(palette.page))
            .show(ctx, |ui| {
                let preview_room = egui::vec2(
                    ui.available_width(),
                    ui.available_height() * STACKED_PREVIEW_SHARE,
                );
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut output = EditorOutput::default();
                        ui.allocate_ui(preview_room, |ui| card_filling(ui, show_player));
                        ui.add_space(GAP);
                        let session = shown.session;
                        output.extend(card(ui, |ui| {
                            self.show_timeline_card(ui, session, playhead, STRIP_HEIGHT)
                        }));
                        ui.add_space(GAP);
                        output.requests.extend(card(ui, |ui| {
                            self.show_scenes_card(ui, shown, TableRoom::Rows(ROWS_ON_A_PAGE))
                        }));
                        output
                    })
                    .inner
            })
            .inner
    }

    /// The heading, the notes and settings the scan asks for, the table of
    /// scenes, and the merge that works on the chosen ones.
    fn show_scenes_card(
        &mut self,
        ui: &mut egui::Ui,
        shown: ScenesShown,
        room: TableRoom,
    ) -> Vec<ReviewRequest> {
        let ScenesShown {
            session,
            detection,
            settings_cards,
        } = shown;
        let mut requests = Vec::new();
        requests.extend(show_scenes_heading(ui, session));
        let row_width = ui.available_width();
        let table_height = match room {
            TableRoom::Fill => ui.available_height(),
            TableRoom::Rows(rows) => RowShape::for_width(row_width).height * rows,
        };
        egui::ScrollArea::vertical()
            .id_salt("editor_scene_table")
            // A drag moves a scene onto another; the wheel scrolls.
            .drag_to_scroll(false)
            .max_height(table_height)
            .min_scrolled_height(table_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                requests.extend(show_notes(ui, session, detection, settings_cards));
                if self.keys_list == KeysList::Shown && show_keys_card(ui).hide_asked {
                    self.toggle_keys_list();
                }
                let shape = RowShape::for_width(ui.available_width());
                requests.extend(self.show_scene_table(ui, session, shape));
            });
        requests
    }

    fn show_scene_table(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        shape: RowShape,
    ) -> Vec<ReviewRequest> {
        let palette = theme::palette(ui.ctx());
        let mut requests = Vec::new();
        ui.spacing_mut().item_spacing.y = 4.0;
        let layouts = session.plan().scene_layouts();
        let fates = card_fates(&session.scene_fates());
        // Where each row was last frame, to know which one a dragged row
        // is over before this frame's rows are laid out.
        let rows_last_frame = std::mem::take(&mut self.scene_cards);
        let pointer = ui.ctx().pointer_interact_pos();
        let drop_target = self
            .dragged_scene
            .and(pointer)
            .and_then(|pointer| rows_last_frame.iter().position(|row| row.contains(pointer)))
            .map(SceneIndex::new);
        let to_merge = self
            .dragged_scene
            .zip(drop_target)
            .filter(|(source, target)| source != target)
            .map(|(source, target)| span_between(source, target));
        for ((scene, layout), &fate) in with_scene_indices(&layouts).zip(&fates) {
            // The row takes its click and drag before the pictures and the
            // menu on it are added, so they still take theirs.
            let (row, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), shape.height),
                egui::Sense::click_and_drag(),
            );
            self.scene_cards.push(row);
            if response.clicked() {
                let kind = click_kind(ui.input(|input| input.modifiers));
                requests.push(ReviewRequest::ClickScene { scene, kind });
            }
            if response.drag_started() {
                self.dragged_scene = Some(scene);
            }
            let rounding = palette.control_rounding();
            let will_merge = to_merge.as_ref().is_some_and(|span| span.contains(&scene));
            if will_merge {
                let color = palette.scene_color(
                    to_merge
                        .as_ref()
                        .map_or(scene, |span| *span.start())
                        .position(),
                );
                let fill = palette.tinted_card(color, CardTint::WillMerge);
                ui.painter()
                    .rect(row, rounding, fill, egui::Stroke::new(2.0_f32, color));
            } else if session.selection().contains(scene) {
                let edge = egui::Stroke::new(1.0_f32, palette.primary);
                ui.painter().rect(row, rounding, palette.primary_tint, edge);
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(row, rounding, palette.control_hovered_fill);
            }
            if session.focus().is_some_and(|focus| focus.scene() == scene) {
                paint_focus_ring(ui.painter(), row, rounding);
            }
            if self.scene_to_scroll_to == Some(scene) {
                ui.scroll_to_rect(row, None);
            }
            let color = if fate.is_dropped() {
                palette.dropped
            } else {
                palette.scene_color(scene.position())
            };
            let scene_row = SceneCard {
                scene,
                layout,
                fate,
            };
            let asked = ui
                .allocate_new_ui(egui::UiBuilder::new().max_rect(row), |ui| {
                    self.show_scene_row(ui, session, scene_row, color, shape)
                })
                .inner;
            requests.extend(asked);
        }
        self.scene_to_scroll_to = None;
        if let Some(source) = self.dragged_scene {
            let released = ui.input(|input| input.pointer.any_released());
            if released {
                requests.extend(to_merge.clone().map(ReviewRequest::MergeScenes));
                self.dragged_scene = None;
            } else if let Some(pointer) = pointer {
                let hint = match &to_merge {
                    Some(span) => merge_description(span),
                    None => format!("Drop {} on another scene to merge them", scene_name(source)),
                };
                paint_drag_hint(ui.ctx(), pointer, &hint);
            }
        }
        requests
    }

    /// One scene as a row: its number, three pictures (near its start, in
    /// the middle and near its end, so two scenes run together show), its
    /// length, start and end with what is worth knowing of it, and a menu.
    /// In a narrow table the pictures go under the rest, as wide as the row.
    fn show_scene_row(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        scene_row: SceneCard,
        color: egui::Color32,
        shape: RowShape,
    ) -> Vec<ReviewRequest> {
        let SceneCard {
            scene,
            layout,
            fate,
        } = scene_row;
        let mut requests = Vec::new();
        let row = ui
            .max_rect()
            .shrink2(egui::vec2(ROW_INSET, ROW_PICTURE_MARGIN));
        let (line, pictures_left) = match shape.pictures {
            PicturesPlace::Beside => (row, row.left() + NUMBER_COLUMN),
            PicturesPlace::Under => (
                egui::Rect::from_min_size(row.min, egui::vec2(row.width(), ROW_FACTS_LINE)),
                row.left(),
            ),
        };
        let number = theme::semibold(format!("{:02}", scene.number()), 14.0).color(color);
        let number_place = egui::Rect::from_min_size(
            line.min,
            egui::vec2(NUMBER_COLUMN, ROW_FACTS_LINE.min(line.height())),
        );
        ui.put(number_place, egui::Label::new(number));

        let pictures_top = match shape.pictures {
            PicturesPlace::Beside => row.top(),
            PicturesPlace::Under => line.bottom() + ROW_PICTURE_MARGIN / 2.0,
        };
        for (place, time) in scene_picture_times(layout.span).into_iter().enumerate() {
            let left = pictures_left + (shape.picture.x + ROW_PICTURE_GAP) * place as f32;
            let at = egui::Rect::from_min_size(egui::pos2(left, pictures_top), shape.picture);
            let shown = ui
                .allocate_new_ui(egui::UiBuilder::new().max_rect(at), |ui| {
                    let mark = PictureMark::Plain;
                    self.show_preview(ui, session.video(), time, shape.picture, mark)
                })
                .inner;
            if shown {
                requests.push(ReviewRequest::PlayFrom(time));
            }
        }

        let menu_place = egui::Rect::from_min_size(
            egui::pos2(line.right() - MENU_COLUMN, line.top()),
            egui::vec2(MENU_COLUMN, ROW_FACTS_LINE.min(line.height())),
        );
        let asked = ui
            .allocate_new_ui(egui::UiBuilder::new().max_rect(menu_place), |ui| {
                show_row_menu(ui, scene, layout.span, fate)
            })
            .inner;
        requests.extend(asked);

        let facts_left = match shape.pictures {
            PicturesPlace::Beside => menu_place.left() - FACTS_COLUMN,
            PicturesPlace::Under => row.left() + NUMBER_COLUMN,
        };
        let facts_place = egui::Rect::from_min_max(
            egui::pos2(facts_left, line.top()),
            egui::pos2(menu_place.left() - ROW_PICTURE_GAP, line.bottom()),
        );
        let title = session.chapter_title(layout.span).map(title_for_card);
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(facts_place), |ui| {
            show_scene_facts(ui, layout, fate, title, shape.pictures);
        });
        requests
    }

    /// The heading with the box for a split at a typed time, the film
    /// strip, and the row of controls that act at the playhead.
    fn show_timeline_card(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        playhead: Option<f64>,
        strip_height: f32,
    ) -> EditorOutput {
        // No heading row: the strip says what it is, and the height goes to
        // the pictures in it.
        let mut output = self.show_film_strip(ui, session, playhead, strip_height);
        ui.add_space(6.0);
        let (row, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::CONTROL_HEIGHT),
            egui::Sense::hover(),
        );
        let left_to_right = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_new_ui(
            egui::UiBuilder::new().max_rect(row).layout(left_to_right),
            |ui| {
                output.requests.extend(show_undo_button(ui, session));
            },
        );
        // The split is the one thing this row is for, so it is in the middle,
        // under the strip it cuts.
        let split_width = SPLIT_AT_WIDTH.min(row.width());
        let split_place = egui::Rect::from_center_size(
            row.center(),
            egui::vec2(split_width, theme::CONTROL_HEIGHT),
        );
        let centred = egui::Layout::top_down(egui::Align::Center);
        ui.allocate_new_ui(
            egui::UiBuilder::new().max_rect(split_place).layout(centred),
            |ui| {
                output.player.extend(show_split_at_button(ui, playhead));
            },
        );
        // At the right end, as undo is at the left, so the row is even.
        let typed_place = egui::Rect::from_min_max(
            egui::pos2(split_place.right(), row.top()),
            row.right_bottom(),
        );
        let right_to_left = egui::Layout::right_to_left(egui::Align::Center);
        ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(typed_place)
                .layout(right_to_left),
            |ui| {
                output
                    .requests
                    .extend(show_typed_split_controls(ui, session));
            },
        );
        show_scene_list_note(ui, session);
        output
    }

    /// The film strip. With scenes chosen it shows only them, from the
    /// start of the first to the end of the last, so a split inside them
    /// can be placed closely, and a thin strip of the whole video under it
    /// keeps the rest in reach. With none chosen it shows the whole video.
    pub(super) fn show_film_strip(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        playhead: Option<f64>,
        strip_height: f32,
    ) -> EditorOutput {
        let whole_video = session.plan().whole_video();
        let Some(chosen) = chosen_span(session) else {
            let whole = StripView::whole(whole_video, strip_height);
            return self.show_strip(ui, session, playhead, whole);
        };
        let close_up = StripView::close_up(chosen, strip_height);
        let mut output = self.show_strip(ui, session, playhead, close_up);
        ui.add_space(4.0);
        output.extend(self.show_strip(
            ui,
            session,
            playhead,
            StripView::overview(whole_video, chosen),
        ));
        output
    }

    /// One strip of the scenes in `view`: the ruler, each scene as its
    /// pictures with its number and name, an amber marker at every cut, and
    /// the playhead. A click moves the playhead there; on a strip that
    /// shows the whole video it also chooses the scene clicked.
    fn show_strip(
        &mut self,
        ui: &mut egui::Ui,
        session: &ReviewSession,
        playhead: Option<f64>,
        view: StripView,
    ) -> EditorOutput {
        let palette = theme::palette(ui.ctx());
        let mut output = EditorOutput::default();
        let plan = session.plan();
        let layouts = plan.scene_layouts();
        let fates = plan.scene_fates();
        let (area, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), view.total_height()),
            egui::Sense::click_and_drag(),
        );
        let ruler =
            egui::Rect::from_min_size(area.min, egui::vec2(area.width(), view.ruler_height()));
        let strip = egui::Rect::from_min_size(
            ruler.left_bottom(),
            egui::vec2(area.width(), view.strip_height),
        );
        let blocks = timeline_blocks(
            layouts.iter().map(|layout| layout.span),
            view.shown,
            strip.x_range(),
        );

        let painter = ui.painter().with_clip_rect(area.intersect(ui.clip_rect()));
        if view.kind != StripKind::Overview {
            paint_ruler(&painter, ruler, view.shown, palette);
        }
        painter.rect_filled(strip, palette.control_rounding(), palette.input_fill);
        for (((scene, layout), &across), &fate) in
            with_scene_indices(&layouts).zip(&blocks).zip(&fates)
        {
            let overlaps_view =
                layout.span.end > view.shown.start && layout.span.start < view.shown.end;
            if !overlaps_view {
                continue;
            }
            let block =
                egui::Rect::from_x_y_ranges(across, strip.y_range()).shrink2(egui::vec2(1.0, 0.0));
            self.paint_scene_pictures(ui.ctx(), &painter, session.video(), block, layout.span);
            if fate == SceneFate::Drop {
                painter.rect_filled(block, 0.0, palette.backdrop);
            }
            // Each scene keeps its colour here as in the table: a band
            // along the bottom of its block and its edge.
            let color = if fate == SceneFate::Drop {
                palette.dropped
            } else {
                palette.scene_color(scene.position())
            };
            let chosen = session.selection().contains(scene);
            let band_height = match view.kind {
                StripKind::Overview => OVERVIEW_BAND_HEIGHT,
                StripKind::Whole | StripKind::CloseUp => SCENE_BAND_HEIGHT,
            };
            let band = egui::Rect::from_min_max(
                egui::pos2(block.left(), block.bottom() - band_height),
                block.right_bottom(),
            );
            painter.rect_filled(band, 0.0, color);
            let edge_width = if chosen && view.kind != StripKind::CloseUp {
                CHOSEN_EDGE_WIDTH
            } else {
                1.5
            };
            painter.rect_stroke(
                block,
                SCENE_LABEL_ROUNDING,
                egui::Stroke::new(edge_width, color),
            );
            if view.kind != StripKind::Overview {
                paint_scene_label(&painter, block, scene, color, palette);
            }
        }
        for layout in layouts.iter().skip(1) {
            let cut = layout.span.start;
            if cut <= view.shown.start || cut >= view.shown.end {
                continue;
            }
            let cut_x = x_at_time(cut, view.shown, strip.x_range());
            match view.kind {
                StripKind::Overview => {
                    let line = egui::Stroke::new(1.0_f32, palette.warning_text);
                    painter.vline(cut_x, strip.y_range(), line);
                }
                StripKind::Whole | StripKind::CloseUp => {
                    paint_cut_marker(&painter, cut_x, strip, cut, palette.warning_text);
                }
            }
        }
        if let Some(framed) = view.framed {
            paint_frame_around(&painter, strip, view.shown, framed, palette);
        }
        if let Some(playhead) =
            playhead.filter(|&time| time >= view.shown.start && time <= view.shown.end)
        {
            let x = x_at_time(playhead, view.shown, strip.x_range());
            let line = egui::Stroke::new(PLAYHEAD_WIDTH, palette.text);
            // The head sits under the times of the ruler, so it never
            // covers one.
            let head_top = ruler.bottom() - PLAYHEAD_HEAD.y;
            painter.vline(x, egui::Rangef::new(head_top, strip.bottom()), line);
            if view.kind != StripKind::Overview {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(x - PLAYHEAD_HEAD.x / 2.0, head_top),
                        egui::pos2(x + PLAYHEAD_HEAD.x / 2.0, head_top),
                        egui::pos2(x, ruler.bottom()),
                    ],
                    palette.text,
                    egui::Stroke::NONE,
                ));
            }
        }

        // Dragging anywhere on the strip or its ruler moves the playhead
        // with the pointer, as in any editor.
        // Only a move asks again, so a drag held still asks nothing.
        if response.drag_stopped() {
            output.player.push(PlayerAsk::ScrubEnded);
            return output;
        }
        if response.dragged() {
            let moved = response.drag_started() || response.drag_delta().x != 0.0;
            if let (true, Some(pointer)) = (moved, response.interact_pointer_pos()) {
                let time = time_at_x(pointer.x, view.shown, strip.x_range());
                output.player.push(PlayerAsk::ScrubTo(time));
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            return output;
        }

        let Some(pointer) = response.hover_pos() else {
            return output;
        };
        let Some(scene) = scene_at_x(&blocks, pointer.x) else {
            return output;
        };
        let (Some(layout), Some(&fate)) =
            (layouts.get(scene.position()), fates.get(scene.position()))
        else {
            return output;
        };
        let clicked = response.clicked();
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text_at_pointer(scene_description(
                scene,
                layout,
                session.chapter_title(layout.span),
                fate,
            ));
        if clicked {
            let time = time_at_x(pointer.x, view.shown, strip.x_range());
            output.player.push(PlayerAsk::GoTo(time));
            if view.kind != StripKind::CloseUp {
                // Shift and Ctrl choose more than one, as in the scene list.
                let kind = click_kind(ui.input(|input| input.modifiers));
                output
                    .requests
                    .push(ReviewRequest::ClickScene { scene, kind });
                self.scene_to_scroll_to = Some(scene);
            }
        }
        output
    }

    /// Pictures of a scene side by side across its block. Those not loaded
    /// yet are asked for while the block is on screen.
    fn paint_scene_pictures(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        video: &Arc<Path>,
        block: egui::Rect,
        span: TimeSpan,
    ) {
        let picture_width = block.height() / PICTURE_ASPECT;
        let count =
            ((block.width() / picture_width).ceil() as usize).clamp(1, MOST_PICTURES_PER_SCENE);
        let clipped = painter.with_clip_rect(block.intersect(painter.clip_rect()));
        let tile_width = block.width() / count as f32;
        for place in 0..count {
            let share = (place as f64 + 0.5) / count as f64;
            let wanted = span.start + span.seconds() * share;
            let time = (wanted / STRIP_PICTURE_STEP).round() * STRIP_PICTURE_STEP;
            let tile = egui::Rect::from_min_size(
                egui::pos2(block.left() + tile_width * place as f32, block.top()),
                egui::vec2(tile_width + 1.0, block.height()),
            );
            let key = preview_key(time, self.preview_pixels);
            match self.previews.get(&key) {
                Some(Preview::Ready(texture)) => {
                    clipped.image(
                        texture.id(),
                        tile,
                        picture_crop(tile.size()),
                        egui::Color32::WHITE,
                    );
                }
                Some(Preview::Loading | Preview::Failed(_)) => {}
                None if painter.clip_rect().intersects(tile) => {
                    self.start_loading_preview(ctx, video, key, time);
                }
                None => {}
            }
        }
    }
}

/// What a strip shows and how.
#[derive(Debug, Clone, Copy, PartialEq)]
struct StripView {
    kind: StripKind,
    /// The stretch of the video across the strip.
    shown: TimeSpan,
    strip_height: f32,
    /// On the overview, the stretch the close-up above it shows.
    framed: Option<TimeSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum StripKind {
    /// The whole video, with nothing chosen.
    Whole,
    /// Only the chosen scenes.
    CloseUp,
    /// The whole video, thin, under a close-up.
    Overview,
}

impl StripView {
    fn whole(video: TimeSpan, strip_height: f32) -> Self {
        Self {
            kind: StripKind::Whole,
            shown: video,
            strip_height,
            framed: None,
        }
    }

    fn close_up(chosen: TimeSpan, strip_height: f32) -> Self {
        Self {
            kind: StripKind::CloseUp,
            shown: chosen,
            strip_height,
            framed: None,
        }
    }

    fn overview(video: TimeSpan, chosen: TimeSpan) -> Self {
        Self {
            kind: StripKind::Overview,
            shown: video,
            strip_height: OVERVIEW_HEIGHT,
            framed: Some(chosen),
        }
    }

    fn ruler_height(self) -> f32 {
        match self.kind {
            StripKind::Overview => 0.0,
            StripKind::Whole | StripKind::CloseUp => RULER_HEIGHT,
        }
    }

    fn total_height(self) -> f32 {
        let marker_labels = match self.kind {
            StripKind::Overview => 0.0,
            StripKind::Whole | StripKind::CloseUp => MARKER_LABEL_HEIGHT,
        };
        self.ruler_height() + self.strip_height + marker_labels
    }
}

/// From the start of the first chosen scene to the end of the last, or
/// `None` when no scene is chosen.
fn chosen_span(session: &ReviewSession) -> Option<TimeSpan> {
    let layouts = session.plan().scene_layouts();
    let mut chosen = session
        .selection()
        .scenes()
        .filter_map(|scene| layouts.get(scene.position()).map(|layout| layout.span));
    let first = chosen.next()?;
    Some(chosen.fold(first, |span, next| TimeSpan {
        start: span.start.min(next.start),
        end: span.end.max(next.end),
    }))
}

/// Dims the overview outside the stretch the close-up shows, and frames it.
fn paint_frame_around(
    painter: &egui::Painter,
    strip: egui::Rect,
    shown: TimeSpan,
    framed: TimeSpan,
    palette: &theme::Palette,
) {
    let left = x_at_time(framed.start, shown, strip.x_range());
    let right = x_at_time(framed.end, shown, strip.x_range());
    let before = egui::Rect::from_x_y_ranges(strip.left()..=left, strip.y_range());
    let after = egui::Rect::from_x_y_ranges(right..=strip.right(), strip.y_range());
    painter.rect_filled(before, 0.0, palette.backdrop);
    painter.rect_filled(after, 0.0, palette.backdrop);
    let frame = egui::Rect::from_x_y_ranges(left..=right, strip.y_range());
    painter.rect_stroke(
        frame,
        SCENE_LABEL_ROUNDING,
        egui::Stroke::new(2.0_f32, palette.primary),
    );
}

/// How much height the table of scenes may take.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TableRoom {
    /// All the card has: the rows grow to fill it, and the merge sits at
    /// the bottom of the card.
    Fill,
    /// A page that scrolls has no height to fill: the table shows a few
    /// rows and scrolls on its own.
    Rows(f32),
}

/// Where a row's pictures go: beside its number and facts, or under them
/// when the table is too narrow for pictures worth looking at.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PicturesPlace {
    Beside,
    Under,
}

/// How tall a row is and how large its three pictures are. The pictures
/// take all the width the rest of the row leaves them, so a wider table
/// shows larger pictures, up to the size they are decoded at.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RowShape {
    pictures: PicturesPlace,
    picture: egui::Vec2,
    height: f32,
}

impl RowShape {
    fn for_width(row_width: f32) -> Self {
        let inside = row_width - 2.0 * ROW_INSET;
        let gaps = 2.0 * ROW_PICTURE_GAP;
        let beside = (inside - NUMBER_COLUMN - FACTS_GAP - FACTS_COLUMN - MENU_COLUMN - gaps) / 3.0;
        if beside >= NARROWEST_PICTURE_BESIDE {
            let picture = picture_of_width(beside.min(WIDEST_ROW_PICTURE));
            return Self {
                pictures: PicturesPlace::Beside,
                picture,
                height: picture.y + 2.0 * ROW_PICTURE_MARGIN,
            };
        }
        let picture = picture_of_width(((inside - gaps) / 3.0).clamp(1.0, WIDEST_ROW_PICTURE));
        Self {
            pictures: PicturesPlace::Under,
            picture,
            height: ROW_FACTS_LINE + picture.y + 2.5 * ROW_PICTURE_MARGIN,
        }
    }
}

fn picture_of_width(width: f32) -> egui::Vec2 {
    let width = width.floor();
    egui::vec2(width, (width * PICTURE_ASPECT).round())
}

/// What a person needs to know about a scene to decide on it: its length,
/// where it starts and ends, and only then what stands out, such as being
/// long enough to hide a missed cut, merged, dropped or a chapter's title.
fn show_scene_facts(
    ui: &mut egui::Ui,
    layout: &SceneLayout,
    fate: CardFate,
    chapter_title: Option<String>,
    pictures: PicturesPlace,
) {
    let palette = theme::palette(ui.ctx());
    let long = is_long_scene(layout.span);
    let length_color = match (fate, long) {
        (CardFate::Dropped, _) => palette.muted_text,
        (_, true) => palette.danger,
        (_, false) => palette.text,
    };
    let length = theme::semibold(format_clock(layout.span.seconds()), 15.0).color(length_color);
    let span = format!(
        "{} to {}",
        format_clock(layout.span.start),
        format_clock(layout.span.end)
    );
    let mut facts: Vec<(String, egui::Color32, Option<&str>)> = Vec::new();
    if let Some(title) = chapter_title {
        facts.push((title, palette.text, None));
    }
    if fate.is_dropped() {
        facts.push((DROPPED_BADGE.to_owned(), palette.muted_text, None));
    } else if long {
        facts.push((
            LONG_SCENE_FACT.to_owned(),
            palette.danger,
            Some(LONG_SCENE_HINT),
        ));
    }
    if layout.is_merged() {
        let parts = format!("{} parts merged", layout.segments.len());
        facts.push((parts, palette.primary, None));
    }
    let fact_label =
        |ui: &mut egui::Ui, (words, color, hint): (String, egui::Color32, Option<&str>)| {
            let label = ui.add(
                egui::Label::new(egui::RichText::new(words).size(FACT_SIZE).color(color))
                    .truncate(),
            );
            if let Some(hint) = hint {
                label.on_hover_text(hint);
            }
        };
    match pictures {
        PicturesPlace::Beside => {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(length);
                ui.label(
                    egui::RichText::new(span)
                        .size(FACT_SIZE)
                        .color(palette.muted_text),
                );
                for fact in facts {
                    fact_label(ui, fact);
                }
            });
        }
        PicturesPlace::Under => {
            ui.horizontal_centered(|ui| {
                ui.label(length);
                // One line only: what stands out, or else where it starts.
                match facts.into_iter().next() {
                    Some(fact) => fact_label(ui, fact),
                    None => {
                        ui.label(
                            egui::RichText::new(span)
                                .size(FACT_SIZE)
                                .color(palette.muted_text),
                        );
                    }
                }
            });
        }
    }
}

/// "Scenes" and how to merge them, or how many are chosen, with the merge
/// of the chosen ones on the right.
fn show_scenes_heading(ui: &mut egui::Ui, session: &ReviewSession) -> Vec<ReviewRequest> {
    let mut requests = Vec::new();
    ui.horizontal(|ui| {
        ui.set_min_height(theme::CONTROL_HEIGHT);
        ui.label(theme::semibold(SCENES_HEADING, CARD_HEADING_SIZE))
            .on_hover_text(SCENES_HINT);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if session.selection().span_to_merge().is_some() {
                let merge = IconButton::new(Icon::Merge, MERGE_LABEL, ButtonKind::Plain);
                if ui.add(merge).on_hover_text(MERGE_SELECTED_HINT).clicked() {
                    requests.push(ReviewRequest::MergeSelected);
                }
            }
            if session.has_merged_scene_selected() {
                let apart = IconButton::new(Icon::Layers, TAKE_APART_LABEL, ButtonKind::Quiet);
                if ui.add(apart).on_hover_text(TAKE_APART_HINT).clicked() {
                    requests.push(ReviewRequest::TakeSelectedApart);
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let chosen = session.selection().scenes().count();
                if chosen > 1 {
                    let words = format!("{} selected", counted(chosen, "scene"));
                    ui.label(theme::caption(ui, words));
                    return;
                }
                // A hint cut short reads as a fault, so it shows whole or
                // not at all. The heading says it on hover either way.
                let hint = theme::caption(ui, SCENES_HINT);
                if theme::text_width(ui, &hint) <= ui.available_width() {
                    ui.label(hint);
                }
            });
        });
    });
    requests
}

/// Which edge of a panel can be dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
enum GripEdge {
    Top,
    Left,
}

/// A short bar in the middle of the edge of `panel` while the pointer is
/// on that edge, so the edge is seen to be one that can be dragged. Away
/// from it, nothing is drawn: bars at rest read as stray marks.
fn paint_grip(ctx: &egui::Context, panel: egui::Rect, edge: GripEdge) {
    let palette = theme::palette(ctx);
    let (center, size, edge_band) = match edge {
        GripEdge::Top => (
            panel.center_top() + egui::vec2(0.0, theme::PANEL_MARGIN / 2.0),
            egui::vec2(GRIP_LENGTH, GRIP_THICKNESS),
            egui::Rect::from_x_y_ranges(
                panel.x_range(),
                egui::Rangef::new(panel.top() - GRIP_REACH, panel.top() + theme::PANEL_MARGIN),
            ),
        ),
        GripEdge::Left => (
            panel.left_center() + egui::vec2(theme::PANEL_MARGIN / 2.0, 0.0),
            egui::vec2(GRIP_THICKNESS, GRIP_LENGTH),
            egui::Rect::from_x_y_ranges(
                egui::Rangef::new(
                    panel.left() - GRIP_REACH,
                    panel.left() + theme::PANEL_MARGIN,
                ),
                panel.y_range(),
            ),
        ),
    };
    // On the panels' own layer, so a dialog in front covers it.
    let layer = egui::LayerId::background();
    let on_the_edge = ctx.pointer_hover_pos().is_some_and(|pointer| {
        edge_band.contains(pointer) && ctx.layer_id_at(pointer) == Some(layer)
    });
    if !on_the_edge {
        return;
    }
    let grip = egui::Rect::from_center_size(center, size);
    ctx.layer_painter(layer)
        .rect_filled(grip, GRIP_THICKNESS / 2.0, palette.primary);
    egui::show_tooltip_at_pointer(ctx, layer, egui::Id::new("panel_grip_hint"), |ui| {
        ui.label(edge.hint());
    });
}

impl GripEdge {
    fn hint(self) -> &'static str {
        match self {
            GripEdge::Top => TIMELINE_GRIP_HINT,
            GripEdge::Left => SCENES_GRIP_HINT,
        }
    }
}

/// egui keeps a panel at the width it was dragged to, whatever the window
/// does after. When the room for it changes, as when the window is made
/// smaller, the panel keeps its share of the room instead, so the picture
/// beside it keeps its share too.
fn keep_share_of_width(ctx: &egui::Context, panel_id: egui::Id, room: f32) {
    let room_id = panel_id.with("room_last_frame");
    let room_before: Option<f32> = ctx.data(|data| data.get_temp(room_id));
    ctx.data_mut(|data| data.insert_temp(room_id, room));
    let Some(room_before) = room_before.filter(|&before| before > 0.0 && before != room) else {
        return;
    };
    let Some(mut panel) = egui::panel::PanelState::load(ctx, panel_id) else {
        return;
    };
    let width = panel.rect.width() * room / room_before;
    panel.rect.set_left(panel.rect.right() - width);
    ctx.data_mut(|data| data.insert_persisted(panel_id, panel));
}

/// How tall the film strip can be in a card of `height`, when the ruler,
/// the cut times, the overview and the controls take `around` of it.
fn strip_height_within(height: f32, around: f32) -> f32 {
    (height - around).clamp(STRIP_HEIGHT, TALLEST_STRIP)
}

/// What a dragged row would do, beside the pointer.
fn paint_drag_hint(ctx: &egui::Context, pointer: egui::Pos2, hint: &str) {
    let palette = theme::palette(ctx);
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("scene_table_drag_hint"));
    let painter = ctx.layer_painter(layer);
    let galley = painter.layout_no_wrap(
        hint.to_owned(),
        theme::semibold_font(13.0),
        palette.on_primary,
    );
    let place = egui::Rect::from_min_size(
        pointer + egui::vec2(14.0, 10.0),
        galley.size() + egui::vec2(16.0, 10.0),
    );
    painter.rect_filled(place, palette.control_rounding(), palette.primary_fill);
    painter.galley(place.min + egui::vec2(8.0, 5.0), galley, palette.on_primary);
}

/// Play the scene, or drop it or keep it after all.
fn show_row_menu(
    ui: &mut egui::Ui,
    scene: SceneIndex,
    span: TimeSpan,
    fate: CardFate,
) -> Vec<ReviewRequest> {
    let mut requests = Vec::new();
    theme::more_menu(ui, ROW_MENU_HINT, |ui| {
        if ui.button(PLAY_SCENE_LABEL).clicked() {
            requests.push(ReviewRequest::PlayFrom(span.start));
            ui.close_menu();
        }
        let (label, request) = if fate.is_dropped() {
            (KEEP_SCENE_LABEL, ReviewRequest::KeepScene(scene))
        } else {
            (DROP_SCENE_LABEL, ReviewRequest::DropScene(scene))
        };
        if ui.button(label).clicked() {
            requests.push(request);
            ui.close_menu();
        }
    });
    requests
}

/// "Add a split at", a box for the time and a plus that adds it, laid out
/// from the right. The words go first when the row is short of room.
fn show_typed_split_controls(ui: &mut egui::Ui, session: &ReviewSession) -> Vec<ReviewRequest> {
    let mut requests = Vec::new();
    let add = theme::ToolButton::new(Icon::Plus, ADD_SPLIT_LABEL, ButtonKind::Plain)
        .size(theme::ToolButtonSize::Small);
    let add_clicked = ui.add(add).on_hover_text(ADD_SPLIT_LABEL).clicked();
    // A hint that is a real time in this video, not one beyond its end.
    let example_time = format_clock(session.plan().whole_video().seconds() / 2.0);
    // The box edits a copy of what the session holds. What the person types
    // goes back as a request, like everything else they do here.
    let mut typed_time = session.new_split_time().to_owned();
    let field = ui
        .add(
            egui::TextEdit::singleline(&mut typed_time)
                .hint_text(example_time)
                .desired_width(TYPED_SPLIT_FIELD_WIDTH),
        )
        .on_hover_text(TYPED_SPLIT_HINT);
    if field.changed() {
        requests.push(ReviewRequest::TypeNewSplitTime(typed_time));
    }
    let pressed_enter = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    if add_clicked || pressed_enter {
        requests.push(ReviewRequest::AddTypedSplit);
    }
    let words = theme::muted(ui, TYPED_SPLIT_LABEL);
    if theme::text_width(ui, &words) + GAP <= ui.available_width() {
        ui.label(words);
    }
    requests
}

/// "Split at 0:42". Its hover names the key S that does the same.
pub(super) fn show_split_at_button(ui: &mut egui::Ui, playhead: Option<f64>) -> Option<PlayerAsk> {
    let position = playhead.unwrap_or(0.0);
    let split = IconButton::new(
        Icon::Scissors,
        format!("{SPLIT_AT_LABEL} {}", format_clock(position)),
        ButtonKind::Plain,
    );
    ui.add(split)
        .on_hover_text(theme::hint_with_key(
            ui.ctx(),
            SPLIT_AT_HINT,
            &crate::keyboard::SPLIT_SHORTCUT,
        ))
        .clicked()
        .then_some(PlayerAsk::SplitHere)
}

/// The one bar left above the footage: the name of the video, the way
/// back to everything else, and the split. The choices about the split are
/// the ones last made.
pub(super) fn show_footage_bar(ui: &mut egui::Ui, session: &ReviewSession) -> Vec<ReviewRequest> {
    let mut requests = Vec::new();
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            requests.extend(show_split_button(ui, session));
            requests.extend(show_room_button(
                ui,
                ReviewRoom::FootageOnly,
                ButtonWords::Shown,
            ));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let name = file_name(session.video());
                ui.add(egui::Label::new(theme::muted(ui, &name)).truncate())
                    .on_hover_text(name);
            });
        });
    });
    requests
}

/// Whether the buttons of a row say what they do, or show only their icons
/// and say it on hover.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ButtonWords {
    Shown,
    OnHover,
}

impl ButtonWords {
    pub(super) fn for_room(room: f32, room_for_words: f32) -> Self {
        if room >= room_for_words {
            Self::Shown
        } else {
            Self::OnHover
        }
    }
}

/// A button with its words or, short of room, its icon alone with the
/// words on hover.
fn add_worded_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    words: ButtonWords,
) -> egui::Response {
    match words {
        ButtonWords::Shown => ui.add(IconButton::new(icon, label, ButtonKind::Plain)),
        ButtonWords::OnHover => {
            let button = theme::ToolButton::new(icon, label, ButtonKind::Plain)
                .size(theme::ToolButtonSize::Small);
            ui.add(button).on_hover_text(label)
        }
    }
}

/// "Focus", or "Show everything" when the footage has the window. Its
/// hover names the key F that does the same.
pub(super) fn show_room_button(
    ui: &mut egui::Ui,
    room: ReviewRoom,
    words: ButtonWords,
) -> Option<ReviewRequest> {
    let (icon, label, hint) = match room {
        ReviewRoom::Everything => (Icon::Film, FOCUS_LABEL, FOCUS_HINT),
        ReviewRoom::FootageOnly => (Icon::Close, SHOW_EVERYTHING_LABEL, SHOW_EVERYTHING_HINT),
    };
    add_worded_button(ui, icon, label, words)
        .on_hover_text(theme::hint_with_key(
            ui.ctx(),
            hint,
            &crate::keyboard::ROOM_SHORTCUT,
        ))
        .clicked()
        .then_some(ReviewRequest::ChangeSettings(
            SettingsChange::SwitchReviewRoom,
        ))
}

/// What happens to the original and where the scenes go, then the split.
pub(super) fn show_export_bar(
    ui: &mut egui::Ui,
    session: &ReviewSession,
    settings: &Settings,
) -> Vec<ReviewRequest> {
    let mut requests = Vec::new();
    let export = settings.export;
    let mut wanted = export;
    let show_choices = |ui: &mut egui::Ui| {
        show_original_choice(ui, session, &mut wanted);
        show_save_to_choice(ui, session, &mut wanted);
    };
    let words = ButtonWords::for_room(ui.available_width(), EXPORT_BAR_WITH_WORDS);
    // In a narrow window the choices take a row of their own, so they never
    // cover the button that splits.
    let mut show_actions = |ui: &mut egui::Ui| {
        requests.extend(show_split_button(ui, session));
        if words == ButtonWords::Shown {
            requests.extend(show_split_later_button(ui, session));
            requests.extend(show_skip_button(ui));
            return;
        }
        theme::more_menu(ui, LATER_OR_SKIP_HINT, |ui| {
            let later = ui
                .add_enabled(
                    session.scene_count() > 1,
                    egui::Button::new(SPLIT_LATER_LABEL),
                )
                .on_hover_text(SPLIT_LATER_EXPLANATION);
            if later.clicked() {
                requests.push(ReviewRequest::SplitLater);
                ui.close_menu();
            }
            if ui.button(SKIP_LABEL).clicked() {
                requests.push(ReviewRequest::Skip);
                ui.close_menu();
            }
        });
    };
    if ui.available_width() >= EXPORT_BAR_ONE_ROW_WIDTH {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                show_actions(ui);
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center),
                    show_choices,
                );
            });
        });
    } else {
        ui.horizontal_wrapped(show_choices);
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            show_actions,
        );
    }
    requests.extend(show_export_obstacle(ui, session));
    show_exact_cut_notice(ui, session, export.cut_precision);
    if wanted != export {
        requests.push(ReviewRequest::ChangeSettings(
            SettingsChange::ExportOptions(wanted),
        ));
    }
    requests
}

/// Keep the original, or move it once the scenes pass the check. When it
/// is kept whatever is chosen, that is said instead.
fn show_original_choice(ui: &mut egui::Ui, session: &ReviewSession, wanted: &mut ExportOptions) {
    let always_kept = OriginalAlwaysKept::for_split(session.dropped_count(), wanted.cut_precision);
    if let Some(why) = always_kept {
        ui.label(theme::muted(ui, KEEP_ORIGINAL_CHOICE))
            .on_hover_text(why.in_plain_words());
        return;
    }
    let recycle = RECYCLE_ORIGINAL_CHOICE.to_owned();
    let shown = match wanted.original_policy {
        OriginalPolicy::Keep => KEEP_ORIGINAL_CHOICE.to_owned(),
        OriginalPolicy::RecycleOnceVerified => recycle.clone(),
    };
    egui::ComboBox::from_id_salt("editor_original")
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut wanted.original_policy,
                OriginalPolicy::Keep,
                KEEP_ORIGINAL_CHOICE,
            );
            ui.selectable_value(
                &mut wanted.original_policy,
                OriginalPolicy::RecycleOnceVerified,
                recycle,
            );
        })
        .response
        .on_hover_text(format!(
            "{} You can restore it from the {TRASH_NAME}.",
            settings_controls::RECYCLE_CHECK_EXPLANATION
        ));
}

fn show_save_to_choice(ui: &mut egui::Ui, session: &ReviewSession, wanted: &mut ExportOptions) {
    let shown = match wanted.scene_location {
        SceneLocation::BesideVideo => BESIDE_VIDEO_CHOICE,
        SceneLocation::NewFolder => NEW_FOLDER_CHOICE,
    };
    let destination = split::scene_dir(session.video(), wanted.scene_location)
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    egui::ComboBox::from_id_salt("editor_save_to")
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut wanted.scene_location,
                SceneLocation::BesideVideo,
                BESIDE_VIDEO_CHOICE,
            );
            ui.selectable_value(
                &mut wanted.scene_location,
                SceneLocation::NewFolder,
                NEW_FOLDER_CHOICE,
            );
        })
        .response
        .on_hover_text(destination);
}

fn card_frame(ui: &egui::Ui) -> egui::Frame {
    let palette = theme::palette(ui.ctx());
    egui::Frame::none()
        .fill(palette.card)
        .stroke(egui::Stroke::new(1.0_f32, palette.card_border))
        .rounding(theme::CARD_ROUNDING)
        .inner_margin(egui::Margin::same(CARD_PADDING))
}

/// A card with an edge around `add_contents`, as wide as there is room.
pub(super) fn card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_frame(ui)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// A card that fills all the room it is given, as the panels of the
/// preview and of the scenes do.
pub(super) fn card_filling<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    card_frame(ui)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ui.available_height());
            add_contents(ui)
        })
        .inner
}

/// The ticks and times along the top of a strip that shows `shown`.
fn paint_ruler(
    painter: &egui::Painter,
    ruler: egui::Rect,
    shown: TimeSpan,
    palette: &theme::Palette,
) {
    let seconds = shown.seconds();
    if seconds <= 0.0 {
        return;
    }
    let pixels_per_second = f64::from(ruler.width()) / seconds;
    let least_step = f64::from(LEAST_PIXELS_BETWEEN_TICK_LABELS) / pixels_per_second;
    let step = TICK_STEPS
        .into_iter()
        .find(|&step| step >= least_step)
        .unwrap_or(TICK_STEPS[TICK_STEPS.len() - 1]);
    let minor_step = step / MINOR_TICKS_PER_STEP;
    let line = egui::Stroke::new(1.0_f32, palette.card_border);
    let first_minor = (shown.start / minor_step).ceil() as u64;
    let last_minor = (shown.end / minor_step).floor() as u64;
    for minor in first_minor..=last_minor {
        let tick = minor as f64 * minor_step;
        let x = ruler.left() + ((tick - shown.start) * pixels_per_second) as f32;
        let labelled = minor % MINOR_TICKS_PER_STEP as u64 == 0;
        let tick_length = if labelled { 8.0 } else { 4.0 };
        painter.vline(
            x,
            egui::Rangef::new(ruler.bottom() - tick_length, ruler.bottom()),
            line,
        );
        if labelled {
            painter.text(
                egui::pos2(x + 3.0, ruler.top()),
                egui::Align2::LEFT_TOP,
                format_clock(tick),
                egui::FontId::proportional(11.0),
                palette.muted_text,
            );
        }
    }
}

/// The scene's number in a dark chip at the top left of its block, when
/// the block is wide enough for it.
fn paint_scene_label(
    painter: &egui::Painter,
    block: egui::Rect,
    scene: SceneIndex,
    color: egui::Color32,
    palette: &theme::Palette,
) {
    let number = painter.layout_no_wrap(
        format!("{:02}", scene.number()),
        theme::semibold_font(12.0),
        color,
    );
    let width = number.size().x + 12.0;
    if width + 12.0 > block.width() {
        return;
    }
    let chip = egui::Rect::from_min_size(
        block.left_top() + egui::vec2(6.0, 6.0),
        egui::vec2(width, 20.0),
    );
    painter.rect_filled(
        chip,
        SCENE_LABEL_ROUNDING,
        palette.page.gamma_multiply(0.85),
    );
    let text_top = chip.center().y - number.size().y / 2.0;
    painter.galley(egui::pos2(chip.left() + 6.0, text_top), number, color);
}

/// An amber line through the strip at a cut, a handle on top, and its time
/// under the strip.
fn paint_cut_marker(
    painter: &egui::Painter,
    x: f32,
    strip: egui::Rect,
    time: f64,
    color: egui::Color32,
) {
    painter.vline(
        x,
        strip.y_range(),
        egui::Stroke::new(CUT_MARKER_WIDTH, color),
    );
    let handle =
        egui::Rect::from_center_size(egui::pos2(x, strip.top() + 6.0), egui::vec2(8.0, 12.0));
    painter.rect_filled(handle, 2.0, color);
    painter.text(
        egui::pos2(x, strip.bottom() + 2.0),
        egui::Align2::CENTER_TOP,
        format_clock(time),
        theme::semibold_font(11.5),
        color,
    );
}

/// The part of a 16 by 9 picture that fills a tile of `size`, cut evenly at
/// the sides or at the top and bottom.
fn picture_crop(size: egui::Vec2) -> egui::Rect {
    let picture_shape = 1.0 / PICTURE_ASPECT;
    let tile_shape = size.x / size.y.max(1.0);
    if tile_shape < picture_shape {
        let cut = (1.0 - tile_shape / picture_shape) / 2.0;
        egui::Rect::from_min_max(egui::pos2(cut, 0.0), egui::pos2(1.0 - cut, 1.0))
    } else {
        let cut = (1.0 - picture_shape / tile_shape) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, cut), egui::pos2(1.0, 1.0 - cut))
    }
}

/// Three moments of a scene: near its start, its middle and near its end.
/// Two scenes run together show as two different looking pictures, which
/// is how a missed split is seen. Near, not at, the ends, so a fade from or
/// to black at a cut does not give a black picture.
fn scene_picture_times(span: TimeSpan) -> [f64; 3] {
    let into_scene = SCENE_PICTURE_INTO_SCENE.min(span.seconds() * SCENE_PICTURE_LARGEST_SHARE);
    [
        span.start + into_scene,
        span.start + span.seconds() / 2.0,
        span.end - into_scene,
    ]
}

const SCENE_PICTURE_INTO_SCENE: f64 = 1.0;
const SCENE_PICTURE_LARGEST_SHARE: f64 = 0.1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_is_shown_near_its_start_in_its_middle_and_near_its_end() {
        let long = TimeSpan {
            start: 60.0,
            end: 180.0,
        };
        assert_eq!(scene_picture_times(long), [61.0, 120.0, 179.0]);
        let short = TimeSpan {
            start: 10.0,
            end: 12.0,
        };
        let [first, middle, last] = scene_picture_times(short);
        assert!((first - 10.2).abs() < 1e-9);
        assert_eq!(middle, 11.0);
        assert!((last - 11.8).abs() < 1e-9);
    }

    #[test]
    fn a_wider_table_shows_larger_pictures_up_to_their_decoded_size() {
        let narrow = RowShape::for_width(560.0);
        let wide = RowShape::for_width(900.0);
        let huge = RowShape::for_width(3000.0);
        assert_eq!(narrow.pictures, PicturesPlace::Beside);
        assert!(wide.picture.x > narrow.picture.x);
        assert!(wide.height > narrow.height);
        assert_eq!(huge.picture.x, WIDEST_ROW_PICTURE);
        assert!((wide.picture.y / wide.picture.x - PICTURE_ASPECT).abs() < 0.01);
    }

    #[test]
    fn in_a_narrow_table_the_pictures_go_under_the_facts() {
        let shape = RowShape::for_width(300.0);
        assert_eq!(shape.pictures, PicturesPlace::Under);
        let beside = (300.0 - 2.0 * ROW_INSET - NUMBER_COLUMN - FACTS_COLUMN - MENU_COLUMN) / 3.0;
        assert!(shape.picture.x > beside);
        assert!(shape.picture.x * 3.0 <= 300.0 - 2.0 * ROW_INSET);
    }

    #[test]
    fn a_taller_timeline_gives_the_strip_its_height_within_limits() {
        assert_eq!(strip_height_within(0.0, AROUND_THE_STRIP), STRIP_HEIGHT);
        let tall = strip_height_within(200.0, AROUND_THE_STRIP);
        assert_eq!(tall, 200.0 - AROUND_THE_STRIP);
        assert_eq!(strip_height_within(3000.0, AROUND_THE_STRIP), TALLEST_STRIP);
    }

    #[test]
    fn a_close_up_has_a_ruler_and_the_overview_under_it_is_thin() {
        let video = TimeSpan {
            start: 0.0,
            end: 600.0,
        };
        let chosen = TimeSpan {
            start: 100.0,
            end: 160.0,
        };
        let close_up = StripView::close_up(chosen, STRIP_HEIGHT);
        let overview = StripView::overview(video, chosen);
        assert_eq!(close_up.shown, chosen);
        assert_eq!(overview.shown, video);
        assert_eq!(overview.framed, Some(chosen));
        assert!(overview.total_height() < close_up.total_height() / 2.0);
    }

    #[test]
    fn a_tile_shows_the_middle_of_the_picture() {
        let wide = picture_crop(egui::vec2(320.0, 90.0));
        assert_eq!(wide.left(), 0.0);
        assert!(wide.top() > 0.0 && wide.bottom() < 1.0);
        let narrow = picture_crop(egui::vec2(40.0, 90.0));
        assert!(narrow.left() > 0.0 && narrow.right() < 1.0);
        let exact = picture_crop(egui::vec2(160.0, 90.0));
        assert!((exact.left()).abs() < 1e-6 && (exact.top()).abs() < 1e-6);
    }
}
