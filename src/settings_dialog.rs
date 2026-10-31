//! The Settings dialog: one tab per kind of setting, each group in a card,
//! so the person sees a handful of choices at a time and not all of them.
//! It only draws. What the person changes comes back as requests.

use eframe::egui;

use crate::export::{OriginalAlwaysKept, SplitMethod, TRASH_NAME};
use crate::icons::Icon;
use crate::keyboard;
use crate::settings::Settings;
use crate::settings_controls::{self, SceneNameDraft};
use crate::theme;

const DIALOG_TITLE: &str = "Settings";
/// Narrow enough for the smallest window, with room around it.
const DIALOG_WIDTH: f32 = 470.0;
/// Room above and below the dialog that stays free in a short window.
const DIALOG_VERTICAL_ROOM: f32 = 140.0;
const CLOSE_LABEL: &str = "Close";
const CLOSE_HINT: &str = "Close (Esc)";
/// The space above and below the line between two groups, and around the tabs.
const SECTION_GAP: f32 = 12.0;
const TAB_HEIGHT: f32 = 30.0;
const TAB_INSET: f32 = 3.0;
/// The height of the part under the tabs, the same for every tab.
const BODY_HEIGHT: f32 = 360.0;

const WHEN_SPLIT_HEADING: &str = "When a video is split";
const FILES_HEADING: &str = "The scene files";
const MISSED_SCENES_HEADING: &str = "Looking for missed scenes";
const BLACK_SCREENS_HEADING: &str = "Black screens";
const BLACK_SCREENS_NOTE: &str =
    "The defaults suit most films. Change them only if scenes are missed or invented.";
const LOOK_HEADING: &str = "How the window looks";
const REVIEW_HEADING: &str = "The review screen";
const SHOW_TIPS_LABEL: &str = "Show tips on the review screen";
const FOLDERS_HEADING: &str = "Folders with their own scan settings";
const FOLDERS_EXPLANATION: &str = "Videos in these folders are not scanned with the general \
     settings. Change a folder's settings on the review screen of one of its videos.";
const NO_FOLDERS_NOTE: &str = "No folder has settings of its own. A review screen can keep \
     its scan settings for the folder its video is in.";
const FORGET_FOLDER_LABEL: &str = "Forget";
const FORGET_FOLDER_EXPLANATION: &str =
    "Videos in this folder are scanned with the general settings again.";
const SETTINGS_SAVED_NOTE: &str =
    "Saved straight away. Press Rescan to use new scan settings on this video.";
const RESTORE_DEFAULTS_LABEL: &str = "Restore default settings";
const RESTORE_DEFAULTS_EXPLANATION: &str = "Puts every setting here back to how it was at first.";
/// Beside the button and not in a hover: it cannot be taken back, so it is
/// said where it is read without pointing.
const RESTORE_DEFAULTS_NOTE: &str = "Also forgets the settings remembered for folders. \
     What happens to originals stays as it is.";

/// The parts of the dialog, one shown at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Splitting,
    FindingScenes,
    Appearance,
    Folders,
}

impl SettingsTab {
    const ALL: [Self; 4] = [
        Self::Splitting,
        Self::FindingScenes,
        Self::Appearance,
        Self::Folders,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Splitting => "Splitting",
            Self::FindingScenes => "Finding scenes",
            Self::Appearance => "Appearance",
            Self::Folders => "Folders",
        }
    }
}

/// What the person did in the dialog during one frame.
#[derive(Debug, PartialEq)]
pub enum SettingsDialogRequest {
    ShowTab(SettingsTab),
    /// Use these settings from now on: the dialog hands back the settings
    /// as its controls left them, whenever they differ from those in use.
    ChangeTo(Box<Settings>),
    Close,
}

/// What the dialog shows: the settings in use, why they could not be saved
/// (or nothing), and the name pattern as it is being typed.
pub struct SettingsInDialog<'app> {
    pub in_use: &'app Settings,
    pub problem: &'app str,
    pub scene_name_draft: &'app mut SceneNameDraft,
}

/// Draws the dialog in front of everything, with the page dimmed behind it.
/// Escape, a click on the dimmed part or Close closes it.
pub fn show(
    ctx: &egui::Context,
    tab: SettingsTab,
    shown: SettingsInDialog,
) -> Vec<SettingsDialogRequest> {
    let mut requests = Vec::new();
    let clicked_outside =
        theme::show_backdrop(ctx, "settings_dialog_backdrop", egui::Order::Middle);
    let screen = ctx.screen_rect();
    let width = DIALOG_WIDTH.min(screen.width() - 2.0 * theme::PANEL_MARGIN);
    let most_height = (screen.height() - DIALOG_VERTICAL_ROOM).max(200.0);
    let mut settings = shown.in_use.clone();
    egui::Window::new(DIALOG_TITLE)
        .title_bar(false)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_width(width);
            if show_header(ui) {
                requests.push(SettingsDialogRequest::Close);
            }
            if let Some(chosen) = show_tabs(ui, tab) {
                requests.push(SettingsDialogRequest::ShowTab(chosen));
            }
            ui.add_space(SECTION_GAP);
            // Every tab is as tall as the others, so the dialog keeps its
            // place and size as the tabs are gone through.
            let body_height = BODY_HEIGHT.min(most_height);
            egui::ScrollArea::vertical()
                .max_height(body_height)
                .min_scrolled_height(body_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    show_problem(ui, shown.problem);
                    match tab {
                        SettingsTab::Splitting => {
                            show_splitting(ui, &mut settings, shown.scene_name_draft);
                        }
                        SettingsTab::FindingScenes => show_finding_scenes(ui, &mut settings),
                        SettingsTab::Appearance => show_appearance(ui, &mut settings),
                        SettingsTab::Folders => show_folders(ui, &mut settings),
                    }
                });
            ui.separator();
            if show_footer(ui) {
                settings = shown.in_use.restored_to_defaults();
            }
        });

    if settings != *shown.in_use {
        requests.push(SettingsDialogRequest::ChangeTo(Box::new(settings)));
    }
    // The dialog is in front of everything, so Escape is its to take.
    if keyboard::take_escape(ctx) || clicked_outside {
        requests.push(SettingsDialogRequest::Close);
    }
    requests
}

/// The title and the button that closes the dialog. True when it was pressed.
fn show_header(ui: &mut egui::Ui) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(theme::semibold(DIALOG_TITLE, 20.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = theme::ToolButton::new(Icon::Close, CLOSE_LABEL, theme::ButtonKind::Quiet)
                .size(theme::ToolButtonSize::Small);
            close = ui.add(button).on_hover_text(CLOSE_HINT).clicked();
        });
    });
    ui.add_space(SECTION_GAP);
    close
}

/// The tabs as one track across the dialog, each of the same width, the
/// one shown filled. Returns another tab when one is picked.
fn show_tabs(ui: &mut egui::Ui, shown: SettingsTab) -> Option<SettingsTab> {
    let palette = theme::palette(ui.ctx());
    let (track, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TAB_HEIGHT + 2.0 * TAB_INSET),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(track, palette.control_rounding(), palette.input_fill);
    let tab_width = (track.width() - 2.0 * TAB_INSET) / SettingsTab::ALL.len() as f32;
    let mut chosen = None;
    for (place, tab) in SettingsTab::ALL.into_iter().enumerate() {
        let rect = egui::Rect::from_min_size(
            track.min + egui::vec2(TAB_INSET + tab_width * place as f32, TAB_INSET),
            egui::vec2(tab_width, TAB_HEIGHT),
        );
        let response = ui.interact(rect, ui.id().with(tab.label()), egui::Sense::click());
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tab.label()));
        let is_shown = tab == shown;
        let (fill, ink, font) = if is_shown {
            (palette.card, palette.primary, theme::semibold_font(14.0))
        } else if response.hovered() {
            (
                palette.control_hovered_fill,
                palette.text,
                egui::FontId::proportional(14.0),
            )
        } else {
            (
                egui::Color32::TRANSPARENT,
                palette.muted_text,
                egui::FontId::proportional(14.0),
            )
        };
        ui.painter()
            .rect_filled(rect, palette.control_rounding(), fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            tab.label(),
            font,
            ink,
        );
        if response.clicked() && !is_shown {
            chosen = Some(tab);
        }
    }
    chosen
}

fn show_problem(ui: &mut egui::Ui, problem: &str) {
    if problem.is_empty() {
        return;
    }
    let palette = theme::palette(ui.ctx());
    ui.add(theme::selectable(
        egui::RichText::new(problem).color(palette.danger),
    ));
    theme::copy_details_button(ui, problem);
    ui.add_space(SECTION_GAP);
}

/// One group of settings under a heading, as wide as the dialog. Groups
/// after the first are set apart by a line.
fn show_group(ui: &mut egui::Ui, heading: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let first = ui.min_rect().height() < 1.0;
    if !first {
        ui.add_space(SECTION_GAP);
        ui.separator();
        ui.add_space(SECTION_GAP);
    }
    ui.label(theme::semibold(heading, 15.0));
    ui.add_space(4.0);
    add_contents(ui);
}

fn show_splitting(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    scene_name_draft: &mut SceneNameDraft,
) {
    show_group(ui, WHEN_SPLIT_HEADING, |ui| {
        settings_controls::show_cut_precision_setting(ui, &mut settings.export.cut_precision);
        // An exact split keeps the original whatever this box says, so the
        // box is not offered as if it had a say.
        let recycling_has_a_say = matches!(
            settings.export.split_method(),
            SplitMethod::CopyAtKeyframes { .. }
        );
        ui.add_enabled_ui(recycling_has_a_say, |ui| {
            settings_controls::show_original_policy_setting(
                ui,
                &mut settings.export.original_policy,
                &format!("Move the original to the {TRASH_NAME} after the check"),
            )
            .on_hover_text(settings_controls::RECYCLE_CHECK_EXPLANATION)
            .on_disabled_hover_text(OriginalAlwaysKept::CutIsExact.in_plain_words());
        });
    });
    show_group(ui, FILES_HEADING, |ui| {
        settings_controls::show_scene_location_setting(ui, &mut settings.export.scene_location);
        settings_controls::show_scene_name_box(
            ui,
            scene_name_draft,
            &mut settings.export.scene_name,
        );
    });
}

fn show_finding_scenes(ui: &mut egui::Ui, settings: &mut Settings) {
    show_group(ui, MISSED_SCENES_HEADING, |ui| {
        settings_controls::show_hard_cut_controls(
            ui,
            &mut settings.detection.hard_cuts,
            "settings_hard_cut_sensitivity",
        );
    });
    show_group(ui, settings_controls::SCENE_LENGTHS_HEADING, |ui| {
        settings_controls::show_scene_length_controls(ui, &mut settings.detection.scene_length);
    });
    show_group(ui, BLACK_SCREENS_HEADING, |ui| {
        ui.label(theme::caption(ui, BLACK_SCREENS_NOTE));
        settings_controls::show_black_screen_controls(ui, &mut settings.detection.scan);
    });
}

fn show_appearance(ui: &mut egui::Ui, settings: &mut Settings) {
    show_group(ui, LOOK_HEADING, |ui| {
        settings_controls::show_style_choice(ui, &mut settings.style);
        settings_controls::show_theme_choice(ui, &mut settings.theme);
    });
    show_group(ui, REVIEW_HEADING, |ui| {
        settings_controls::show_switch_setting(ui, SHOW_TIPS_LABEL, &mut settings.show_tips);
    });
}

fn show_folders(ui: &mut egui::Ui, settings: &mut Settings) {
    show_group(ui, FOLDERS_HEADING, |ui| {
        if settings.folders.is_empty() {
            ui.add(egui::Label::new(theme::muted(ui, NO_FOLDERS_NOTE)).wrap());
            return;
        }
        ui.add(egui::Label::new(theme::caption(ui, FOLDERS_EXPLANATION)).wrap());
        let mut folder_to_forget = None;
        for (folder, _) in settings.folders.iter() {
            let shown = folder.display().to_string();
            ui.horizontal(|ui| {
                let forget = ui
                    .link(FORGET_FOLDER_LABEL)
                    .on_hover_text(FORGET_FOLDER_EXPLANATION);
                if forget.clicked() {
                    folder_to_forget = Some(folder.to_path_buf());
                }
                ui.add(egui::Label::new(theme::caption(ui, &shown)).truncate())
                    .on_hover_text(&shown);
            });
        }
        if let Some(folder) = folder_to_forget {
            settings.folders.forget(&folder);
        }
    });
}

/// The note on saving and the way back to the defaults. True when the
/// defaults were asked for.
fn show_footer(ui: &mut egui::Ui) -> bool {
    let mut restore = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = theme::IconButton::new(
                Icon::Undo,
                RESTORE_DEFAULTS_LABEL,
                theme::ButtonKind::Quiet,
            );
            restore = ui
                .add(button)
                .on_hover_text(format!(
                    "{RESTORE_DEFAULTS_EXPLANATION} {RESTORE_DEFAULTS_NOTE}"
                ))
                .clicked();
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(theme::caption(ui, SETTINGS_SAVED_NOTE)).wrap());
            });
        });
    });
    restore
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(
        ctx: &egui::Context,
        tab: SettingsTab,
        events: Vec<egui::Event>,
    ) -> Vec<SettingsDialogRequest> {
        let settings = Settings::default();
        let mut draft = SceneNameDraft::of(settings.export.scene_name);
        let mut requests = Vec::new();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            let shown = SettingsInDialog {
                in_use: &settings,
                problem: "",
                scene_name_draft: &mut draft,
            };
            requests = show(ctx, tab, shown);
        });
        requests
    }

    #[test]
    fn every_tab_is_drawn_in_the_smallest_window_and_changes_nothing() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        for tab in SettingsTab::ALL {
            draw(&ctx, tab, Vec::new());
            assert_eq!(draw(&ctx, tab, Vec::new()), Vec::new(), "{tab:?}");
        }
    }

    #[test]
    fn escape_closes_the_dialog() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        draw(&ctx, SettingsTab::Splitting, Vec::new());
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let requests = draw(&ctx, SettingsTab::Splitting, vec![escape]);
        assert_eq!(requests, vec![SettingsDialogRequest::Close]);
    }

    #[test]
    fn every_tab_has_a_name_of_its_own() {
        let names: std::collections::HashSet<_> = SettingsTab::ALL.map(SettingsTab::label).into();
        assert_eq!(names.len(), SettingsTab::ALL.len());
    }
}
