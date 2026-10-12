//! Writes the scenes of one video, verifies them, and only then removes the
//! original if the person asked for that.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::fates::ScenesToWrite;
use crate::library::VIDEO_EXTENSIONS;
use crate::scene_list::{SceneList, SceneListFormat};
use crate::scene_name::SceneNamePattern;
use crate::split::{self, SceneLocation, SceneOutcome};
use crate::verify::{self, SceneToVerify};

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
    /// How the scene files are named. A settings file from before this
    /// could be chosen has none, and gets the names the app always wrote.
    pub scene_name: SceneNamePattern,
}

impl Default for ExportOptions {
    /// Scenes go beside the video, and the original is recycled once the
    /// scenes are verified, so the scenes take its place.
    fn default() -> Self {
        Self {
            scene_location: SceneLocation::BesideVideo,
            original_policy: OriginalPolicy::RecycleOnceVerified,
            scene_name: SceneNamePattern::default(),
        }
    }
}

/// What may happen to the original after one split.
#[derive(Debug, Clone, Copy, PartialEq)]
enum OriginalRule {
    /// Every scene is written, so the scene files together are the whole
    /// video and what the person chose for the original applies.
    AsChosen(OriginalPolicy),
    /// A scene was dropped. The scene files do not hold the whole video, so
    /// the original is the only place the dropped part still exists.
    KeptBecauseScenesDropped,
}

/// One split, ready to run: the scenes to write, and what may happen to the
/// original afterwards.
///
/// There is no way to make an order that drops a scene and may recycle the
/// original. `new` is the only way to make one, and it lets the person's
/// choice about the original through only when every scene is written.
#[derive(Debug, PartialEq)]
pub struct SplitOrder {
    scenes: ScenesToWrite,
    original: OriginalRule,
}

impl SplitOrder {
    /// `chosen` is what the person set for originals. It counts only when
    /// no scene is dropped.
    pub fn new(scenes: ScenesToWrite, chosen: OriginalPolicy) -> Self {
        let original = if scenes.has_dropped_scenes() {
            OriginalRule::KeptBecauseScenesDropped
        } else {
            OriginalRule::AsChosen(chosen)
        };
        Self { scenes, original }
    }

    pub fn scenes(&self) -> &ScenesToWrite {
        &self.scenes
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
    /// Kept whatever the person chose, because the split dropped a scene.
    KeptBecauseScenesDropped,
    MovedToRecycleBin,
    KeptBecauseUnverified,
    KeptBecauseRecyclingFailed(String),
}

#[derive(Debug)]
pub struct ExportReport {
    pub scene_files: Vec<PathBuf>,
    pub out_dir: PathBuf,
    /// How many scenes the person dropped, which are in no scene file.
    pub dropped_scenes: usize,
    pub verification: Verification,
    pub original: OriginalOutcome,
}

impl ExportReport {
    /// True when the scenes are verified and nothing went wrong afterwards.
    pub fn is_clean(&self) -> bool {
        matches!(self.verification, Verification::Passed)
            && matches!(
                self.original,
                OriginalOutcome::Kept
                    | OriginalOutcome::KeptBecauseScenesDropped
                    | OriginalOutcome::MovedToRecycleBin
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
            Verification::Passed if self.dropped_scenes > 0 => format!(
                "Checked: each file contains its own part of the original, with nothing missing \
                 or changed. {} and {} in no file.",
                counted_dropped_scenes(self.dropped_scenes),
                if self.dropped_scenes == 1 { "is" } else { "are" }
            ),
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
            OriginalOutcome::KeptBecauseScenesDropped => ORIGINAL_KEPT_WITH_DROPPED_SCENES.to_owned(),
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
            | OriginalOutcome::KeptBecauseScenesDropped
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

/// "1 scene was dropped", "3 scenes were dropped".
pub fn counted_dropped_scenes(count: usize) -> String {
    if count == 1 {
        "1 scene was dropped".to_owned()
    } else {
        format!("{count} scenes were dropped")
    }
}

/// Said before and after a split with dropped scenes, so nobody waits for
/// the original to go to the Recycle Bin or Trash.
pub const ORIGINAL_KEPT_WITH_DROPPED_SCENES: &str =
    "The original is still in its place. It is always kept when scenes are dropped, \
     because the new files do not hold all of it.";

/// Fails only when the scenes could not be written. A failed verification is
/// reported in the result, and always leaves the original in place.
///
/// Scene files are named by `scene_name`. The scenes that are written are
/// numbered from one without gaps, whichever scenes were dropped.
pub fn export_scenes(
    video: &Path,
    order: &SplitOrder,
    out_dir: &Path,
    scene_name: SceneNamePattern,
) -> Result<ExportReport> {
    let outcomes = split::split_scenes(
        video,
        order.scenes(),
        out_dir,
        scene_name,
        verify::record_dropped_scene,
    )?;
    let scene_files = split::written_files(&outcomes);
    let scenes_to_verify: Vec<SceneToVerify> = outcomes
        .into_iter()
        .map(|outcome| match outcome {
            SceneOutcome::Written(scene_file) => SceneToVerify::InFile(scene_file),
            SceneOutcome::Dropped(packets) => SceneToVerify::Dropped(packets),
        })
        .collect();

    // A split of every scene goes through the check it always went through.
    // Only a split that dropped scenes needs what was noted about them.
    let verified = if order.scenes().has_dropped_scenes() {
        verify::verify_scenes_with_dropped(video, &scenes_to_verify)
    } else {
        verify::verify_scenes(video, &scene_files)
    };
    let verification = match verified {
        Ok(()) => Verification::Passed,
        Err(error) => Verification::Failed(format!("{error:#}")),
    };
    let original = match decide_about_original(order.original, &verification) {
        OriginalDecision::Keep => OriginalOutcome::Kept,
        OriginalDecision::KeepBecauseScenesDropped => OriginalOutcome::KeptBecauseScenesDropped,
        OriginalDecision::KeepBecauseUnverified => OriginalOutcome::KeptBecauseUnverified,
        OriginalDecision::Recycle => match trash::delete(video) {
            Ok(()) => OriginalOutcome::MovedToRecycleBin,
            Err(error) => OriginalOutcome::KeptBecauseRecyclingFailed(error.to_string()),
        },
    };

    Ok(ExportReport {
        scene_files,
        out_dir: out_dir.to_path_buf(),
        dropped_scenes: order.scenes().dropped_count(),
        verification,
        original,
    })
}

/// What to do when the file a scene list is saved to exists already.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExistingListFile {
    /// Stop, and leave the file as it is.
    Refuse,
    /// Replace it. The person was asked, or said so on the command line.
    Replace,
}

/// Writes the list of scenes to `path` in the given format. Nothing is
/// split, and the video is only named in the list.
///
/// A list is never written over a video, whatever was asked: not over the
/// one it lists, and not over any file with a video's extension. A slip in
/// a file dialog must not be able to cost a video.
pub fn save_scene_list(
    list: &SceneList,
    format: SceneListFormat,
    path: &Path,
    if_exists: ExistingListFile,
) -> Result<()> {
    if has_video_extension(path) {
        bail!(
            "{} has the name of a video. Give the scene list another name, such as one ending in .{}",
            path.display(),
            format.extension()
        );
    }
    if if_exists == ExistingListFile::Refuse && path.exists() {
        bail!(
            "{} exists already. Pass --overwrite to replace it, or choose another name",
            path.display()
        );
    }
    let text = list.in_format(format)?;
    std::fs::write(path, text)
        .with_context(|| format!("could not write the scene list to {}", path.display()))
}

fn has_video_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|video| extension.eq_ignore_ascii_case(video))
        })
}

#[derive(Debug, PartialEq)]
enum OriginalDecision {
    Keep,
    KeepBecauseScenesDropped,
    KeepBecauseUnverified,
    Recycle,
}

/// The one rule that guards the original: it is only recycled when every
/// scene was written, the person asked for that, and the scenes passed
/// verification.
fn decide_about_original(rule: OriginalRule, verification: &Verification) -> OriginalDecision {
    match (rule, verification) {
        (OriginalRule::AsChosen(OriginalPolicy::Keep), _) => OriginalDecision::Keep,
        (_, Verification::Failed(_)) => OriginalDecision::KeepBecauseUnverified,
        (OriginalRule::KeptBecauseScenesDropped, Verification::Passed) => {
            OriginalDecision::KeepBecauseScenesDropped
        }
        (OriginalRule::AsChosen(OriginalPolicy::RecycleOnceVerified), Verification::Passed) => {
            OriginalDecision::Recycle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fates::SceneFate;
    use crate::scenes::CutTimes;
    use crate::timecode::TimeSpan;

    fn failed() -> Verification {
        Verification::Failed("packet 3 of stream 0 differs".to_owned())
    }

    fn report(verification: Verification, original: OriginalOutcome) -> ExportReport {
        ExportReport {
            scene_files: vec![PathBuf::from("film_scene_001.mp4")],
            out_dir: PathBuf::from("scenes"),
            dropped_scenes: 0,
            verification,
            original,
        }
    }

    /// A folder of its own for one test, removed when the test ends.
    struct Folder(PathBuf);

    impl Folder {
        fn for_test(name: &str) -> Self {
            let folder = std::env::temp_dir().join(format!(
                "scene_splitter_export_test_{}_{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            Self(folder)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn list_of_three_scenes() -> SceneList {
        SceneList::of_split(
            Path::new("film.mp4"),
            &ScenesToWrite::every_scene(three_scenes()),
            crate::scene_list::FrameRate::per_second(25.0).unwrap(),
            SceneNamePattern::default(),
        )
        .unwrap()
    }

    #[test]
    fn a_scene_list_is_saved_in_the_format_asked_for() {
        let folder = Folder::for_test("list_saved");
        let path = folder.0.join("scenes.csv");
        save_scene_list(
            &list_of_three_scenes(),
            SceneListFormat::Csv,
            &path,
            ExistingListFile::Refuse,
        )
        .unwrap();

        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with("scene,start,end,length,file\r\n"));
        assert_eq!(saved.lines().count(), 4);
    }

    #[test]
    fn a_scene_list_is_never_written_over_a_video() {
        let folder = Folder::for_test("list_over_video");
        let video = folder.0.join("film.MP4");
        std::fs::write(&video, "the video").unwrap();

        for if_exists in [ExistingListFile::Refuse, ExistingListFile::Replace] {
            let error = save_scene_list(
                &list_of_three_scenes(),
                SceneListFormat::Csv,
                &video,
                if_exists,
            )
            .unwrap_err();
            assert!(error.to_string().contains("has the name of a video"));
        }
        assert_eq!(std::fs::read_to_string(&video).unwrap(), "the video");
    }

    #[test]
    fn an_existing_list_is_only_replaced_when_that_was_asked_for() {
        let folder = Folder::for_test("list_exists");
        let path = folder.0.join("scenes.edl");
        std::fs::write(&path, "an earlier list").unwrap();
        let list = list_of_three_scenes();

        let refused = save_scene_list(&list, SceneListFormat::Edl, &path, ExistingListFile::Refuse);
        assert!(refused.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "an earlier list");

        save_scene_list(
            &list,
            SceneListFormat::Edl,
            &path,
            ExistingListFile::Replace,
        )
        .unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("TITLE: film.mp4\n"));
    }

    const BOTH_POLICIES: [OriginalPolicy; 2] =
        [OriginalPolicy::Keep, OriginalPolicy::RecycleOnceVerified];

    fn three_scenes() -> CutTimes {
        let whole_video = TimeSpan {
            start: 0.0,
            end: 30.0,
        };
        CutTimes::inside(whole_video, vec![10.0, 20.0]).unwrap()
    }

    fn with_the_middle_scene_dropped() -> ScenesToWrite {
        let fates = vec![SceneFate::Keep, SceneFate::Drop, SceneFate::Keep];
        ScenesToWrite::new(three_scenes(), fates).unwrap()
    }

    #[test]
    fn a_split_with_a_dropped_scene_keeps_the_original_whatever_was_chosen() {
        for chosen in BOTH_POLICIES {
            let order = SplitOrder::new(with_the_middle_scene_dropped(), chosen);
            assert_eq!(order.original, OriginalRule::KeptBecauseScenesDropped);
        }
    }

    #[test]
    fn a_split_of_every_scene_does_what_was_chosen_for_the_original() {
        for chosen in BOTH_POLICIES {
            let order = SplitOrder::new(ScenesToWrite::every_scene(three_scenes()), chosen);
            assert_eq!(order.original, OriginalRule::AsChosen(chosen));
        }
    }

    #[test]
    fn no_order_with_a_dropped_scene_ever_leads_to_recycling() {
        // Every way of dropping scenes from three, with both choices about
        // the original and both results of the check.
        let keep_or_drop = [SceneFate::Keep, SceneFate::Drop];
        for first in keep_or_drop {
            for second in keep_or_drop {
                for third in keep_or_drop {
                    let fates = vec![first, second, third];
                    let Ok(scenes) = ScenesToWrite::new(three_scenes(), fates) else {
                        // Dropping every scene is no split at all.
                        continue;
                    };
                    if !scenes.has_dropped_scenes() {
                        continue;
                    }
                    for chosen in BOTH_POLICIES {
                        let order = SplitOrder::new(scenes.clone(), chosen);
                        for verification in [Verification::Passed, failed()] {
                            assert_ne!(
                                decide_about_original(order.original, &verification),
                                OriginalDecision::Recycle
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_checked_split_with_dropped_scenes_says_why_the_original_stayed() {
        assert_eq!(
            decide_about_original(
                OriginalRule::KeptBecauseScenesDropped,
                &Verification::Passed
            ),
            OriginalDecision::KeepBecauseScenesDropped
        );
        assert_eq!(
            decide_about_original(OriginalRule::KeptBecauseScenesDropped, &failed()),
            OriginalDecision::KeepBecauseUnverified
        );
    }

    #[test]
    fn the_result_of_a_split_with_dropped_scenes_says_what_was_dropped_and_kept() {
        let mut with_drops = report(
            Verification::Passed,
            OriginalOutcome::KeptBecauseScenesDropped,
        );
        with_drops.dropped_scenes = 2;
        assert!(with_drops.is_clean());
        assert_eq!(
            with_drops.plain_summary(),
            "1 scene file is in\nscenes\n\n\
             Checked: each file contains its own part of the original, with nothing missing or \
             changed. 2 scenes were dropped and are in no file.\n\n\
             The original is still in its place. It is always kept when scenes are dropped, \
             because the new files do not hold all of it."
        );

        with_drops.dropped_scenes = 1;
        assert!(with_drops
            .plain_summary()
            .contains("1 scene was dropped and is in no file."));
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
            decide_about_original(
                OriginalRule::AsChosen(OriginalPolicy::RecycleOnceVerified),
                &Verification::Passed
            ),
            OriginalDecision::Recycle
        );
    }

    #[test]
    fn a_failed_verification_always_keeps_the_original() {
        assert_eq!(
            decide_about_original(
                OriginalRule::AsChosen(OriginalPolicy::RecycleOnceVerified),
                &failed()
            ),
            OriginalDecision::KeepBecauseUnverified
        );
    }

    #[test]
    fn the_original_is_kept_unless_recycling_was_asked_for() {
        assert_eq!(
            decide_about_original(
                OriginalRule::AsChosen(OriginalPolicy::Keep),
                &Verification::Passed
            ),
            OriginalDecision::Keep
        );
        assert_eq!(
            decide_about_original(OriginalRule::AsChosen(OriginalPolicy::Keep), &failed()),
            OriginalDecision::Keep
        );
    }
}
