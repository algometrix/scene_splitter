//! Writes the scenes of one video, verifies them, and only then removes the
//! original if the person asked for that.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::exact_cut::CutPrecision;
use crate::exact_split;
use crate::exact_verify::ExactCutProof;
use crate::fates::ScenesToWrite;
use crate::library::VIDEO_EXTENSIONS;
use crate::scene_list::{SceneList, SceneListFormat};
use crate::scene_name::{SceneNamePattern, SceneNames};
use crate::split::{self, SceneLocation, SceneOutcome};
use crate::timecode::format_timestamp;
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
            scene_name: SceneNamePattern::default(),
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

    pub fn cut_precision(self) -> CutPrecision {
        match self {
            Self::CopyAtKeyframes { .. } => CutPrecision::NearestKeyframe,
            Self::ExactFrames => CutPrecision::ExactFrame,
        }
    }
}

/// Why a split keeps the original whatever the person chose for it.
///
/// This is the one model behind every "the original is always kept" in the
/// app: the review says it before the split, the result says it after, and
/// `SplitOrder` enforces it in between.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OriginalAlwaysKept {
    /// A scene was dropped. The scene files do not hold the whole video, so
    /// the original is the only place the dropped part still exists.
    ScenesDropped,
    /// The pictures around each cut were encoded again. No check can
    /// compare those with the original one for one.
    CutIsExact,
    ScenesDroppedAndCutIsExact,
}

impl OriginalAlwaysKept {
    /// Whether a split that drops this many scenes and cuts this precisely
    /// keeps its original whatever was chosen, and why.
    pub fn for_split(dropped_scene_count: usize, cut_precision: CutPrecision) -> Option<Self> {
        match (dropped_scene_count, cut_precision) {
            (0, CutPrecision::NearestKeyframe) => None,
            (0, CutPrecision::ExactFrame) => Some(Self::CutIsExact),
            (_, CutPrecision::NearestKeyframe) => Some(Self::ScenesDropped),
            (_, CutPrecision::ExactFrame) => Some(Self::ScenesDroppedAndCutIsExact),
        }
    }

    /// When the original is always kept and why, as the rest of a sentence
    /// that begins with "The original is" or "It is".
    fn rule_in_words(self) -> &'static str {
        match self {
            Self::ScenesDropped => {
                "always kept when scenes are dropped, because the new files do not hold all of it."
            }
            Self::CutIsExact => {
                "always kept after an exact cut, because pictures that were encoded again \
                 cannot be compared with it one for one."
            }
            Self::ScenesDroppedAndCutIsExact => {
                "always kept when scenes are dropped or cuts are exact, because the new files \
                 do not hold all of it and pictures that were encoded again cannot be compared \
                 with it one for one."
            }
        }
    }

    /// The one sentence the window shows before such a split, in place of
    /// the choice about the original.
    pub fn in_plain_words(self) -> String {
        format!("The original is {}", self.rule_in_words())
    }

    /// The same rule after a sentence that has already named the original.
    fn as_a_following_sentence(self) -> String {
        format!("It is {}", self.rule_in_words())
    }
}

/// What may happen to the original after one split.
#[derive(Debug, Clone, Copy, PartialEq)]
enum OriginalRule {
    /// Every scene is written by copying, so the scene files together are
    /// the whole video and what the person chose for the original applies.
    AsChosen(OriginalPolicy),
    AlwaysKept(OriginalAlwaysKept),
}

/// One split, ready to run: the scenes to write, how precisely to cut, and
/// what may happen to the original afterwards.
///
/// There is no way to make an order that drops a scene or cuts on exact
/// pictures and may recycle the original. `new` is the only way to make
/// one, and it lets the person's choice about the original through only
/// when every scene is written and every packet is copied.
#[derive(Debug, PartialEq)]
pub struct SplitOrder {
    scenes: ScenesToWrite,
    cut_precision: CutPrecision,
    original: OriginalRule,
}

impl SplitOrder {
    pub fn new(scenes: ScenesToWrite, method: SplitMethod) -> Self {
        let cut_precision = method.cut_precision();
        let original = match OriginalAlwaysKept::for_split(scenes.dropped_count(), cut_precision) {
            Some(why) => OriginalRule::AlwaysKept(why),
            None => OriginalRule::AsChosen(method.original_policy()),
        };
        Self {
            scenes,
            cut_precision,
            original,
        }
    }

    pub fn scenes(&self) -> &ScenesToWrite {
        &self.scenes
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
    /// Kept whatever the person chose, for the reason given.
    AlwaysKept(OriginalAlwaysKept),
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
        matches!(
            self.verification,
            Verification::Passed | Verification::PassedForExactCut(_)
        ) && matches!(
            self.original,
            OriginalOutcome::Kept
                | OriginalOutcome::AlwaysKept(_)
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
            Verification::Passed if self.dropped_scenes > 0 => format!(
                "Checked: each file contains its own part of the original, with nothing missing \
                 or changed. {}",
                dropped_scenes_are_in_no_file(self.dropped_scenes)
            ),
            Verification::Passed => {
                "Checked: together they contain the whole original, with nothing missing or changed."
                    .to_owned()
            }
            // The check ran on every scene, before the dropped ones were
            // left out, so the count of pictures is the whole original's.
            Verification::PassedForExactCut(proof) if self.dropped_scenes > 0 => format!(
                "Checked before the dropped scenes were left out: every scene plays to its \
                 end, together they have all {} pictures of the original, each scene starts on \
                 the picture that was chosen, and the sound is the original's, unchanged. {} \
                 around the cuts were encoded again. {}",
                proof.pictures,
                counted_pictures(proof.pictures_encoded_again),
                dropped_scenes_are_in_no_file(self.dropped_scenes)
            ),
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
            OriginalOutcome::AlwaysKept(why) => {
                format!("The original is still in its place. {}", why.as_a_following_sentence())
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
            | OriginalOutcome::AlwaysKept(_)
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

/// "1 scene was dropped and is in no file.", as the result of a split says
/// it after what was checked.
fn dropped_scenes_are_in_no_file(count: usize) -> String {
    format!(
        "{} and {} in no file.",
        counted_dropped_scenes(count),
        if count == 1 { "is" } else { "are" }
    )
}

/// "1 picture", "50 pictures".
fn counted_pictures(count: usize) -> String {
    if count == 1 {
        "1 picture".to_owned()
    } else {
        format!("{count} pictures")
    }
}

/// Splits the video the way `order` says, and checks the scenes.
///
/// Scene files are named by the pattern `scene_names` have in use, and the
/// scene files of earlier splits are found by every naming they know. The
/// scenes that are written are numbered from one without gaps, whichever
/// scenes were dropped.
pub fn export_scenes(
    video: &Path,
    order: &SplitOrder,
    out_dir: &Path,
    scene_names: impl Into<SceneNames>,
) -> Result<ExportReport> {
    let scene_names = scene_names.into();
    match order.cut_precision {
        CutPrecision::NearestKeyframe => export_copied_scenes(video, order, out_dir, scene_names),
        CutPrecision::ExactFrame => export_exact_scenes(video, order, out_dir, scene_names),
    }
}

/// Writes scenes that start on the exact pictures chosen, and checks them.
///
/// The original is never recycled after an exact split, whatever the order
/// says about it. Fails when the scenes could not be written or did not
/// pass their check. In both cases no scene file is put in place.
fn export_exact_scenes(
    video: &Path,
    order: &SplitOrder,
    out_dir: &Path,
    scene_names: SceneNames,
) -> Result<ExportReport> {
    let split = exact_split::split_on_exact_frames(video, order.scenes(), out_dir, scene_names)?;
    let verification = Verification::PassedForExactCut(split.proof);
    let original = match decide_about_original(order.original, &verification) {
        OriginalDecision::AlwaysKeep(why) => OriginalOutcome::AlwaysKept(why),
        // No order for an exact split leads to these. Should one ever, the
        // original stays all the same.
        OriginalDecision::Keep
        | OriginalDecision::KeepBecauseUnverified
        | OriginalDecision::Recycle => OriginalOutcome::AlwaysKept(OriginalAlwaysKept::CutIsExact),
    };
    Ok(ExportReport {
        scene_files: split.scene_files,
        out_dir: out_dir.to_path_buf(),
        dropped_scenes: order.scenes().dropped_count(),
        verification,
        original,
    })
}
/// Copies the scenes out of the video at keyframes. Fails only when the
/// scenes could not be written. A failed verification is reported in the
/// result, and always leaves the original in place.
fn export_copied_scenes(
    video: &Path,
    order: &SplitOrder,
    out_dir: &Path,
    scene_names: SceneNames,
) -> Result<ExportReport> {
    let outcomes = split::split_scenes(
        video,
        order.scenes(),
        out_dir,
        scene_names,
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
        OriginalDecision::AlwaysKeep(why) => OriginalOutcome::AlwaysKept(why),
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
    AlwaysKeep(OriginalAlwaysKept),
    KeepBecauseUnverified,
    Recycle,
}

/// The one rule that guards the original: it is only recycled when nothing
/// about the split says it must stay, the person asked for that, and the
/// scenes were proven to hold every packet of it. The check of an exact cut
/// proves less than that, so it never counts.
fn decide_about_original(rule: OriginalRule, verification: &Verification) -> OriginalDecision {
    match (rule, verification) {
        (OriginalRule::AsChosen(OriginalPolicy::Keep), _) => OriginalDecision::Keep,
        (_, Verification::Failed(_)) => OriginalDecision::KeepBecauseUnverified,
        (
            OriginalRule::AlwaysKept(why),
            Verification::Passed | Verification::PassedForExactCut(_),
        ) => OriginalDecision::AlwaysKeep(why),
        (
            OriginalRule::AsChosen(OriginalPolicy::RecycleOnceVerified),
            Verification::PassedForExactCut(_),
        ) => OriginalDecision::AlwaysKeep(OriginalAlwaysKept::CutIsExact),
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
        assert!(saved.starts_with("scene,start,end,length,file,title\r\n"));
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

    /// Every way a split can be carried out: copied with either choice
    /// about the original, or cut on exact pictures.
    const EVERY_METHOD: [SplitMethod; 3] = [
        SplitMethod::CopyAtKeyframes {
            original_policy: OriginalPolicy::Keep,
        },
        SplitMethod::CopyAtKeyframes {
            original_policy: OriginalPolicy::RecycleOnceVerified,
        },
        SplitMethod::ExactFrames,
    ];

    fn copying(original_policy: OriginalPolicy) -> SplitMethod {
        SplitMethod::CopyAtKeyframes { original_policy }
    }

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

    fn exact_cut_proof() -> ExactCutProof {
        ExactCutProof {
            pictures: 225,
            pictures_encoded_again: 50,
            sound_packets: 389,
            cut_landings: vec![crate::exact_verify::CutLanding {
                asked_for: 2.9,
                landed_at: 2.92,
            }],
            largest_sound_shift_seconds: 0.0007,
        }
    }

    #[test]
    fn a_split_with_a_dropped_scene_keeps_the_original_whatever_was_chosen() {
        for chosen in BOTH_POLICIES {
            let order = SplitOrder::new(with_the_middle_scene_dropped(), copying(chosen));
            assert_eq!(
                order.original,
                OriginalRule::AlwaysKept(OriginalAlwaysKept::ScenesDropped)
            );
        }
    }

    #[test]
    fn a_copied_split_of_every_scene_does_what_was_chosen_for_the_original() {
        for chosen in BOTH_POLICIES {
            let order =
                SplitOrder::new(ScenesToWrite::every_scene(three_scenes()), copying(chosen));
            assert_eq!(order.original, OriginalRule::AsChosen(chosen));
        }
    }

    #[test]
    fn an_exact_split_keeps_the_original_and_says_every_reason_that_applies() {
        let every_scene = ScenesToWrite::every_scene(three_scenes());
        assert_eq!(
            SplitOrder::new(every_scene, SplitMethod::ExactFrames).original,
            OriginalRule::AlwaysKept(OriginalAlwaysKept::CutIsExact)
        );
        let with_a_drop = with_the_middle_scene_dropped();
        assert_eq!(
            SplitOrder::new(with_a_drop, SplitMethod::ExactFrames).original,
            OriginalRule::AlwaysKept(OriginalAlwaysKept::ScenesDroppedAndCutIsExact)
        );
    }

    #[test]
    fn the_reason_the_original_is_always_kept_follows_from_drops_and_precision_alone() {
        let on_keyframes = CutPrecision::NearestKeyframe;
        let exact = CutPrecision::ExactFrame;
        assert_eq!(OriginalAlwaysKept::for_split(0, on_keyframes), None);
        assert_eq!(
            OriginalAlwaysKept::for_split(2, on_keyframes),
            Some(OriginalAlwaysKept::ScenesDropped)
        );
        assert_eq!(
            OriginalAlwaysKept::for_split(0, exact),
            Some(OriginalAlwaysKept::CutIsExact)
        );
        assert_eq!(
            OriginalAlwaysKept::for_split(1, exact),
            Some(OriginalAlwaysKept::ScenesDroppedAndCutIsExact)
        );
    }

    #[test]
    fn each_reason_is_one_sentence_that_names_the_original() {
        assert_eq!(
            OriginalAlwaysKept::ScenesDropped.in_plain_words(),
            "The original is always kept when scenes are dropped, because the new files do \
             not hold all of it."
        );
        assert_eq!(
            OriginalAlwaysKept::CutIsExact.in_plain_words(),
            "The original is always kept after an exact cut, because pictures that were \
             encoded again cannot be compared with it one for one."
        );
        assert_eq!(
            OriginalAlwaysKept::ScenesDroppedAndCutIsExact.in_plain_words(),
            "The original is always kept when scenes are dropped or cuts are exact, because \
             the new files do not hold all of it and pictures that were encoded again cannot \
             be compared with it one for one."
        );
    }

    #[test]
    fn no_order_that_drops_a_scene_or_cuts_exactly_ever_leads_to_recycling() {
        // Every way of dropping scenes from three, with every way to carry
        // out the split and every result of the check.
        let keep_or_drop = [SceneFate::Keep, SceneFate::Drop];
        for first in keep_or_drop {
            for second in keep_or_drop {
                for third in keep_or_drop {
                    let fates = vec![first, second, third];
                    let Ok(scenes) = ScenesToWrite::new(three_scenes(), fates) else {
                        // Dropping every scene is no split at all.
                        continue;
                    };
                    for method in EVERY_METHOD {
                        let copies_every_scene = !scenes.has_dropped_scenes()
                            && method.cut_precision() == CutPrecision::NearestKeyframe;
                        if copies_every_scene {
                            continue;
                        }
                        let order = SplitOrder::new(scenes.clone(), method);
                        let every_result = [
                            Verification::Passed,
                            Verification::PassedForExactCut(exact_cut_proof()),
                            failed(),
                        ];
                        for verification in every_result {
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
        let because_dropped = OriginalRule::AlwaysKept(OriginalAlwaysKept::ScenesDropped);
        assert_eq!(
            decide_about_original(because_dropped, &Verification::Passed),
            OriginalDecision::AlwaysKeep(OriginalAlwaysKept::ScenesDropped)
        );
        assert_eq!(
            decide_about_original(because_dropped, &failed()),
            OriginalDecision::KeepBecauseUnverified
        );
    }

    #[test]
    fn the_result_of_a_split_with_dropped_scenes_says_what_was_dropped_and_kept() {
        let mut with_drops = report(
            Verification::Passed,
            OriginalOutcome::AlwaysKept(OriginalAlwaysKept::ScenesDropped),
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

    fn exact_cut_report() -> ExportReport {
        report(
            Verification::PassedForExactCut(exact_cut_proof()),
            OriginalOutcome::AlwaysKept(OriginalAlwaysKept::CutIsExact),
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
    fn the_result_of_an_exact_split_with_dropped_scenes_says_both_things() {
        let mut report = exact_cut_report();
        report.dropped_scenes = 1;
        report.original =
            OriginalOutcome::AlwaysKept(OriginalAlwaysKept::ScenesDroppedAndCutIsExact);
        assert!(report.is_clean());
        assert_eq!(
            report.plain_summary(),
            "1 scene file is in\nscenes\n\n\
             Checked before the dropped scenes were left out: every scene plays to its end, \
             together they have all 225 pictures of the original, each scene starts on the \
             picture that was chosen, and the sound is the original's, unchanged. 50 pictures \
             around the cuts were encoded again. 1 scene was dropped and is in no file.\n\n\
             The original is still in its place. It is always kept when scenes are dropped or \
             cuts are exact, because the new files do not hold all of it and pictures that \
             were encoded again cannot be compared with it one for one."
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
            decide_about_original(
                OriginalRule::AsChosen(OriginalPolicy::RecycleOnceVerified),
                &Verification::Passed
            ),
            OriginalDecision::Recycle
        );
    }

    #[test]
    fn the_check_of_an_exact_cut_never_lets_the_original_be_recycled() {
        // No order pairs this rule with this check. The rule holds anyway.
        assert_eq!(
            decide_about_original(
                OriginalRule::AsChosen(OriginalPolicy::RecycleOnceVerified),
                &Verification::PassedForExactCut(exact_cut_proof())
            ),
            OriginalDecision::AlwaysKeep(OriginalAlwaysKept::CutIsExact)
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
