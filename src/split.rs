//! Writes one file per scene by copying the compressed streams. Nothing is
//! re-encoded, so a two hour video splits in seconds and loses no quality.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cache::fnv1a_64;
use crate::export::TRASH_NAME;
use crate::fates::{SceneFate, ScenesToWrite};
use crate::ffmpeg::{Program, TimeLimit, COPY_SCENE_STREAMS};
use crate::folder_settings::{PathCase, SYSTEM_PATH_CASE};
use crate::scene_name::{
    self, EarlierSplit, SceneNamePattern, SceneNames, SceneNaming, FIRST_SCENE_NUMBER,
};
use crate::scenes::CutTimes;
use crate::timecode::TimeSpan;
use crate::tools;

/// The segment muxer starts a new file at the first keyframe at or after each
/// cut time. Asking slightly early keeps timestamp rounding from pushing a cut
/// to the following keyframe.
const CUT_MARGIN_SECONDS: f64 = 0.001;

/// Copying takes as long as the drives need to read and write the whole
/// video, which no fixed total fits. So ffmpeg is asked to report its
/// progress, which it does about twice a second, and the copy is stopped
/// only when the reports stop. Two minutes of silence allows for a sleeping
/// disk or a slow network share, and is far longer than a healthy copy ever
/// pauses.
const COPY_STALL_LIMIT: TimeLimit = TimeLimit::Silence(Duration::from_secs(120));

const NEW_FOLDER_SUFFIX: &str = "_scenes";
/// Scenes are written into "unfinished_<number of the video's name>" inside
/// the folder they go to, as "001.<extension>" and so on, and moved out of
/// it when the split has worked. A split into a thousand scenes or more
/// goes on with "1000.<extension>".
const UNFINISHED_NUMBER_DIGITS: usize = 3;
///
/// Both names are short on purpose. Windows refuses a path of more than
/// about 260 characters, and a scene file of a video with a long name is
/// close to that already. Repeating the video's name in the folder would
/// make a split fail that used to work.
const UNFINISHED_FOLDER_PREFIX: &str = "unfinished_";

/// Where the scene files of a video are written.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SceneLocation {
    /// In the same folder as the video.
    BesideVideo,
    /// In a new "<video name>_scenes" folder next to the video.
    NewFolder,
}

pub fn scene_dir(video: &Path, location: SceneLocation) -> Result<PathBuf> {
    match location {
        SceneLocation::BesideVideo => Ok(video
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf()),
        SceneLocation::NewFolder => {
            let stem = video
                .file_stem()
                .with_context(|| format!("{} has no file name", video.display()))?;
            let mut folder_name = stem.to_os_string();
            folder_name.push(NEW_FOLDER_SUFFIX);
            Ok(video.with_file_name(folder_name))
        }
    }
}

/// The files a split writes for `scenes`, in scene order. `scenes` are the
/// scenes that are written, so the files are numbered without gaps.
pub fn scene_file_paths(
    video: &Path,
    out_dir: &Path,
    pattern: SceneNamePattern,
    scenes: &[TimeSpan],
) -> Result<Vec<PathBuf>> {
    let file_names = SceneNaming::of(video, pattern)?.file_names(scenes)?;
    Ok(file_names
        .into_iter()
        .map(|file_name| out_dir.join(file_name))
        .collect())
}

/// The earlier splits of this video that left scene files in `out_dir`,
/// each with its files. A scene file is one named the way `names` name the
/// scenes of this video now, or named them for an earlier split, so the
/// same naming decides what a split writes and what it finds.
///
/// A new split replaces or removes all of them, so callers ask the person
/// before going ahead when this is not empty.
pub fn earlier_splits_in(
    video: &Path,
    out_dir: &Path,
    names: impl Into<SceneNames>,
) -> Result<Vec<EarlierSplit>> {
    let names = names.into();
    if !out_dir.is_dir() {
        // Asked all the same, so that a video no scene can be named after
        // is refused whether or not its folder exists yet.
        return Ok(scene_name::earlier_splits_among(video, names, Vec::new())?);
    }
    let entries = std::fs::read_dir(out_dir)
        .with_context(|| format!("could not read {}", out_dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("could not read {}", out_dir.display()))?
            .path();
        if path.is_file() {
            files.push(path);
        }
    }
    Ok(scene_name::earlier_splits_among(video, names, files)?)
}

/// Every scene file of this video already in `out_dir`, whichever earlier
/// split wrote it, in name order. For tests that look at a folder after a
/// split.
#[cfg(test)]
pub fn scene_files_from_earlier_splits(
    video: &Path,
    out_dir: &Path,
    names: impl Into<SceneNames>,
) -> Result<Vec<PathBuf>> {
    Ok(files_of(&earlier_splits_in(video, out_dir, names)?))
}

/// The files of `earlier_splits`, in name order, each once.
pub fn files_of(earlier_splits: &[EarlierSplit]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = earlier_splits
        .iter()
        .flat_map(|split| split.files().iter().cloned())
        .collect();
    files.sort();
    files.dedup();
    files
}

/// What a split made of one scene.
#[derive(Debug, PartialEq)]
pub enum SceneOutcome<Record> {
    /// The scene is in this file.
    Written(PathBuf),
    /// The scene was dropped and is in no file. This is what the caller
    /// noted about it while it still existed.
    Dropped(Record),
}

/// Splits `input` at the given keyframe times into one file per scene, with
/// the default names, and returns the scene files in order.
#[cfg(test)]
pub fn split_at_keyframes(input: &Path, cuts: &CutTimes, out_dir: &Path) -> Result<Vec<PathBuf>> {
    let every_scene = ScenesToWrite::every_scene(cuts.clone());
    let nothing_is_dropped = |_dropped: &Path| Ok(());
    let outcomes = split_scenes(
        input,
        &every_scene,
        out_dir,
        SceneNamePattern::default(),
        nothing_is_dropped,
    )?;
    Ok(written_files(&outcomes))
}

/// The scene files among `outcomes`, in scene order.
pub fn written_files<Record>(outcomes: &[SceneOutcome<Record>]) -> Vec<PathBuf> {
    outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            SceneOutcome::Written(scene_file) => Some(scene_file.clone()),
            SceneOutcome::Dropped(_) => None,
        })
        .collect()
}

/// Splits `input` at the cuts of `scenes` and says, for every scene in
/// order, which file it is in or that it was dropped. Scene files of this
/// video from an earlier split are replaced, and those with names the new
/// split does not use are moved to the Recycle Bin or Trash.
///
/// The scenes are written into a folder of their own first and only moved
/// to their real names once all of them are there. A split that fails or is
/// stopped half way therefore leaves nothing that looks like a finished
/// scene, and leaves the scene files of an earlier split as they were.
///
/// Dropped scenes are written into that folder too, since ffmpeg cuts the
/// whole video in one pass, and are removed with it. Before that,
/// `record_dropped_scene` is shown each of them, so the caller can keep
/// what it needs to prove later that the kept scenes are the right parts
/// of the video. A split with dropped scenes so needs as much free room
/// while it runs as one without.
pub fn split_scenes<Record>(
    input: &Path,
    scenes: &ScenesToWrite,
    out_dir: &Path,
    names: impl Into<SceneNames>,
    record_dropped_scene: impl Fn(&Path) -> Result<Record>,
) -> Result<Vec<SceneOutcome<Record>>> {
    let names = names.into();
    let name = UnfinishedNames::of(&SceneNaming::of(input, names.in_use())?);
    write_then_move_into_place(
        input,
        scenes,
        out_dir,
        names,
        |unfinished| {
            write_scenes(input, scenes.cuts(), &unfinished.folder, &name).map(|_written| ())
        },
        record_dropped_scene,
    )
}

/// Where the scenes of a split are written before they get their names.
pub struct UnfinishedScenes {
    /// A folder of this split's own, removed when the split is over.
    pub folder: PathBuf,
    /// The files to write, in scene order, one for every scene of the
    /// video, the dropped ones included.
    pub files: Vec<PathBuf>,
}

/// The safe way to replace the scene files of `input` in `out_dir`, whatever
/// writes them. `write_scenes` writes every scene to the place it is given,
/// the dropped ones too. Only when it succeeds is each dropped scene shown
/// to `record_dropped_scene`, are the kept scenes moved to their real
/// names, and the scene files of earlier splits replaced or recycled.
///
/// The scenes get the name `names` have in use. What earlier splits left is
/// looked for under every naming `names` know, and nothing else in the
/// folder is replaced or removed.
pub fn write_then_move_into_place<Record>(
    input: &Path,
    scenes: &ScenesToWrite,
    out_dir: &Path,
    names: impl Into<SceneNames>,
    write_scenes: impl FnOnce(&UnfinishedScenes) -> Result<()>,
    record_dropped_scene: impl Fn(&Path) -> Result<Record>,
) -> Result<Vec<SceneOutcome<Record>>> {
    let names = names.into();
    let pattern = names.in_use();
    let name = UnfinishedNames::of(&SceneNaming::of(input, pattern)?);
    let earlier_splits = earlier_splits_in(input, out_dir, names)?;
    let scene_files = scene_file_paths(input, out_dir, pattern, &scenes.kept_scenes())?;
    refuse_to_write_over_the_video(input, &scene_files)?;
    refuse_to_write_over_other_files(&scene_files, &earlier_splits, SYSTEM_PATH_CASE)?;

    let unfinished_dir = out_dir.join(name.unfinished_folder());
    // What a run that was killed left there is of no use to anyone.
    remove_unfinished_scenes(&unfinished_dir)?;
    std::fs::create_dir_all(&unfinished_dir)
        .with_context(|| format!("could not create {}", unfinished_dir.display()))?;
    let unfinished = UnfinishedScenes {
        files: name.unfinished_files_in(&unfinished_dir, scenes.fates().len()),
        folder: unfinished_dir.clone(),
    };

    let settled = write_scenes(&unfinished).and_then(|()| {
        // Asked again: writing takes a while, and a file that turned up at
        // a scene's name in the meantime is nobody's earlier scene.
        refuse_to_write_over_other_files(&scene_files, &earlier_splits, SYSTEM_PATH_CASE)?;
        settle_scenes(
            &unfinished.files,
            scenes.fates(),
            &scene_files,
            &record_dropped_scene,
        )
    });
    let tidied = remove_unfinished_scenes(&unfinished_dir);
    // Why the split failed matters more than a folder that stayed behind.
    let outcomes = settled?;
    tidied?;

    remove_leftover_scenes(
        &earlier_splits,
        &scene_files,
        SYSTEM_PATH_CASE,
        MOVE_LEFTOVER_AWAY,
    )?;
    Ok(outcomes)
}

/// Moving a scene to its name replaces whatever file has that name. That is
/// meant for a scene file of an earlier split, which the person was asked
/// about. A file in that place that was not found to be one was never asked
/// about, so the split stops before it writes anything.
///
/// Nearly every name a split gives is also found as an earlier scene. The
/// exception is a number of four digits or more with no count in the name,
/// which counts as a scene only beside the first scene of its split.
fn refuse_to_write_over_other_files(
    scene_files: &[PathBuf],
    earlier_splits: &[EarlierSplit],
    case: PathCase,
) -> Result<()> {
    let is_an_earlier_scene = |scene_file: &Path| {
        earlier_splits
            .iter()
            .flat_map(EarlierSplit::files)
            .any(|earlier| has_same_file_name(earlier, scene_file, case))
    };
    // Asked of the name itself, so that a link that leads nowhere counts as
    // being there too.
    let is_there = |scene_file: &Path| std::fs::symlink_metadata(scene_file).is_ok();
    let in_the_way = scene_files
        .iter()
        .find(|scene_file| is_there(scene_file) && !is_an_earlier_scene(scene_file));
    if let Some(in_the_way) = in_the_way {
        bail!(
            "{} is where a scene would go, and it does not look like a scene file of an \
             earlier split of this video. Move it away, or choose another name for scene files",
            in_the_way.display()
        );
    }
    Ok(())
}

/// Whether two files of one folder are the same file, going by their names.
fn has_same_file_name(one: &Path, other: &Path, case: PathCase) -> bool {
    match (one.file_name(), other.file_name()) {
        (Some(one), Some(other)) => {
            scene_name::is_same_file_name(&one.to_string_lossy(), &other.to_string_lossy(), case)
        }
        _ => false,
    }
}

/// No pattern can name a scene like its video today, since every scene name
/// holds the video's name and a number besides. Should that ever change,
/// the split stops here instead of replacing the original with a scene.
/// Names that differ only in capitals are one file on Windows and macOS.
fn refuse_to_write_over_the_video(input: &Path, scene_files: &[PathBuf]) -> Result<()> {
    let same_name_as_the_video = scene_files
        .iter()
        .find(|scene_file| has_same_file_name(scene_file, input, SYSTEM_PATH_CASE));
    if let Some(scene_file) = same_name_as_the_video {
        bail!(
            "the scene file {} would have the name of the video itself",
            scene_file.display()
        );
    }
    Ok(())
}

/// Shows every dropped scene to `record_dropped_scene`, then gives every
/// kept scene its real name. The dropped ones come first: looking at them
/// changes nothing, so a failure there leaves an earlier split untouched.
fn settle_scenes<Record>(
    unfinished_files: &[PathBuf],
    fates: &[SceneFate],
    scene_files: &[PathBuf],
    record_dropped_scene: &impl Fn(&Path) -> Result<Record>,
) -> Result<Vec<SceneOutcome<Record>>> {
    let mut records = Vec::new();
    let mut kept_unfinished_files = Vec::new();
    for (unfinished, fate) in unfinished_files.iter().zip(fates) {
        match fate {
            SceneFate::Keep => kept_unfinished_files.push(unfinished.clone()),
            SceneFate::Drop => records.push(record_dropped_scene(unfinished)?),
        }
    }
    move_into_place(&kept_unfinished_files, scene_files)?;

    let mut scene_files = scene_files.iter();
    let mut records = records.into_iter();
    fates
        .iter()
        .map(|fate| match fate {
            SceneFate::Keep => scene_files.next().cloned().map(SceneOutcome::Written),
            SceneFate::Drop => records.next().map(SceneOutcome::Dropped),
        })
        .collect::<Option<Vec<_>>>()
        .context("there are not as many scene files as scenes to write")
}

/// Has ffmpeg write every scene into `unfinished_dir` and returns those
/// files in scene order. Fails unless every one of them was written.
fn write_scenes(
    input: &Path,
    cuts: &CutTimes,
    unfinished_dir: &Path,
    name: &UnfinishedNames,
) -> Result<Vec<PathBuf>> {
    let cut_times = cuts
        .as_slice()
        .iter()
        .map(|cut| format!("{:.6}", cut - CUT_MARGIN_SECONDS))
        .collect::<Vec<_>>()
        .join(",");

    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-y"])
        // Progress reports on standard output, which the stall limit watches.
        .args(["-progress", "pipe:1"])
        .arg("-i")
        .arg(input)
        .args(COPY_SCENE_STREAMS)
        .args(["-f", "segment", "-segment_times", &cut_times])
        .args(["-segment_start_number", &FIRST_SCENE_NUMBER.to_string()])
        // Each scene file starts at time zero instead of its time in the source.
        .args(["-reset_timestamps", "1"])
        .arg(name.ffmpeg_pattern_in(unfinished_dir))
        .stream_lines(COPY_STALL_LIMIT, |_progress_report| Ok(()))
        .with_context(|| format!("could not split {}", input.display()))?;

    let unfinished_files = name.unfinished_files_in(unfinished_dir, cuts.len() + 1);
    if let Some(missing) = unfinished_files.iter().find(|file| !file.is_file()) {
        bail!("ffmpeg finished without writing {}", missing.display());
    }
    Ok(unfinished_files)
}

/// Gives each finished scene its real name, replacing a scene file of an
/// earlier split that has it. Both are in the same folder tree, so each
/// move is a rename and not a copy.
fn move_into_place(unfinished_files: &[PathBuf], scene_files: &[PathBuf]) -> Result<()> {
    for (moved_before, (unfinished, scene_file)) in
        unfinished_files.iter().zip(scene_files).enumerate()
    {
        std::fs::rename(unfinished, scene_file).with_context(|| {
            format!(
                "could not put {} in its place, after {moved_before} of {} scene files were. \
                 Those are new, the rest are still from an earlier split",
                scene_file.display(),
                scene_files.len()
            )
        })?;
    }
    Ok(())
}

/// Removes the folder scenes are written into, with whatever is left in
/// it. A folder that is not there is fine.
fn remove_unfinished_scenes(unfinished_dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(unfinished_dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "could not remove the unfinished scene files in {}",
                unfinished_dir.display()
            )
        }),
    }
}

/// How a scene file of an earlier split leaves the folder when the new
/// split has no use for its name.
type MoveLeftoverAway = fn(&Path) -> Result<()>;

/// A leftover is only known by its name, and a name can fit a file that no
/// split ever wrote. So nothing found by name is deleted for good: it goes
/// to the Recycle Bin or Trash, where the person can get it back.
#[cfg(not(test))]
const MOVE_LEFTOVER_AWAY: MoveLeftoverAway = recycle_leftover;
/// The tests split the videos they made over and over. Their leftovers are
/// deleted, so that running the tests does not fill the Recycle Bin of
/// whoever runs them.
#[cfg(test)]
const MOVE_LEFTOVER_AWAY: MoveLeftoverAway = delete_leftover_of_a_test;

#[cfg_attr(test, allow(dead_code))]
fn recycle_leftover(leftover: &Path) -> Result<()> {
    trash::delete(leftover).with_context(|| {
        format!(
            "the new scene files are in place, but the outdated {} could not be moved to the \
             {TRASH_NAME}. Remove it yourself, or it sits beside the new scenes",
            leftover.display()
        )
    })
}

#[cfg(test)]
fn delete_leftover_of_a_test(leftover: &Path) -> Result<()> {
    std::fs::remove_file(leftover)
        .with_context(|| format!("could not remove the outdated {}", leftover.display()))
}

/// An earlier split leaves files the new split did not write over: the
/// higher numbers of a split into more scenes, or every file of a split
/// that was named another way or said another count. Left in place they
/// would sit beside the new scenes and show the same parts of the video
/// twice.
///
/// Only the files of an `EarlierSplit` are moved away, and each split is
/// dealt with on its own.
fn remove_leftover_scenes(
    earlier_splits: &[EarlierSplit],
    new_scene_files: &[PathBuf],
    case: PathCase,
    move_away: MoveLeftoverAway,
) -> Result<()> {
    earlier_splits.iter().try_for_each(|earlier_split| {
        leftovers_of(earlier_split, new_scene_files, case)
            .into_iter()
            .try_for_each(move_away)
    })
}

/// The files of an earlier split that the new split did not write over.
///
/// The names are compared the way the system tells files apart. Where
/// capitals do not count, "Holiday_scene_1_of_2.mp4" of an earlier split is
/// the very file the new "holiday_scene_1_of_2.mp4" was written over, and
/// moving it away would move the new scene away.
fn leftovers_of<'split>(
    earlier_split: &'split EarlierSplit,
    new_scene_files: &[PathBuf],
    case: PathCase,
) -> Vec<&'split Path> {
    earlier_split
        .files()
        .iter()
        .filter(|earlier| {
            !new_scene_files
                .iter()
                .any(|new| has_same_file_name(earlier, new, case))
        })
        .map(PathBuf::as_path)
        .collect()
}

/// The naming of one video's scene files while they are being written. What
/// they are called once they are finished is `SceneNaming`'s to say, which
/// is also where the video's name is read.
struct UnfinishedNames {
    video_file_name: String,
    extension: String,
}

impl UnfinishedNames {
    fn of(naming: &SceneNaming) -> Self {
        Self {
            video_file_name: naming.video_file_name(),
            extension: naming.extension().to_owned(),
        }
    }

    /// The folder the scenes of this video are written into until all of
    /// them are there. Told apart by the video's name, since two videos of
    /// one folder can be split at the same time, but by a number made from
    /// the name and not the name itself, to keep the path short.
    fn unfinished_folder(&self) -> String {
        format!(
            "{UNFINISHED_FOLDER_PREFIX}{:016x}",
            fnv1a_64(self.video_file_name.as_bytes())
        )
    }

    /// What a scene is called while it is in the unfinished folder.
    fn unfinished_name(&self, number: usize) -> String {
        format!("{number:0UNFINISHED_NUMBER_DIGITS$}.{}", self.extension)
    }

    /// The files a split into `scene_count` scenes writes into
    /// `unfinished_dir`, in scene order.
    fn unfinished_files_in(&self, unfinished_dir: &Path, scene_count: usize) -> Vec<PathBuf> {
        (FIRST_SCENE_NUMBER..FIRST_SCENE_NUMBER + scene_count)
            .map(|number| unfinished_dir.join(self.unfinished_name(number)))
            .collect()
    }

    /// The name ffmpeg is given for the files of `unfinished_files_in`. It
    /// reads the name as a printf pattern and fills in the scene number, so
    /// a literal percent sign has to be doubled.
    fn ffmpeg_pattern_in(&self, unfinished_dir: &Path) -> PathBuf {
        unfinished_dir.join(format!(
            "%0{UNFINISHED_NUMBER_DIGITS}d.{}",
            self.extension.replace('%', "%%")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenes_go_beside_the_video_by_default() {
        assert_eq!(
            scene_dir(Path::new("videos/holiday.mp4"), SceneLocation::BesideVideo).unwrap(),
            Path::new("videos")
        );
    }

    #[test]
    fn a_video_in_the_current_folder_keeps_its_scenes_there() {
        assert_eq!(
            scene_dir(Path::new("holiday.mp4"), SceneLocation::BesideVideo).unwrap(),
            Path::new(".")
        );
    }

    #[test]
    fn a_new_folder_is_named_after_the_video() {
        assert_eq!(
            scene_dir(Path::new("videos/holiday.mp4"), SceneLocation::NewFolder).unwrap(),
            Path::new("videos/holiday_scenes")
        );
    }

    fn pattern(typed: &str) -> SceneNamePattern {
        SceneNamePattern::new(typed).unwrap()
    }

    fn unfinished_names_of(video: &str) -> UnfinishedNames {
        let naming = SceneNaming::of(Path::new(video), SceneNamePattern::default()).unwrap();
        UnfinishedNames::of(&naming)
    }

    /// Scenes of ten seconds each, one after another.
    fn scenes(count: usize) -> Vec<TimeSpan> {
        (0..count)
            .map(|scene| TimeSpan {
                start: scene as f64 * 10.0,
                end: (scene + 1) as f64 * 10.0,
            })
            .collect()
    }

    #[test]
    fn scene_files_are_numbered_from_one() {
        let files = scene_file_paths(
            Path::new("videos/holiday.mp4"),
            Path::new("out"),
            SceneNamePattern::default(),
            &scenes(2),
        )
        .unwrap();
        assert_eq!(
            files,
            vec![
                Path::new("out").join("holiday_scene_1_of_2.mp4"),
                Path::new("out").join("holiday_scene_2_of_2.mp4"),
            ]
        );
    }

    #[test]
    fn scene_files_take_the_name_the_person_chose() {
        let files = scene_file_paths(
            Path::new("videos/holiday.mp4"),
            Path::new("out"),
            pattern("{number} of {count} - {name}"),
            &scenes(2),
        )
        .unwrap();
        assert_eq!(
            files,
            vec![
                Path::new("out").join("1 of 2 - holiday.mp4"),
                Path::new("out").join("2 of 2 - holiday.mp4"),
            ]
        );
    }

    #[test]
    fn the_default_scene_names_are_those_the_folder_scan_recognises() {
        let files = scene_file_paths(
            Path::new("holiday.mp4"),
            Path::new("out"),
            SceneNamePattern::default(),
            &scenes(3),
        )
        .unwrap();
        let names = SceneNames::from(SceneNamePattern::default());
        assert!(files
            .iter()
            .all(|file| names.is_scene_file_of_some_video(file)));
    }

    #[test]
    fn a_scene_file_named_like_its_video_is_refused() {
        let video = Path::new("videos/holiday.mp4");
        let error =
            refuse_to_write_over_the_video(video, &[PathBuf::from("out/holiday.mp4")]).unwrap_err();
        assert!(error.to_string().contains("name of the video itself"));
        refuse_to_write_over_the_video(video, &[PathBuf::from("out/holiday_scene_001.mp4")])
            .unwrap();
    }

    #[test]
    fn a_scene_file_named_like_its_video_but_for_capitals_is_refused_where_those_are_one_file() {
        let refused = refuse_to_write_over_the_video(
            Path::new("Holiday.MP4"),
            &[PathBuf::from("holiday.mp4")],
        );
        assert_eq!(
            refused.is_err(),
            SYSTEM_PATH_CASE == crate::folder_settings::PathCase::Ignored
        );
    }

    #[test]
    fn the_ffmpeg_pattern_names_the_files_of_the_unfinished_folder() {
        let name = unfinished_names_of("100% fun.mkv");
        let unfinished_dir = Path::new("out").join(name.unfinished_folder());

        // The video's name, percent sign and all, is in neither.
        assert_eq!(
            name.ffmpeg_pattern_in(&unfinished_dir),
            unfinished_dir.join("%03d.mkv")
        );
        assert_eq!(
            name.unfinished_files_in(&unfinished_dir, 2),
            vec![
                unfinished_dir.join("001.mkv"),
                unfinished_dir.join("002.mkv")
            ]
        );
    }

    #[test]
    fn the_unfinished_folder_is_short_and_differs_from_video_to_video() {
        let long_name = format!("{}.mp4", "a very long name of a video ".repeat(6));
        let long = unfinished_names_of(&long_name);
        let holiday = unfinished_names_of("holiday.mp4");
        let same_name_other_kind = unfinished_names_of("holiday.mkv");

        assert_eq!(long.unfinished_folder().len(), "unfinished_".len() + 16);
        assert_ne!(long.unfinished_folder(), holiday.unfinished_folder());
        assert_ne!(
            holiday.unfinished_folder(),
            same_name_other_kind.unfinished_folder()
        );
        // The same video gets the same folder every time, so what a killed
        // run left behind is found and removed by the next one.
        assert_eq!(
            holiday.unfinished_folder(),
            unfinished_names_of("elsewhere/holiday.mp4").unfinished_folder()
        );
    }

    #[test]
    fn an_unfinished_scene_has_a_shorter_path_than_the_finished_one() {
        // As long as the name of the clip that showed the problem: 64 characters.
        let video =
            Path::new("A long film name (2022)- with spaces and bracket - whole scene 1.mp4");
        let name = unfinished_names_of(&video.display().to_string());
        let out_dir = Path::new("out");

        let finished =
            &scene_file_paths(video, out_dir, SceneNamePattern::default(), &scenes(1)).unwrap()[0];
        let unfinished = &name.unfinished_files_in(&out_dir.join(name.unfinished_folder()), 1)[0];
        assert!(unfinished.as_os_str().len() < finished.as_os_str().len());
    }

    #[test]
    fn no_earlier_scenes_in_a_folder_that_does_not_exist() {
        let earlier = scene_files_from_earlier_splits(
            Path::new("holiday.mp4"),
            Path::new("no/such/folder"),
            SceneNamePattern::default(),
        )
        .unwrap();
        assert!(earlier.is_empty());
    }

    /// A folder of its own for one test, removed when the test ends.
    struct Folder(PathBuf);

    impl Folder {
        fn with_files(name: &str, file_names: &[&str]) -> Self {
            let folder = std::env::temp_dir().join(format!(
                "scene_splitter_split_test_{}_{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            for file_name in file_names {
                std::fs::write(folder.join(file_name), b"").unwrap();
            }
            Self(folder)
        }

        fn write(&self, file_name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(file_name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            path
        }

        fn read(&self, file_name: &str) -> String {
            std::fs::read_to_string(self.0.join(file_name)).unwrap()
        }

        fn file_names(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn earlier_scenes_of_this_video_are_found_whatever_their_number() {
        let folder = Folder::with_files(
            "earlier",
            &[
                "holiday.mp4",
                "holiday_scene_1_of_7.mp4",
                "holiday_scene_7_of_7.mp4",
                "holiday_scene_1_of_7.mkv",
                "holiday2_scene_1_of_7.mp4",
                "holiday_scene_8_of_7.mp4",
                "holiday_scene_notes.mp4",
            ],
        );
        let earlier = scene_files_from_earlier_splits(
            &folder.0.join("holiday.mp4"),
            &folder.0,
            SceneNamePattern::default(),
        )
        .unwrap();
        assert_eq!(
            earlier,
            vec![
                folder.0.join("holiday_scene_1_of_7.mp4"),
                folder.0.join("holiday_scene_7_of_7.mp4"),
            ]
        );
    }

    #[test]
    fn earlier_scenes_are_found_under_the_name_in_use_and_under_the_names_before_it() {
        let folder = Folder::with_files(
            "earlier_named",
            &[
                "holiday.mp4",
                "holiday_scene_001.mp4",
                "holiday_scene_1_of_3.mp4",
                "holiday part 001.mp4",
                "holiday part 012.mp4",
                "holiday2 part 001.mp4",
                "001 holiday.mp4",
                "002 holiday.mp4",
                "003 holiday trip.mp4",
            ],
        );
        let mut used_before = scene_name::PatternsUsed::default();
        used_before.remember(pattern("{number} {name}"));
        let names = SceneNames::new(pattern("{name} part {number}"), used_before);

        let earlier = earlier_splits_in(&folder.0.join("holiday.mp4"), &folder.0, names).unwrap();
        let found: Vec<(scene_name::EarlierNaming, Vec<String>)> = earlier
            .iter()
            .map(|split| (split.naming(), split.file_names()))
            .collect();
        let named = |file_names: &[&str]| -> Vec<String> {
            file_names.iter().map(|name| (*name).to_owned()).collect()
        };
        assert_eq!(
            found,
            vec![
                (
                    scene_name::EarlierNaming::AsScenesAreNamedNow,
                    named(&["holiday part 001.mp4", "holiday part 012.mp4"])
                ),
                (
                    scene_name::EarlierNaming::AnotherWay,
                    named(&["001 holiday.mp4", "002 holiday.mp4"])
                ),
                (
                    scene_name::EarlierNaming::AnotherWay,
                    named(&["holiday_scene_1_of_3.mp4"])
                ),
                (
                    scene_name::EarlierNaming::AnotherWay,
                    named(&["holiday_scene_001.mp4"])
                ),
            ]
        );
    }

    #[test]
    fn kept_scenes_get_numbers_without_gaps_and_dropped_ones_are_recorded() {
        let folder = Folder::with_files("settle", &[]);
        let unfinished = [
            folder.write("unfinished_0/001.mp4", "advert"),
            folder.write("unfinished_0/002.mp4", "first part"),
            folder.write("unfinished_0/003.mp4", "credits"),
            folder.write("unfinished_0/004.mp4", "second part"),
        ];
        let fates = [
            SceneFate::Drop,
            SceneFate::Keep,
            SceneFate::Drop,
            SceneFate::Keep,
        ];
        let scene_files = [
            folder.0.join("holiday_scene_001.mp4"),
            folder.0.join("holiday_scene_002.mp4"),
        ];
        let read_it = |dropped: &Path| -> Result<String> { Ok(std::fs::read_to_string(dropped)?) };

        let outcomes = settle_scenes(&unfinished, &fates, &scene_files, &read_it).unwrap();
        assert_eq!(
            outcomes,
            vec![
                SceneOutcome::Dropped("advert".to_owned()),
                SceneOutcome::Written(scene_files[0].clone()),
                SceneOutcome::Dropped("credits".to_owned()),
                SceneOutcome::Written(scene_files[1].clone()),
            ]
        );
        assert_eq!(written_files(&outcomes), scene_files);
        assert_eq!(folder.read("holiday_scene_001.mp4"), "first part");
        assert_eq!(folder.read("holiday_scene_002.mp4"), "second part");
    }

    #[test]
    fn a_dropped_scene_that_cannot_be_recorded_stops_the_split_before_anything_moves() {
        let folder = Folder::with_files("settle_fails", &[]);
        let earlier = folder.write("holiday_scene_001.mp4", "earlier split");
        let unfinished = [
            folder.write("unfinished_0/001.mp4", "first part"),
            folder.write("unfinished_0/002.mp4", "credits"),
        ];
        let fates = [SceneFate::Keep, SceneFate::Drop];
        let cannot_read = |_dropped: &Path| -> Result<()> { bail!("unreadable") };

        let error = settle_scenes(&unfinished, &fates, &[earlier], &cannot_read).unwrap_err();
        assert_eq!(error.to_string(), "unreadable");
        assert_eq!(folder.read("holiday_scene_001.mp4"), "earlier split");
    }

    #[test]
    fn scenes_left_over_from_a_longer_split_are_removed() {
        let folder = Folder::with_files(
            "leftover",
            &[
                "holiday.mp4",
                "holiday_scene_001.mp4",
                "holiday_scene_002.mp4",
                "holiday_scene_003.mp4",
            ],
        );
        let earlier = earlier_splits_in(
            &folder.0.join("holiday.mp4"),
            &folder.0,
            pattern("{name}_scene_{number}"),
        )
        .unwrap();
        let new_scenes = [
            folder.0.join("holiday_scene_001.mp4"),
            folder.0.join("holiday_scene_002.mp4"),
        ];

        remove_leftover_scenes(
            &earlier,
            &new_scenes,
            PathCase::Matters,
            delete_leftover_of_a_test,
        )
        .unwrap();
        assert_eq!(
            folder.file_names(),
            vec![
                "holiday.mp4",
                "holiday_scene_001.mp4",
                "holiday_scene_002.mp4"
            ]
        );
    }

    #[test]
    fn every_scene_of_a_split_named_another_way_is_removed_and_nothing_else() {
        let folder = Folder::with_files(
            "leftover_named_another_way",
            &[
                "holiday.mp4",
                // An earlier split under the old default name.
                "holiday_scene_001.mp4",
                "holiday_scene_002.mp4",
                "holiday_scene_003.mp4",
                // An earlier split into five, of which two files are left.
                "holiday_scene_4_of_5.mp4",
                "holiday_scene_5_of_5.mp4",
                // What the new split has just written.
                "holiday_scene_1_of_2.mp4",
                "holiday_scene_2_of_2.mp4",
                // Other videos, which only look alike.
                "holiday_scene_1_of_50_extended.mp4",
                "holiday_scene_2.mp4",
                "holiday_scene_9_of_5.mp4",
                "holiday trip_scene_001.mp4",
            ],
        );
        let earlier = earlier_splits_in(
            &folder.0.join("holiday.mp4"),
            &folder.0,
            SceneNamePattern::default(),
        )
        .unwrap();
        let new_scenes = [
            folder.0.join("holiday_scene_1_of_2.mp4"),
            folder.0.join("holiday_scene_2_of_2.mp4"),
        ];

        remove_leftover_scenes(
            &earlier,
            &new_scenes,
            PathCase::Matters,
            delete_leftover_of_a_test,
        )
        .unwrap();
        assert_eq!(
            folder.file_names(),
            vec![
                "holiday trip_scene_001.mp4",
                "holiday.mp4",
                "holiday_scene_1_of_2.mp4",
                "holiday_scene_1_of_50_extended.mp4",
                "holiday_scene_2.mp4",
                "holiday_scene_2_of_2.mp4",
                "holiday_scene_9_of_5.mp4",
            ]
        );
    }

    #[test]
    fn a_leftover_that_cannot_be_moved_away_stops_with_the_reason() {
        let earlier_files = [PathBuf::from("out/holiday_scene_003.mp4")];
        let earlier =
            scene_name::earlier_splits_for_test("holiday.mp4", &["out/holiday_scene_003.mp4"]);
        assert_eq!(files_of(&earlier), earlier_files);
        let no_bin: MoveLeftoverAway = |_leftover| bail!("there is no bin on this drive");

        let error = remove_leftover_scenes(&earlier, &[], PathCase::Matters, no_bin).unwrap_err();
        assert_eq!(error.to_string(), "there is no bin on this drive");
        // A scene the new split wrote under the same name is no leftover.
        remove_leftover_scenes(&earlier, &earlier_files, PathCase::Matters, no_bin).unwrap();
    }

    #[test]
    fn a_scene_written_over_an_earlier_one_is_no_leftover_whatever_its_capitals() {
        let earlier = scene_name::earlier_splits_for_test(
            "Holiday.mp4",
            &["Holiday_scene_1_of_2.mp4", "Holiday_scene_2_of_2.mp4"],
        );
        // The video was renamed in between, to the same letters in small.
        let new_scenes = [
            PathBuf::from("holiday_scene_1_of_2.mp4"),
            PathBuf::from("holiday_scene_2_of_2.mp4"),
        ];
        let earlier_split = &earlier[0];

        // Where capitals do not count, the new scenes are those very files.
        assert_eq!(
            leftovers_of(earlier_split, &new_scenes, PathCase::Ignored),
            Vec::<&Path>::new()
        );
        // Where they do, the earlier files are files of their own.
        assert_eq!(
            leftovers_of(earlier_split, &new_scenes, PathCase::Matters),
            earlier_split.files()
        );
    }

    #[test]
    fn a_file_where_a_scene_would_go_is_only_replaced_when_it_is_an_earlier_scene() {
        let folder = Folder::with_files(
            "other_file_in_the_way",
            &["film_0001.mp4", "film_0002.mp4", "film_1000.mp4"],
        );
        let video = folder.0.join("film.mp4");
        let new_scenes = [
            folder.0.join("film_0001.mp4"),
            folder.0.join("film_1000.mp4"),
        ];

        // Found as scenes of an earlier split, they may be replaced.
        let found = earlier_splits_in(&video, &folder.0, pattern("{name}_{number}")).unwrap();
        refuse_to_write_over_other_files(&new_scenes, &found, PathCase::Matters).unwrap();
        // A scene that goes where no file is yet is in nobody's way.
        let free = [folder.0.join("film_0500.mp4")];
        refuse_to_write_over_other_files(&free, &[], PathCase::Matters).unwrap();

        // Not found as one, a file is left alone and the split does not start.
        let error =
            refuse_to_write_over_other_files(&new_scenes, &[], PathCase::Matters).unwrap_err();
        assert!(
            error.to_string().contains("film_0001.mp4")
                && error
                    .to_string()
                    .contains("does not look like a scene file"),
            "{error}"
        );
    }

    #[test]
    fn another_video_beside_the_one_being_split_is_not_an_earlier_scene() {
        let folder = Folder::with_files(
            "neighbours",
            &[
                "Rocky.mp4",
                "Rocky 2.mp4",
                "Rocky 2049.mp4",
                "Rocky 001.mp4",
                "Rocky 002.mp4",
            ],
        );
        let earlier = earlier_splits_in(
            &folder.0.join("Rocky.mp4"),
            &folder.0,
            pattern("{name} {number}"),
        )
        .unwrap();
        assert_eq!(earlier.len(), 1);
        assert_eq!(earlier[0].file_names(), ["Rocky 001.mp4", "Rocky 002.mp4"]);
    }

    #[test]
    fn finished_scenes_take_the_place_of_those_of_an_earlier_split() {
        let folder = Folder::with_files("into_place", &[]);
        let earlier = folder.write("holiday_scene_001.mp4", "earlier split");
        let second = folder.0.join("holiday_scene_002.mp4");
        let unfinished = [
            folder.write("unfinished_0/001.mp4", "new one"),
            folder.write("unfinished_0/002.mp4", "new two"),
        ];

        move_into_place(&unfinished, &[earlier, second]).unwrap();
        assert_eq!(folder.read("holiday_scene_001.mp4"), "new one");
        assert_eq!(folder.read("holiday_scene_002.mp4"), "new two");
        assert!(unfinished.iter().all(|file| !file.exists()));
    }

    #[test]
    fn a_scene_that_was_never_written_cannot_be_put_in_place() {
        let folder = Folder::with_files("never_written", &["holiday_scene_001.mp4"]);
        let never_written = folder.0.join("unfinished_0").join("001.mp4");

        let error = move_into_place(&[never_written], &[folder.0.join("holiday_scene_001.mp4")])
            .unwrap_err();
        assert!(format!("{error:#}").contains("could not put"), "{error:#}");
        assert_eq!(folder.file_names(), vec!["holiday_scene_001.mp4"]);
    }

    #[test]
    fn unfinished_scenes_are_removed_with_their_folder() {
        let folder = Folder::with_files("unfinished", &["holiday_scene_001.mp4"]);
        folder.write("unfinished_0/001.mp4", "half");
        let unfinished_dir = folder.0.join("unfinished_0");

        remove_unfinished_scenes(&unfinished_dir).unwrap();
        assert_eq!(folder.file_names(), vec!["holiday_scene_001.mp4"]);
        // Nothing to remove is not a failure.
        remove_unfinished_scenes(&unfinished_dir).unwrap();
    }
}
