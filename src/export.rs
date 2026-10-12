//! Writes the scenes of one video, verifies them, and only then removes the
//! original if the person asked for that.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::exact_cut::CutPrecision;
use crate::exact_split;
use crate::exact_verify::ExactCutProof;
use crate::scenes::CutTimes;
use crate::split::{self, SceneLocation};
use crate::timecode::format_timestamp;
use crate::verify;

/// What each system calls the place deleted files go to.
pub const TRASH_NAME: &str = if cfg!(windows) {
    "Recycle Bin"
} else {
    "Trash"
};

/// What to do with the original video after its scenes are written.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OriginalPolicy {
    Keep,
    /// Move it to the Recycle Bin or Trash, but only once the scenes are
    /// verified.
    RecycleOnceVerified,
}

/// The choices a person makes about how scenes are written.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
    pub scene_location: SceneLocation,
    pub original_policy: OriginalPolicy,
    /// Missing from settings saved before the choice existed, which then
    /// cut on keyframes as they always did.
    pub cut_precision: CutPrecision,
}

impl Default for ExportOptions {
    /// Scenes go beside the video, cuts go on keyframes, and the original is
    /// recycled once the scenes are verified, so the scenes take its place.
    fn default() -> Self {
        Self {
            scene_location: SceneLocation::BesideVideo,
            original_policy: OriginalPolicy::RecycleOnceVerified,
            cut_precision: CutPrecision::NearestKeyframe,
        }
    }
}

impl ExportOptions {
    /// How a split with these options is carried out.
    pub fn split_method(&self) -> SplitMethod {
        match self.cut_precision {
            CutPrecision::NearestKeyframe => SplitMethod::CopyAtKeyframes {
                original_policy: self.original_policy,
            },
            CutPrecision::ExactFrame => SplitMethod::ExactFrames,
        }
    }
}

/// How the scenes of one split are written, and with it what may happen to
/// the original afterwards.
///
/// Only a split that copies every packet can be compared with the original
/// packet for packet, so only that kind carries a policy for the original.
/// An exact split encodes the pictures around each cut again. It has no
/// policy to carry, and so no way to ask for the original to be recycled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitMethod {
    CopyAtKeyframes { original_policy: OriginalPolicy },
    ExactFrames,
}

impl SplitMethod {
    /// What happens to the original when the scenes pass their check.
    pub fn original_policy(self) -> OriginalPolicy {
        match self {
            Self::CopyAtKeyframes { original_policy } => original_policy,
            Self::ExactFrames => OriginalPolicy::Keep,
        }
    }
}

#[derive(Debug)]
pub enum Verification {
    /// The scenes hold every packet of the original, unchanged.
    Passed,
    /// The scenes of an exact split passed the checks that can be made of
    /// them. Scenes that do not are never put in place.
    PassedForExactCut(ExactCutProof),
    Failed(String),
}

#[derive(Debug)]
pub enum OriginalOutcome {
    Kept,
    /// The original of an exact split is kept whatever the settings say.
    KeptBecauseCutIsExact,
    MovedToRecycleBin,
    KeptBecauseUnverified,
    KeptBecauseRecyclingFailed(String),
}

#[derive(Debug)]
pub struct ExportReport {
    pub scene_files: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub verification: Verification,
    pub original: OriginalOutcome,
}

impl ExportReport {
    /// True when the scenes are verified and nothing went wrong afterwards.
    pub fn is_clean(&self) -> bool {
        matches!(
            self.verification,
            Verification::Passed | Verification::PassedForExactCut(_)
        ) && matches!(
            self.original,
            OriginalOutcome::Kept
                | OriginalOutcome::KeptBecauseCutIsExact
                | OriginalOutcome::MovedToRecycleBin
        )
    }

    /// What the check of an exact split measured, one line per fact, for a
    /// run from the command line. `None` after any other split.
    pub fn exact_cut_measurements(&self) -> Option<String> {
        let Verification::PassedForExactCut(proof) = &self.verification else {
            return None;
        };
        let mut lines = vec![
            format!(
                "Pictures: {} in the original and in the scenes together, {} of them encoded again.",
                proof.pictures, proof.pictures_encoded_again
            ),
            format!(
                "Sound: {} packets, all copied unchanged. Largest shift against the picture: {:.4} s.",
                proof.sound_packets, proof.largest_sound_shift_seconds
            ),
        ];
        for (position, landing) in proof.cut_landings.iter().enumerate() {
            lines.push(format!(
                "Cut {}: asked for at {}, the next scene starts at {} ({:.3} s later).",
                position + 1,
                format_timestamp(landing.asked_for),
                format_timestamp(landing.landed_at),
                // A landing a rounding step early is no earlier than asked.
                (landing.landed_at - landing.asked_for).max(0.0)
            ));
        }
        Some(lines.join("\n"))
    }

    /// What happened, in words a person who knows nothing of video can act
    /// on, with the technical reason after each thing that went wrong.
    pub fn summary(&self) -> String {
        let with_details = |sentence: String, details: Option<&str>| match details {
            Some(details) => format!("{sentence}\n\nDetails: {details}"),
            None => sentence,
        };
        format!(
            "{}\n\n{}\n\n{}",
            self.where_the_scenes_are(),
            with_details(self.check_in_plain_words(), self.verification_details()),
            with_details(self.original_in_plain_words(), self.recycling_details())
        )
    }

    /// The summary without the technical reasons, for a screen that shows
    /// those apart, where they can be copied.
    pub fn plain_summary(&self) -> String {
        format!(
            "{}\n\n{}\n\n{}",
            self.where_the_scenes_are(),
            self.check_in_plain_words(),
            self.original_in_plain_words()
        )
    }

    /// The technical reasons for whatever went wrong, or `None` after a
    /// split with nothing to report.
    pub fn details(&self) -> Option<String> {
        let details: Vec<&str> = [self.verification_details(), self.recycling_details()]
            .into_iter()
            .flatten()
            .collect();
        (!details.is_empty()).then(|| details.join("\n\n"))
    }

    fn where_the_scenes_are(&self) -> String {
        format!(
            "{}\n{}",
            counted_scene_files(self.scene_files.len()),
            self.out_dir.display()
        )
    }

    fn check_in_plain_words(&self) -> String {
        match &self.verification {
            Verification::Passed => {
                "Checked: together they contain the whole original, with nothing missing or changed."
                    .to_owned()
            }
            Verification::PassedForExactCut(proof) => format!(
                "Checked: every scene plays to its end, together they have all {} pictures of \
                 the original, each scene starts on the picture that was chosen, and the sound \
                 is the original's, unchanged. {} around the cuts were encoded again.",
                proof.pictures,
                counted_pictures(proof.pictures_encoded_again)
            ),
            Verification::Failed(_) => {
                "The check did not pass, so the original was kept. Play the scene files before you delete anything."
                    .to_owned()
            }
        }
    }

    fn verification_details(&self) -> Option<&str> {
        match &self.verification {
            Verification::Passed | Verification::PassedForExactCut(_) => None,
            Verification::Failed(reason) => Some(reason),
        }
    }

    fn original_in_plain_words(&self) -> String {
        match &self.original {
            OriginalOutcome::Kept | OriginalOutcome::KeptBecauseUnverified => {
                "The original is still in its place.".to_owned()
            }
            OriginalOutcome::KeptBecauseCutIsExact => ORIGINAL_KEPT_AFTER_EXACT_CUT.to_owned(),
            OriginalOutcome::MovedToRecycleBin => {
                format!("The original is in the {TRASH_NAME}. You can restore it from there.")
            }
            OriginalOutcome::KeptBecauseRecyclingFailed(_) => format!(
                "The scene files are fine. The original could not be moved to the {TRASH_NAME} and is still in its place."
            ),
        }
    }

    fn recycling_details(&self) -> Option<&str> {
        match &self.original {
            OriginalOutcome::KeptBecauseRecyclingFailed(reason) => Some(reason),
            OriginalOutcome::Kept
            | OriginalOutcome::KeptBecauseCutIsExact
            | OriginalOutcome::MovedToRecycleBin
            | OriginalOutcome::KeptBecauseUnverified => None,
        }
    }
}

/// "1 scene file is in", "3 scene files are in": the start of the sentence
/// that says where the scenes went.
fn counted_scene_files(count: usize) -> String {
    if count == 1 {
        "1 scene file is in".to_owned()
    } else {
        format!("{count} scene files are in")
    }
}

/// "1 picture", "50 pictures".
fn counted_pictures(count: usize) -> String {
    if count == 1 {
        "1 picture".to_owned()
    } else {
        format!("{count} pictures")
    }
}

/// Said in the result of every exact split, whatever the setting for
/// originals is.
const ORIGINAL_KEPT_AFTER_EXACT_CUT: &str =
    "The original is still in its place. It is always kept after an exact cut, because \
     pictures that were encoded again cannot be compared with it one for one.";

/// Splits the video the way `method` says. See [`export_scenes`] and
/// [`export_exact_scenes`].
pub fn export_scenes_as(
    video: &Path,
    cuts: &CutTimes,
    out_dir: &Path,
    method: SplitMethod,
) -> Result<ExportReport> {
    match method {
        SplitMethod::CopyAtKeyframes { original_policy } => {
            export_scenes(video, cuts, out_dir, original_policy)
        }
        SplitMethod::ExactFrames => export_exact_scenes(video, cuts, out_dir),
    }
}

/// Writes scenes that start on the exact pictures chosen, and checks them.
///
/// Takes no policy for the original: it is never recycled after an exact
/// split. Fails when the scenes could not be written or did not pass their
/// check. In both cases no scene file is put in place.
pub fn export_exact_scenes(video: &Path, cuts: &CutTimes, out_dir: &Path) -> Result<ExportReport> {
    let split = exact_split::split_on_exact_frames(video, cuts, out_dir)?;
    Ok(ExportReport {
        scene_files: split.scene_files,
        out_dir: out_dir.to_path_buf(),
        verification: Verification::PassedForExactCut(split.proof),
        original: OriginalOutcome::KeptBecauseCutIsExact,
    })
}

/// Fails only when the scenes could not be written. A failed verification is
/// reported in the result, and always leaves the original in place.
pub fn export_scenes(
    video: &Path,
    cuts: &CutTimes,
    out_dir: &Path,
    policy: OriginalPolicy,
) -> Result<ExportReport> {
    let scene_files = split::split_at_keyframes(video, cuts, out_dir)?;

    let verification = match verify::verify_scenes(video, &scene_files) {
        Ok(()) => Verification::Passed,
        Err(error) => Verification::Failed(format!("{error:#}")),
    };
    let original = match decide_about_original(policy, &verification) {
        OriginalDecision::Keep => OriginalOutcome::Kept,
        OriginalDecision::KeepBecauseUnverified => OriginalOutcome::KeptBecauseUnverified,
        OriginalDecision::Recycle => match trash::delete(video) {
            Ok(()) => OriginalOutcome::MovedToRecycleBin,
            Err(error) => OriginalOutcome::KeptBecauseRecyclingFailed(error.to_string()),
        },
    };

    Ok(ExportReport {
        scene_files,
        out_dir: out_dir.to_path_buf(),
        verification,
        original,
    })
}

#[derive(Debug, PartialEq)]
enum OriginalDecision {
    Keep,
    KeepBecauseUnverified,
    Recycle,
}

/// The one rule that guards the original: it is only recycled when the
/// person asked for that and the scenes were proven to hold every packet of
/// it. The check of an exact cut proves less than that, so it never counts.
fn decide_about_original(policy: OriginalPolicy, verification: &Verification) -> OriginalDecision {
    match (policy, verification) {
        (OriginalPolicy::Keep, _) | (_, Verification::PassedForExactCut(_)) => {
            OriginalDecision::Keep
        }
        (OriginalPolicy::RecycleOnceVerified, Verification::Failed(_)) => {
            OriginalDecision::KeepBecauseUnverified
        }
        (OriginalPolicy::RecycleOnceVerified, Verification::Passed) => OriginalDecision::Recycle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> Verification {
        Verification::Failed("packet 3 of stream 0 differs".to_owned())
    }

    fn report(verification: Verification, original: OriginalOutcome) -> ExportReport {
        ExportReport {
            scene_files: vec![PathBuf::from("film_scene_001.mp4")],
            out_dir: PathBuf::from("scenes"),
            verification,
            original,
        }
    }

    fn exact_cut_report() -> ExportReport {
        let proof = ExactCutProof {
            pictures: 225,
            pictures_encoded_again: 50,
            sound_packets: 389,
            cut_landings: vec![crate::exact_verify::CutLanding {
                asked_for: 2.9,
                landed_at: 2.92,
            }],
            largest_sound_shift_seconds: 0.0007,
        };
        report(
            Verification::PassedForExactCut(proof),
            OriginalOutcome::KeptBecauseCutIsExact,
        )
    }

    #[test]
    fn an_exact_split_never_carries_a_wish_to_recycle_the_original() {
        for original_policy in [OriginalPolicy::Keep, OriginalPolicy::RecycleOnceVerified] {
            let options = ExportOptions {
                original_policy,
                cut_precision: CutPrecision::ExactFrame,
                ..ExportOptions::default()
            };
            assert_eq!(options.split_method(), SplitMethod::ExactFrames);
            assert_eq!(
                options.split_method().original_policy(),
                OriginalPolicy::Keep
            );
        }
    }

    #[test]
    fn a_split_on_keyframes_carries_the_setting_for_the_original() {
        for original_policy in [OriginalPolicy::Keep, OriginalPolicy::RecycleOnceVerified] {
            let options = ExportOptions {
                original_policy,
                ..ExportOptions::default()
            };
            assert_eq!(
                options.split_method(),
                SplitMethod::CopyAtKeyframes { original_policy }
            );
            assert_eq!(options.split_method().original_policy(), original_policy);
        }
    }

    #[test]
    fn cuts_go_on_keyframes_unless_chosen_otherwise() {
        assert_eq!(
            ExportOptions::default().cut_precision,
            CutPrecision::NearestKeyframe
        );
    }

    #[test]
    fn the_result_of_an_exact_split_says_what_was_checked_and_that_the_original_is_kept() {
        let report = exact_cut_report();
        assert!(report.is_clean());
        assert_eq!(report.details(), None);
        assert_eq!(
            report.plain_summary(),
            "1 scene file is in\nscenes\n\n\
             Checked: every scene plays to its end, together they have all 225 pictures of \
             the original, each scene starts on the picture that was chosen, and the sound \
             is the original's, unchanged. 50 pictures around the cuts were encoded again.\n\n\
             The original is still in its place. It is always kept after an exact cut, because \
             pictures that were encoded again cannot be compared with it one for one."
        );
    }

    #[test]
    fn the_measurements_of_an_exact_split_say_where_each_cut_landed() {
        assert_eq!(
            exact_cut_report().exact_cut_measurements().as_deref(),
            Some(
                "Pictures: 225 in the original and in the scenes together, 50 of them encoded again.\n\
                 Sound: 389 packets, all copied unchanged. Largest shift against the picture: 0.0007 s.\n\
                 Cut 1: asked for at 00:00:02.900, the next scene starts at 00:00:02.920 (0.020 s later)."
            )
        );
        let copied = report(Verification::Passed, OriginalOutcome::Kept);
        assert_eq!(copied.exact_cut_measurements(), None);
    }

    #[test]
    fn a_clean_split_has_no_details_to_copy() {
        let clean = report(Verification::Passed, OriginalOutcome::MovedToRecycleBin);
        assert_eq!(clean.details(), None);
        assert_eq!(clean.summary(), clean.plain_summary());
    }

    #[test]
    fn a_failed_check_is_said_plainly_with_its_reason_apart() {
        let unverified = report(failed(), OriginalOutcome::KeptBecauseUnverified);
        assert_eq!(
            unverified.details().as_deref(),
            Some("packet 3 of stream 0 differs")
        );
        assert!(!unverified.plain_summary().contains("packet 3"));
        assert!(unverified
            .plain_summary()
            .contains("The check did not pass, so the original was kept."));
    }

    #[test]
    fn the_full_summary_still_carries_each_reason_after_its_sentence() {
        let unverified = report(failed(), OriginalOutcome::KeptBecauseUnverified);
        assert_eq!(
            unverified.summary(),
            "1 scene file is in\nscenes\n\n\
             The check did not pass, so the original was kept. Play the scene files before you delete anything.\n\n\
             Details: packet 3 of stream 0 differs\n\n\
             The original is still in its place."
        );
    }

    #[test]
    fn a_failed_move_of_the_original_gives_its_reason_as_details() {
        let not_moved = report(
            Verification::Passed,
            OriginalOutcome::KeptBecauseRecyclingFailed("access denied".to_owned()),
        );
        assert_eq!(not_moved.details().as_deref(), Some("access denied"));
        assert!(not_moved.summary().ends_with("Details: access denied"));
    }

    #[test]
    fn the_original_is_recycled_only_when_asked_and_verified() {
        assert_eq!(
            decide_about_original(OriginalPolicy::RecycleOnceVerified, &Verification::Passed),
            OriginalDecision::Recycle
        );
    }

    #[test]
    fn the_check_of_an_exact_cut_never_lets_the_original_be_recycled() {
        let Verification::PassedForExactCut(proof) = exact_cut_report().verification else {
            panic!("the report of an exact cut carries its proof");
        };
        assert_eq!(
            decide_about_original(
                OriginalPolicy::RecycleOnceVerified,
                &Verification::PassedForExactCut(proof)
            ),
            OriginalDecision::Keep
        );
    }

    #[test]
    fn a_failed_verification_always_keeps_the_original() {
        assert_eq!(
            decide_about_original(OriginalPolicy::RecycleOnceVerified, &failed()),
            OriginalDecision::KeepBecauseUnverified
        );
    }

    #[test]
    fn the_original_is_kept_unless_recycling_was_asked_for() {
        assert_eq!(
            decide_about_original(OriginalPolicy::Keep, &Verification::Passed),
            OriginalDecision::Keep
        );
        assert_eq!(
            decide_about_original(OriginalPolicy::Keep, &failed()),
            OriginalDecision::Keep
        );
    }
}
