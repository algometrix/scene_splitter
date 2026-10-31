//! The controls for settings that are offered in more than one place: in the
//! sidebar's Settings section and on the review screen. Each is drawn here
//! once, so its range and its meaning cannot drift apart between the two.

use eframe::egui;

use crate::detect::{ScanMode, ScanSettings};
use crate::exact_cut::{CutPrecision, EXACT_CUT_SETTING};
use crate::export::OriginalPolicy;
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::scene_length::{SceneLength, SceneLengthLimits};
use crate::scene_name::{PatternError, Placeholder, SceneNamePattern};
use crate::split::SceneLocation;
use crate::theme::{self, Style, ThemeChoice};
use crate::timecode::{format_length, parse_timestamp, SECONDS_PER_MINUTE};

/// The range offered for the shortest scene a hard-cut scan may produce.
const SHORTEST_SCENE_MINUTES: std::ops::RangeInclusive<f64> = 0.5..=60.0;
/// The ranges offered for what counts as a black screen.
const SHORTEST_BLACK_SECONDS: std::ops::RangeInclusive<f64> = 0.05..=10.0;
const BLACK_SHARE_PERCENT: std::ops::RangeInclusive<f64> = 50.0..=100.0;
const PERCENT: f64 = 100.0;
/// The scene lengths on offer, in seconds: from one second to ten hours.
/// The top is far above any film, and only there so that a slip of the hand
/// cannot drag a length out of sight.
const SCENE_LENGTH_SECONDS: std::ops::RangeInclusive<f64> = 1.0..=36_000.0;
/// What a limit is set to when its box is ticked: short enough to keep real
/// scenes, long enough to merge title cards away.
const SHORTEST_SCENE_WHEN_TICKED: f64 = SECONDS_PER_MINUTE;
/// What the other limit is set to when its box is ticked.
const LONGEST_SCENE_WHEN_TICKED: f64 = 20.0 * SECONDS_PER_MINUTE;
/// How many seconds a limit changes for each point its number is dragged.
/// The longest scene is counted in minutes, so it moves faster.
const SHORTEST_SCENE_DRAG_SPEED: f64 = 1.0;
const LONGEST_SCENE_DRAG_SPEED: f64 = 10.0;

pub const SCENE_LENGTHS_HEADING: &str = "How long a scene may be";
const SHORTEST_SCENE_LABEL: &str = "No scene shorter than";
const SHORTEST_SCENE_EXPLANATION: &str =
    "Splits are left out until no scene is shorter than this. The splits at the longest \
     black screens stay. Use it for videos with many black screens close together, \
     such as title cards.";
const LONGEST_SCENE_LABEL: &str = "No scene longer than";
const LONGEST_SCENE_EXPLANATION: &str =
    "A scene longer than this is cut into equal parts, at the nearest points where the \
     video can be cut.";
const SCENE_LENGTH_HINT: &str = "Drag, or type a time like 1:30.";
const HARD_CUT_SHORTEST_SCENE_EXPLANATION: &str =
    "Only for sudden picture changes, which films have every few seconds. \
     Splits at black screens are not thinned by this.";

/// Every choice and number box of a setting row is this wide, so their
/// edges line up down a group whatever the names beside them.
pub const SETTING_CONTROL_WIDTH: f32 = 140.0;
/// Between the name of a setting and its control.
const SETTING_GAP: f32 = 16.0;

const HARD_CUT_SENSITIVITY_LABEL: &str = "Detect";
const HARD_CUT_SHORTEST_SCENE_LABEL: &str = "Ignore scenes shorter than";
const SHORTEST_BLACK_LABEL: &str = "Shortest black screen";
const DARKNESS_LABEL: &str = "How dark counts as black";
const DARKNESS_EXPLANATION: &str =
    "0 is pure black, 255 is white. Raise this if dark grey screens are missed.";
const BLACK_SHARE_LABEL: &str = "How much of the picture must be black";
const BLACK_SHARE_EXPLANATION: &str = "A logo in a corner is why this is not 100%.";
const EVERY_FRAME_LABEL: &str = "Check every picture (much slower)";
const EVERY_FRAME_EXPLANATION: &str =
    "Use this if black screens are being missed, for example in grainy footage.";
const NEW_FOLDER_LABEL: &str = "Put the scenes in a new folder";

const THEME_LABEL: &str = "Colours";
/// What the check before recycling does. It reads the new files; it does
/// not compare them with the original (see verify.rs).
pub const RECYCLE_CHECK_EXPLANATION: &str = "The new files are checked first: each must read to \
     its end and play. If one does not, the original stays where it is.";
const STYLE_LABEL: &str = "Style";
const STYLE_EXPLANATION: &str = "How the window looks and how the review screen is laid out. \
     Editor puts the player beside a table of the scenes and a film strip. Soft shows every \
     scene as a card with its pictures. Studio puts a large player over a strip of the scenes.";
const STYLE_BOX_ID: &str = "settings_style";

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

/// One setting on a row of its own: its name on the left, wrapping when
/// the row is narrow, and its control at the right edge. Every row of a
/// group puts its control in the same place, so the controls line up.
/// Returns the name and the control as one response, so a hover on either
/// explains the setting.
pub fn setting_row(
    ui: &mut egui::Ui,
    label: &str,
    add_control: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.set_min_height(theme::CONTROL_HEIGHT);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let control = add_control(ui);
            ui.add_space(SETTING_GAP);
            let name_column = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true);
            let name = ui
                .with_layout(name_column, |ui| ui.add(egui::Label::new(label).wrap()))
                .inner;
            name.union(control)
        })
        .inner
    })
    .inner
}

/// A setting that is on or off, as a row with a switch at its end.
fn toggle_row(ui: &mut egui::Ui, label: &str, on: &mut bool) -> egui::Response {
    setting_row(ui, label, |ui| ui.add(theme::Toggle::new(on, label)))
}

/// A choice from a list, as wide as every control of a setting row.
fn choice_box<Choice: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    chosen: &mut Choice,
    choices: impl IntoIterator<Item = Choice>,
    name_of: impl Fn(Choice) -> &'static str,
) -> egui::Response {
    let mut changed = false;
    let mut response = egui::ComboBox::from_id_salt(id_salt)
        .width(SETTING_CONTROL_WIDTH)
        .selected_text(name_of(*chosen))
        .show_ui(ui, |ui| {
            for choice in choices {
                changed |= ui
                    .selectable_value(chosen, choice, name_of(choice))
                    .changed();
            }
        })
        .response;
    if changed {
        response.mark_changed();
    }
    response
}

/// A number box as wide as every control of a setting row.
fn number_box(ui: &mut egui::Ui, number: egui::DragValue<'_>) -> egui::Response {
    ui.add_sized(
        egui::vec2(SETTING_CONTROL_WIDTH, theme::CONTROL_HEIGHT),
        number,
    )
}

/// How the window looks. The window changes as soon as one is picked.
pub fn show_style_choice(ui: &mut egui::Ui, style: &mut Style) {
    setting_row(ui, STYLE_LABEL, |ui| {
        choice_box(ui, STYLE_BOX_ID, style, Style::ALL, Style::label)
    })
    .on_hover_text(STYLE_EXPLANATION);
}

/// Whether the window is light, dark, or as the system is. The window
/// changes as soon as one is picked.
pub fn show_theme_choice(ui: &mut egui::Ui, theme: &mut ThemeChoice) {
    setting_row(ui, THEME_LABEL, |ui| {
        choice_box(
            ui,
            THEME_BOX_ID,
            theme,
            ThemeChoice::ALL,
            ThemeChoice::label,
        )
    })
    .on_hover_text(THEME_EXPLANATION);
}

/// How much the slow scan detects and the shortest scene it may leave, a
/// row each. `id_salt` tells the control of one place apart from the same
/// control in another.
pub fn show_hard_cut_controls(
    ui: &mut egui::Ui,
    hard_cuts: &mut HardCutSettings,
    id_salt: &'static str,
) {
    setting_row(ui, HARD_CUT_SENSITIVITY_LABEL, |ui| {
        choice_box(
            ui,
            id_salt,
            &mut hard_cuts.sensitivity,
            Sensitivity::ALL,
            Sensitivity::label,
        )
    });
    let mut minutes = hard_cuts.min_scene_seconds / SECONDS_PER_MINUTE;
    let shortest_scene = setting_row(ui, HARD_CUT_SHORTEST_SCENE_LABEL, |ui| {
        let number = egui::DragValue::new(&mut minutes)
            .range(SHORTEST_SCENE_MINUTES)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" min");
        number_box(ui, number)
    })
    .on_hover_text(HARD_CUT_SHORTEST_SCENE_EXPLANATION);
    if shortest_scene.changed() {
        hard_cuts.min_scene_seconds = minutes * SECONDS_PER_MINUTE;
    }
}

/// The shortest and the longest a scene may be, each with a box to tick,
/// so that leaving a limit off is one click. The sidebar shows these for
/// the general settings and a review screen for the settings of its video.
///
/// A limit can only be dragged or typed as far as the other one allows, so
/// what stands here is always a pair no scene is shut out by.
pub fn show_scene_length_controls(ui: &mut egui::Ui, limits: &mut SceneLengthLimits) {
    let shortest = show_scene_length_limit(
        ui,
        LimitControl {
            label: SHORTEST_SCENE_LABEL,
            explanation: SHORTEST_SCENE_EXPLANATION,
            when_ticked: SHORTEST_SCENE_WHEN_TICKED,
            drag_speed: SHORTEST_SCENE_DRAG_SPEED,
        },
        limits.shortest(),
        *SCENE_LENGTH_SECONDS.start()..=seconds_or(limits.longest(), *SCENE_LENGTH_SECONDS.end()),
    );
    // Taken over at once, so that the other limit is held to it in the same
    // frame. A pair that cannot be leaves the limits as they were.
    if let Ok(changed) = limits.with_shortest(shortest) {
        *limits = changed;
    }
    let longest = show_scene_length_limit(
        ui,
        LimitControl {
            label: LONGEST_SCENE_LABEL,
            explanation: LONGEST_SCENE_EXPLANATION,
            when_ticked: LONGEST_SCENE_WHEN_TICKED,
            drag_speed: LONGEST_SCENE_DRAG_SPEED,
        },
        limits.longest(),
        seconds_or(limits.shortest(), *SCENE_LENGTH_SECONDS.start())..=*SCENE_LENGTH_SECONDS.end(),
    );
    if let Ok(changed) = limits.with_longest(longest) {
        *limits = changed;
    }
}

fn seconds_or(limit: Option<SceneLength>, otherwise: f64) -> f64 {
    limit.map_or(otherwise, SceneLength::seconds)
}

/// How one of the two limits is offered.
#[derive(Clone, Copy)]
struct LimitControl {
    label: &'static str,
    explanation: &'static str,
    /// The length, in seconds, the limit starts at when its box is ticked.
    when_ticked: f64,
    drag_speed: f64,
}

/// One limit: a box to tick and, while it is ticked, the length. Returns
/// the limit as the person left it, within `allowed_seconds`.
fn show_scene_length_limit(
    ui: &mut egui::Ui,
    control: LimitControl,
    limit: Option<SceneLength>,
    allowed_seconds: std::ops::RangeInclusive<f64>,
) -> Option<SceneLength> {
    let mut wanted = limit;
    // The switch at the end of the row, and the length before it while the
    // limit is on, so the switches line up with those of other rows.
    setting_row(ui, control.label, |ui| {
        let mut is_set = limit.is_some();
        let switch = ui.add(theme::Toggle::new(&mut is_set, control.label));
        if switch.changed() {
            wanted = is_set
                .then(|| scene_length_within(control.when_ticked, &allowed_seconds))
                .flatten();
        }
        let Some(length) = wanted else {
            return switch;
        };
        ui.add_space(SETTING_GAP / 2.0);
        let mut seconds = length.seconds();
        let number = egui::DragValue::new(&mut seconds)
            .range(allowed_seconds.clone())
            .speed(control.drag_speed)
            .custom_formatter(|seconds, _| format_length(seconds))
            .custom_parser(parse_timestamp);
        let number = number_box(ui, number).on_hover_text(SCENE_LENGTH_HINT);
        if number.changed() {
            wanted = scene_length_within(seconds, &allowed_seconds).or(wanted);
        }
        switch.union(number)
    })
    .on_hover_text(control.explanation);
    wanted
}

/// The length nearest to `seconds` that the other limit allows.
fn scene_length_within(
    seconds: f64,
    allowed_seconds: &std::ops::RangeInclusive<f64>,
) -> Option<SceneLength> {
    let within = seconds.clamp(*allowed_seconds.start(), *allowed_seconds.end());
    SceneLength::from_seconds(within).ok()
}

/// What counts as a black screen, and how much of the video is looked at to
/// find them. The sidebar shows these for the general settings and a review
/// screen for the settings its video is scanned with.
pub fn show_black_screen_controls(ui: &mut egui::Ui, scan: &mut ScanSettings) {
    let criteria = &mut scan.criteria;
    setting_row(ui, SHORTEST_BLACK_LABEL, |ui| {
        let number = egui::DragValue::new(&mut criteria.min_seconds)
            .range(SHORTEST_BLACK_SECONDS)
            .speed(0.01)
            .fixed_decimals(2)
            .suffix(" s");
        number_box(ui, number)
    });
    setting_row(ui, DARKNESS_LABEL, |ui| {
        number_box(ui, egui::DragValue::new(&mut criteria.max_dark_luma))
    })
    .on_hover_text(DARKNESS_EXPLANATION);
    let mut percent = criteria.min_dark_fraction * PERCENT;
    let share = setting_row(ui, BLACK_SHARE_LABEL, |ui| {
        let number = egui::DragValue::new(&mut percent)
            .range(BLACK_SHARE_PERCENT)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" %");
        number_box(ui, number)
    })
    .on_hover_text(BLACK_SHARE_EXPLANATION);
    if share.changed() {
        criteria.min_dark_fraction = percent / PERCENT;
    }
    let mut every_frame = scan.mode == ScanMode::EveryFrame;
    let every_frame_switch =
        toggle_row(ui, EVERY_FRAME_LABEL, &mut every_frame).on_hover_text(EVERY_FRAME_EXPLANATION);
    if every_frame_switch.changed() {
        scan.mode = if every_frame {
            ScanMode::EveryFrame
        } else {
            ScanMode::Suspicious
        };
    }
}

/// The switch for moving the original to the Recycle Bin or Trash once
/// the scenes are verified. Each place words it for its surroundings.
pub fn show_original_policy_setting(
    ui: &mut egui::Ui,
    policy: &mut OriginalPolicy,
    label: &str,
) -> egui::Response {
    let mut recycle = is_recycling(*policy);
    let switch = toggle_row(ui, label, &mut recycle);
    if switch.changed() {
        *policy = original_policy_for(recycle);
    }
    switch
}

pub fn show_scene_location_setting(
    ui: &mut egui::Ui,
    location: &mut SceneLocation,
) -> egui::Response {
    let mut new_folder = is_new_folder(*location);
    let switch = toggle_row(ui, NEW_FOLDER_LABEL, &mut new_folder);
    if switch.changed() {
        *location = scene_location_for(new_folder);
    }
    switch
}

/// The setting for cutting on the exact picture, as the window names it.
/// The message of a failed exact cut names it the same way.
pub const EXACT_CUT_LABEL: &str = EXACT_CUT_SETTING;
const EXACT_CUT_EXPLANATION: &str =
    "Normally a video can only be cut at certain points. With this on, a split \
     is made on the very picture you choose, and a few seconds around each cut are \
     encoded again. That takes longer, and the original is always kept. A video \
     that cannot be cut that way is cut at the nearest points instead, and the \
     result says so.";

pub fn show_cut_precision_setting(
    ui: &mut egui::Ui,
    precision: &mut CutPrecision,
) -> egui::Response {
    let mut exact = is_exact(*precision);
    let switch = toggle_row(ui, EXACT_CUT_LABEL, &mut exact).on_hover_text(EXACT_CUT_EXPLANATION);
    if switch.changed() {
        *precision = cut_precision_for(exact);
    }
    switch
}

/// A setting that is on or off and lives outside these shared controls, in
/// the same row as theirs.
pub fn show_switch_setting(ui: &mut egui::Ui, label: &str, on: &mut bool) -> egui::Response {
    toggle_row(ui, label, on)
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

    /// Draws the scene length controls once, as the sidebar would.
    fn draw_scene_lengths(ctx: &egui::Context, limits: &mut SceneLengthLimits) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(270.0, 600.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_scene_length_controls(ui, limits));
        });
    }

    fn length(seconds: f64) -> SceneLength {
        SceneLength::from_seconds(seconds).unwrap()
    }

    #[test]
    fn drawing_the_scene_lengths_changes_none_of_them() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        for as_set in [
            SceneLengthLimits::NONE,
            SceneLengthLimits::new(Some(length(45.0)), None).unwrap(),
            SceneLengthLimits::new(Some(length(60.0)), Some(length(1200.0))).unwrap(),
        ] {
            let mut limits = as_set;
            draw_scene_lengths(&ctx, &mut limits);
            assert_eq!(limits, as_set);
        }
    }

    #[test]
    fn a_ticked_limit_starts_where_the_other_limit_allows() {
        let any = SCENE_LENGTH_SECONDS;
        assert_eq!(
            scene_length_within(SHORTEST_SCENE_WHEN_TICKED, &any),
            Some(length(60.0))
        );
        // The longest scene is five minutes, so the shortest starts there
        // or below. And the other way around.
        assert_eq!(
            scene_length_within(SHORTEST_SCENE_WHEN_TICKED, &(1.0..=30.0)),
            Some(length(30.0))
        );
        assert_eq!(
            scene_length_within(LONGEST_SCENE_WHEN_TICKED, &(3600.0..=36_000.0)),
            Some(length(3600.0))
        );
    }

    #[test]
    fn the_scene_lengths_set_by_a_tick_are_on_offer() {
        assert!(SCENE_LENGTH_SECONDS.contains(&SHORTEST_SCENE_WHEN_TICKED));
        assert!(SCENE_LENGTH_SECONDS.contains(&LONGEST_SCENE_WHEN_TICKED));
    }

    #[test]
    fn a_scene_length_reads_as_people_say_it_and_is_typed_as_a_time() {
        assert_eq!(format_length(90.0), "1 min 30 s");
        assert_eq!(parse_timestamp("1:30"), Some(90.0));
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
        assert!(EXACT_CUT_EXPLANATION.contains("cut at the nearest points instead"));
    }

    #[test]
    fn the_shortest_scene_on_offer_fits_the_default() {
        let default_minutes = HardCutSettings::default().min_scene_seconds / SECONDS_PER_MINUTE;
        assert!(SHORTEST_SCENE_MINUTES.contains(&default_minutes));
    }
}
