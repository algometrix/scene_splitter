//! Writes the scenes of one video, verifies them, and only then removes the
//! original if the person asked for that.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::scenes::CutTimes;
use crate::split::{self, SceneLocation};
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
}

impl Default for ExportOptions {
    /// Scenes go beside the video, and the original is recycled once the
    /// scenes are verified, so the scenes take its place.
    fn default() -> Self {
        Self {
            scene_location: SceneLocation::BesideVideo,
            original_policy: OriginalPolicy::RecycleOnceVerified,
        }
    }
}

#[derive(Debug)]
pub enum Verification {
    Passed,
    Failed(String),
}

#[derive(Debug)]
pub enum OriginalOutcome {
    Kept,
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
        matches!(self.verification, Verification::Passed)
            && matches!(
                self.original,
                OriginalOutcome::Kept | OriginalOutcome::MovedToRecycleBin
            )
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
            Verification::Failed(_) => {
                "The check did not pass, so the original was kept. Play the scene files before you delete anything."
                    .to_owned()
            }
        }
    }

    fn verification_details(&self) -> Option<&str> {
        match &self.verification {
            Verification::Passed => None,
            Verification::Failed(reason) => Some(reason),
        }
    }

    fn original_in_plain_words(&self) -> String {
        match &self.original {
            OriginalOutcome::Kept | OriginalOutcome::KeptBecauseUnverified => {
                "The original is still in its place.".to_owned()
            }
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
/// person asked for that and the scenes passed verification.
fn decide_about_original(policy: OriginalPolicy, verification: &Verification) -> OriginalDecision {
    match (policy, verification) {
        (OriginalPolicy::Keep, _) => OriginalDecision::Keep,
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
