//! Writes the scenes of one video, verifies them, and only then removes the
//! original if the person asked for that.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

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

    pub fn summary(&self) -> String {
        let written = format!(
            "Wrote {} scene files to\n{}",
            self.scene_files.len(),
            self.out_dir.display()
        );
        let verification = match &self.verification {
            Verification::Passed => {
                "Verified: the scenes hold every video and audio packet of the original, unchanged."
                    .to_owned()
            }
            Verification::Failed(reason) => format!("Verification failed: {reason}"),
        };
        let original = match &self.original {
            OriginalOutcome::Kept => "The original was kept.".to_owned(),
            OriginalOutcome::MovedToRecycleBin => {
                format!("The original was moved to the {TRASH_NAME}.")
            }
            OriginalOutcome::KeptBecauseUnverified => {
                "The original was kept, because the scenes could not be verified.".to_owned()
            }
            OriginalOutcome::KeptBecauseRecyclingFailed(reason) => {
                format!("The original could not be moved to the {TRASH_NAME}: {reason}")
            }
        };
        format!("{written}\n\n{verification}\n\n{original}")
    }
}

/// Fails only when the scenes could not be written. A failed verification is
/// reported in the result, and always leaves the original in place.
pub fn export_scenes(
    video: &Path,
    cuts: &[f64],
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
