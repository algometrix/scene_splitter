//! The controls for settings that are offered in more than one place: in the
//! sidebar's Settings section and on the review screen. Each is drawn here
//! once, so its range and its meaning cannot drift apart between the two.

use eframe::egui;

use crate::detect::{ScanMode, ScanSettings};
use crate::exact_cut::{CutPrecision, EXACT_CUT_SETTING};
use crate::export::OriginalPolicy;
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::scene_name::{PatternError, Placeholder, SceneNamePattern};
use crate::split::SceneLocation;
use crate::theme::{self, ThemeChoice};
use crate::timecode::SECONDS_PER_MINUTE;

/// The range offered for the shortest scene a hard-cut scan may produce.
const SHORTEST_SCENE_MINUTES: std::ops::RangeInclusive<f64> = 0.5..=60.0;
/// The ranges offered for what counts as a black screen.
const SHORTEST_BLACK_SECONDS: std::ops::RangeInclusive<f64> = 0.05..=10.0;
const BLACK_SHARE_PERCENT: std::ops::RangeInclusive<f64> = 50.0..=100.0;
const PERCENT: f64 = 100.0;

const THEME_LABEL: &str = "Colours";
const THEME_EXPLANATION: &str =
    "Light or dark. \"Same as the system\" changes with your computer's own setting.";
const THEME_BOX_ID: &str = "settings_theme";
const SCENE_NAME_LABEL: &str = "Name scene files";
const SCENE_NAME_HINT_INTRODUCTION: &str =
    "What each scene file is called. The ending of the video's name, such as .mp4, is added. \
     You can use:";
const SCENE_NAME_REFUSED_NOTE: &str = "Until that is fixed, scene files are still named like";

/// The box a scene name pattern is typed into. What is typed is often not a
/// usable pattern yet, half way through a placeholder for one, so the box
/// has text of its own and the settings only ever hold a pattern that
/// passed its checks.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneNameDraft {
    typed: String,
    /// The pattern in use when the text was last put in step with it. When
    /// the settings hold another one, they were changed elsewhere, as
    /// "Restore default settings" does, and the box shows that one.
    in_step_with: SceneNamePattern,
}

impl SceneNameDraft {
    pub fn of(in_use: SceneNamePattern) -> Self {
        Self {
            typed: in_use.as_str().to_owned(),
            in_step_with: in_use,
        }
    }

    /// Takes what now stands in the box. A usable pattern becomes the one
    /// in use at once. Anything else leaves the one in use as it is.
    fn take_typed(&mut self, typed: String, in_use: &mut SceneNamePattern) {
        self.typed = typed;
        if let Ok(pattern) = SceneNamePattern::new(&self.typed) {
            *in_use = pattern;
            self.in_step_with = pattern;
        }
    }

    /// Shows the pattern in use again, after it was changed somewhere else
    /// or the person left the box with something unusable in it.
    fn show_again(&mut self, in_use: SceneNamePattern) {
        *self = Self::of(in_use);
    }

    /// What the text in the box would name a scene, or why it cannot.
    fn outcome(&self) -> Result<String, PatternError> {
        SceneNamePattern::new(&self.typed).map(|pattern| pattern.example())
    }
}

/// What the pointer resting on the name box says: every placeholder and
/// what it stands for.
fn scene_name_hint() -> String {
    let mut hint = SCENE_NAME_HINT_INTRODUCTION.to_owned();
    for placeholder in Placeholder::ALL {
        hint.push_str(&format!(
            "\n{}  {}",
            placeholder.typed(),
            placeholder.meaning()
        ));
    }
    hint
}

/// The box for the name of scene files, with an example of the name under
/// it, or the reason the text cannot be used. `in_use` changes as soon as
/// what is typed is a usable pattern.
pub fn show_scene_name_box(
    ui: &mut egui::Ui,
    draft: &mut SceneNameDraft,
    in_use: &mut SceneNamePattern,
) {
    let palette = theme::palette(ui.ctx());
    if draft.in_step_with != *in_use {
        draft.show_again(*in_use);
    }
    ui.label(SCENE_NAME_LABEL).on_hover_text(scene_name_hint());
    let mut typed = draft.typed.clone();
    let name_box = ui
        .add(egui::TextEdit::singleline(&mut typed).desired_width(f32::INFINITY))
        .on_hover_text(scene_name_hint());
    if name_box.changed() {
        draft.take_typed(typed, in_use);
    }
    match draft.outcome() {
        Ok(example) => {
            ui.add(egui::Label::new(theme::caption(ui, format!("For example: {example}"))).wrap());
        }
        Err(problem) => {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(problem.to_string()).color(palette.warning_text),
                )
                .wrap(),
            );
            ui.add(
                egui::Label::new(theme::caption(
                    ui,
                    format!("{SCENE_NAME_REFUSED_NOTE} {}", in_use.example()),
                ))
                .wrap(),
            );
            // A box left with something unusable in it would go on showing
            // a name that is not the one in use.
            if name_box.lost_focus() {
                draft.show_again(*in_use);
            }
        }
    }
}

/// Whether the window is light, dark, or as the system is, in the row the
/// caller has opened. The window changes as soon as one is picked.
pub fn show_theme_choice(ui: &mut egui::Ui, theme: &mut ThemeChoice) {
    ui.label(THEME_LABEL).on_hover_text(THEME_EXPLANATION);
    egui::ComboBox::from_id_salt(THEME_BOX_ID)
        .selected_text(theme.label())
        .show_ui(ui, |ui| {
            for choice in ThemeChoice::ALL {
                ui.selectable_value(theme, choice, choice.label());
            }
        });
}

/// How much the slow scan detects and the shortest scene it may leave, in
/// the row or wrapping row the caller has opened. `id_salt` tells the
/// control of one place apart from the same control in another.
pub fn show_hard_cut_controls(
    ui: &mut egui::Ui,
    hard_cuts: &mut HardCutSettings,
    id_salt: &'static str,
) {
    ui.label("Detect");
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(hard_cuts.sensitivity.label())
        .show_ui(ui, |ui| {
            for sensitivity in Sensitivity::ALL {
                ui.selectable_value(&mut hard_cuts.sensitivity, sensitivity, sensitivity.label());
            }
        });

    ui.label("and ignore scenes shorter than");
    let mut minutes = hard_cuts.min_scene_seconds / SECONDS_PER_MINUTE;
    let shortest_scene = egui::DragValue::new(&mut minutes)
        .range(SHORTEST_SCENE_MINUTES)
        .speed(0.1)
        .fixed_decimals(1)
        .suffix(" min");
    if ui.add(shortest_scene).changed() {
        hard_cuts.min_scene_seconds = minutes * SECONDS_PER_MINUTE;
    }
}

/// What counts as a black screen, and how much of the video is looked at to
/// find them. The sidebar shows these for the general settings and a review
/// screen for the settings its video is scanned with.
pub fn show_black_screen_controls(ui: &mut egui::Ui, scan: &mut ScanSettings) {
    let criteria = &mut scan.criteria;
    ui.horizontal(|ui| {
        ui.label("Shortest black screen");
        ui.add(
            egui::DragValue::new(&mut criteria.min_seconds)
                .range(SHORTEST_BLACK_SECONDS)
                .speed(0.01)
                .fixed_decimals(2)
                .suffix(" s"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("How dark counts as black").on_hover_text(
            "0 is pure black, 255 is white. Raise this if dark grey screens are missed.",
        );
        ui.add(egui::DragValue::new(&mut criteria.max_dark_luma));
    });
    ui.horizontal(|ui| {
        ui.label("How much of the picture must be black")
            .on_hover_text("A logo in a corner is why this is not 100%.");
        let mut percent = criteria.min_dark_fraction * PERCENT;
        let share = egui::DragValue::new(&mut percent)
            .range(BLACK_SHARE_PERCENT)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" %");
        if ui.add(share).changed() {
            criteria.min_dark_fraction = percent / PERCENT;
        }
    });
    let mut every_frame = scan.mode == ScanMode::EveryFrame;
    let every_frame_box = ui
        .checkbox(&mut every_frame, "Check every picture (much slower)")
        .on_hover_text(
            "Use this if black screens are being missed, for example in grainy footage.",
        );
    if every_frame_box.changed() {
        scan.mode = if every_frame {
            ScanMode::EveryFrame
        } else {
            ScanMode::Suspicious
        };
    }
}

/// The checkbox for moving the original to the Recycle Bin or Trash once
/// the scenes are verified. Each place words it for its surroundings.
pub fn show_original_policy_checkbox(
    ui: &mut egui::Ui,
    policy: &mut OriginalPolicy,
    label: String,
) -> egui::Response {
    let mut recycle = is_recycling(*policy);
    let checkbox = ui.checkbox(&mut recycle, label);
    if checkbox.changed() {
        *policy = original_policy_for(recycle);
    }
    checkbox
}

pub fn show_scene_location_checkbox(
    ui: &mut egui::Ui,
    location: &mut SceneLocation,
) -> egui::Response {
    let mut new_folder = is_new_folder(*location);
    let checkbox = ui.checkbox(&mut new_folder, "Put the scenes in a new folder");
    if checkbox.changed() {
        *location = scene_location_for(new_folder);
    }
    checkbox
}

/// The setting for cutting on the exact picture, as the window names it.
/// The message of a failed exact cut names it the same way.
pub const EXACT_CUT_LABEL: &str = EXACT_CUT_SETTING;
const EXACT_CUT_EXPLANATION: &str =
    "Normally a video can only be cut at certain points. With this on, a split \
     is made on the very picture you choose, and a few seconds around each cut are \
     encoded again. That takes longer, and the original is always kept.";

pub fn show_cut_precision_checkbox(
    ui: &mut egui::Ui,
    precision: &mut CutPrecision,
) -> egui::Response {
    let mut exact = is_exact(*precision);
    let checkbox = ui
        .checkbox(&mut exact, EXACT_CUT_LABEL)
        .on_hover_text(EXACT_CUT_EXPLANATION);
    if checkbox.changed() {
        *precision = cut_precision_for(exact);
    }
    checkbox
}

fn is_exact(precision: CutPrecision) -> bool {
    match precision {
        CutPrecision::ExactFrame => true,
        CutPrecision::NearestKeyframe => false,
    }
}

/// What a ticked or unticked exact-cut checkbox means.
fn cut_precision_for(ticked: bool) -> CutPrecision {
    if ticked {
        CutPrecision::ExactFrame
    } else {
        CutPrecision::NearestKeyframe
    }
}

fn is_recycling(policy: OriginalPolicy) -> bool {
    match policy {
        OriginalPolicy::RecycleOnceVerified => true,
        OriginalPolicy::Keep => false,
    }
}

/// What a ticked or unticked recycling checkbox means.
fn original_policy_for(ticked: bool) -> OriginalPolicy {
    if ticked {
        OriginalPolicy::RecycleOnceVerified
    } else {
        OriginalPolicy::Keep
    }
}

fn is_new_folder(location: SceneLocation) -> bool {
    match location {
        SceneLocation::NewFolder => true,
        SceneLocation::BesideVideo => false,
    }
}

/// What a ticked or unticked new-folder checkbox means.
fn scene_location_for(ticked: bool) -> SceneLocation {
    if ticked {
        SceneLocation::NewFolder
    } else {
        SceneLocation::BesideVideo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkbox_shows_the_choice_it_would_set() {
        for policy in [OriginalPolicy::Keep, OriginalPolicy::RecycleOnceVerified] {
            assert_eq!(original_policy_for(is_recycling(policy)), policy);
        }
        for location in [SceneLocation::BesideVideo, SceneLocation::NewFolder] {
            assert_eq!(scene_location_for(is_new_folder(location)), location);
        }
        for precision in [CutPrecision::NearestKeyframe, CutPrecision::ExactFrame] {
            assert_eq!(cut_precision_for(is_exact(precision)), precision);
        }
    }

    #[test]
    fn a_ticked_box_recycles_and_an_unticked_one_keeps() {
        assert_eq!(
            original_policy_for(true),
            OriginalPolicy::RecycleOnceVerified
        );
        assert_eq!(original_policy_for(false), OriginalPolicy::Keep);
    }

    fn pattern(typed: &str) -> SceneNamePattern {
        SceneNamePattern::new(typed).unwrap()
    }

    #[test]
    fn the_name_box_starts_with_the_pattern_in_use_and_an_example_of_it() {
        let draft = SceneNameDraft::of(SceneNamePattern::default());
        assert_eq!(draft.typed, "{name}_scene_{number}_of_{count}");
        assert_eq!(
            draft.outcome(),
            Ok("My video_scene_02_of_12.mp4".to_owned())
        );
    }

    #[test]
    fn a_usable_pattern_is_in_use_as_soon_as_it_is_typed() {
        let mut in_use = SceneNamePattern::default();
        let mut draft = SceneNameDraft::of(in_use);

        draft.take_typed("{name} part {number}".to_owned(), &mut in_use);
        assert_eq!(in_use, pattern("{name} part {number}"));
        assert_eq!(draft.outcome(), Ok("My video part 002.mp4".to_owned()));
    }

    #[test]
    fn an_unusable_pattern_is_explained_and_leaves_the_one_in_use() {
        let mut in_use = SceneNamePattern::default();
        let mut draft = SceneNameDraft::of(in_use);

        draft.take_typed("{name} part {numb".to_owned(), &mut in_use);
        assert_eq!(in_use, SceneNamePattern::default());
        assert_eq!(draft.typed, "{name} part {numb");
        assert_eq!(draft.outcome(), Err(PatternError::StrayBrace));

        draft.show_again(in_use);
        assert_eq!(draft, SceneNameDraft::of(SceneNamePattern::default()));
    }

    #[test]
    fn the_hint_names_every_placeholder() {
        let hint = scene_name_hint();
        for placeholder in Placeholder::ALL {
            assert!(hint.contains(&placeholder.typed()), "{hint}");
        }
    }

    /// Draws the name box once, with the given events, as the sidebar would.
    fn draw_name_box(
        ctx: &egui::Context,
        draft: &mut SceneNameDraft,
        in_use: &mut SceneNamePattern,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(270.0, 600.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_scene_name_box(ui, draft, in_use));
        });
    }

    #[test]
    fn the_name_box_follows_a_pattern_that_was_changed_elsewhere() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut in_use = pattern("{name} part {number}");
        let mut draft = SceneNameDraft::of(in_use);
        draw_name_box(&ctx, &mut draft, &mut in_use);
        assert_eq!(draft.typed, "{name} part {number}");

        // "Restore default settings" was pressed.
        in_use = SceneNamePattern::default();
        draw_name_box(&ctx, &mut draft, &mut in_use);
        assert_eq!(draft, SceneNameDraft::of(SceneNamePattern::default()));
        assert_eq!(in_use, SceneNamePattern::default());
    }

    #[test]
    fn an_unticked_box_cuts_without_encoding() {
        assert_eq!(cut_precision_for(false), CutPrecision::default());
        assert_eq!(cut_precision_for(true), CutPrecision::ExactFrame);
    }

    #[test]
    fn the_exact_cut_texts_say_what_is_encoded_and_that_the_original_stays() {
        assert!(EXACT_CUT_EXPLANATION.contains("encoded"));
        assert!(EXACT_CUT_EXPLANATION.contains("the original is always kept"));
    }

    #[test]
    fn the_shortest_scene_on_offer_fits_the_default() {
        let default_minutes = HardCutSettings::default().min_scene_seconds / SECONDS_PER_MINUTE;
        assert!(SHORTEST_SCENE_MINUTES.contains(&default_minutes));
    }
}
