//! The controls for settings that are offered in more than one place: in the
//! sidebar's Settings section and on the review screen. Each is drawn here
//! once, so its range and its meaning cannot drift apart between the two.

use eframe::egui;

use crate::detect::{ScanMode, ScanSettings};
use crate::export::OriginalPolicy;
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::split::SceneLocation;
use crate::timecode::SECONDS_PER_MINUTE;

/// The range offered for the shortest scene a hard-cut scan may produce.
const SHORTEST_SCENE_MINUTES: std::ops::RangeInclusive<f64> = 0.5..=60.0;
/// The ranges offered for what counts as a black screen.
const SHORTEST_BLACK_SECONDS: std::ops::RangeInclusive<f64> = 0.05..=10.0;
const BLACK_SHARE_PERCENT: std::ops::RangeInclusive<f64> = 50.0..=100.0;
const PERCENT: f64 = 100.0;

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
    fn the_shortest_scene_on_offer_fits_the_default() {
        let default_minutes = HardCutSettings::default().min_scene_seconds / SECONDS_PER_MINUTE;
        assert!(SHORTEST_SCENE_MINUTES.contains(&default_minutes));
    }
}
