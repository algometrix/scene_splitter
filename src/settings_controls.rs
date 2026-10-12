//! The controls for settings that are offered in more than one place: in the
//! sidebar's Settings section and on the review screen. Each is drawn here
//! once, so its range and its meaning cannot drift apart between the two.

use eframe::egui;

use crate::exact_cut::CutPrecision;
use crate::export::OriginalPolicy;
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::split::SceneLocation;
use crate::timecode::SECONDS_PER_MINUTE;

/// The range offered for the shortest scene a hard-cut scan may produce.
const SHORTEST_SCENE_MINUTES: std::ops::RangeInclusive<f64> = 0.5..=60.0;

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
pub const EXACT_CUT_LABEL: &str = "Cut on the exact picture (slower)";
const EXACT_CUT_EXPLANATION: &str =
    "A video can only be cut at certain points without encoding. With this on, a split \
     is made on the very picture you choose, and a few seconds around each cut are \
     encoded again. That takes longer, and the original is always kept.";
/// Shown wherever the choice about the original is offered while exact cuts
/// are on, in place of that choice.
pub const ORIGINAL_KEPT_WITH_EXACT_CUTS: &str =
    "Cuts are made on the exact picture. A few seconds around each cut are encoded \
     again, and the original is always kept.";

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

    #[test]
    fn an_unticked_box_cuts_without_encoding() {
        assert_eq!(cut_precision_for(false), CutPrecision::default());
        assert_eq!(cut_precision_for(true), CutPrecision::ExactFrame);
    }

    #[test]
    fn the_exact_cut_texts_say_what_is_encoded_and_that_the_original_stays() {
        for text in [EXACT_CUT_EXPLANATION, ORIGINAL_KEPT_WITH_EXACT_CUTS] {
            assert!(text.contains("encoded"), "{text}");
            assert!(text.contains("the original is always kept"), "{text}");
            assert!(!text.contains('\u{2014}'), "{text}");
        }
    }

    #[test]
    fn the_shortest_scene_on_offer_fits_the_default() {
        let default_minutes = HardCutSettings::default().min_scene_seconds / SECONDS_PER_MINUTE;
        assert!(SHORTEST_SCENE_MINUTES.contains(&default_minutes));
    }
}
