//! Writes one file per scene by copying the compressed streams. Nothing is
//! re-encoded, so a two hour video splits in seconds and loses no quality.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::tools;

/// The segment muxer starts a new file at the first keyframe at or after each
/// cut time. Asking slightly early keeps timestamp rounding from pushing a cut
/// to the following keyframe.
const CUT_MARGIN_SECONDS: f64 = 0.001;

const FIRST_SCENE_NUMBER: usize = 1;
const SCENE_NUMBER_DIGITS: usize = 3;
/// Scene files are named "<video name>_scene_001.<extension>".
const SCENE_NAME_MARKER: &str = "_scene_";
const NEW_FOLDER_SUFFIX: &str = "_scenes";

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

/// The files a split into `scene_count` scenes writes, in scene order.
pub fn scene_file_paths(video: &Path, out_dir: &Path, scene_count: usize) -> Result<Vec<PathBuf>> {
    let name = SceneFileName::of(video)?;
    Ok((FIRST_SCENE_NUMBER..FIRST_SCENE_NUMBER + scene_count)
        .map(|number| out_dir.join(name.numbered(number)))
        .collect())
}

/// Every scene file of this video already in `out_dir`, whatever its number.
///
/// A new split replaces all of them, including numbers it does not use
/// itself, so callers ask the person before going ahead when this is not
/// empty.
pub fn scene_files_from_earlier_splits(video: &Path, out_dir: &Path) -> Result<Vec<PathBuf>> {
    let name = SceneFileName::of(video)?;
    if !out_dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(out_dir)
        .with_context(|| format!("could not read {}", out_dir.display()))?;
    let mut scene_files = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("could not read {}", out_dir.display()))?
            .path();
        if path.is_file() && name.is_scene_of_this_video(&path) {
            scene_files.push(path);
        }
    }
    scene_files.sort();
    Ok(scene_files)
}

/// Whether a file is a scene written by an earlier split. Scenes sit next to
/// the videos they came from, and a scene should not be split a second time
/// just because its folder was added again.
pub fn is_scene_file(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let Some((video_name, number)) = stem.rsplit_once(SCENE_NAME_MARKER) else {
        return false;
    };
    !video_name.is_empty()
        && number.len() == SCENE_NUMBER_DIGITS
        && number.chars().all(|digit| digit.is_ascii_digit())
}

/// Splits `input` at the given keyframe times and returns the scene files in
/// order. Scene files of this video from an earlier split are replaced, and
/// those with numbers the new split does not use are removed.
pub fn split_at_keyframes(input: &Path, cuts: &[f64], out_dir: &Path) -> Result<Vec<PathBuf>> {
    let name = SceneFileName::of(input)?;
    let earlier_scene_files = scene_files_from_earlier_splits(input, out_dir)?;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("could not create {}", out_dir.display()))?;

    let cut_times = cuts
        .iter()
        .map(|cut| format!("{:.6}", cut - CUT_MARGIN_SECONDS))
        .collect::<Vec<_>>()
        .join(",");

    let status = tools::ffmpeg_command()
        .args(["-nostdin", "-v", "error", "-y"])
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0", "-map", "0:a?", "-c", "copy"])
        .args(["-f", "segment", "-segment_times", &cut_times])
        .args(["-segment_start_number", &FIRST_SCENE_NUMBER.to_string()])
        // Each scene file starts at time zero instead of its time in the source.
        .args(["-reset_timestamps", "1"])
        .arg(out_dir.join(name.ffmpeg_pattern()))
        .stdin(Stdio::null())
        .status()
        .context("could not start ffmpeg, is it on PATH?")?;
    if !status.success() {
        bail!("ffmpeg could not split {} ({status})", input.display());
    }

    let scene_files = scene_file_paths(input, out_dir, cuts.len() + 1)?;
    if let Some(missing) = scene_files.iter().find(|file| !file.is_file()) {
        bail!("ffmpeg finished without writing {}", missing.display());
    }
    remove_leftover_scenes(&earlier_scene_files, &scene_files)?;
    Ok(scene_files)
}

/// An earlier split into more scenes leaves higher-numbered files that the
/// new split did not overwrite. Left in place they would sit beside the new
/// scenes and repeat part of the last one.
fn remove_leftover_scenes(
    earlier_scene_files: &[PathBuf],
    new_scene_files: &[PathBuf],
) -> Result<()> {
    for leftover in earlier_scene_files
        .iter()
        .filter(|earlier| !new_scene_files.contains(earlier))
    {
        std::fs::remove_file(leftover)
            .with_context(|| format!("could not remove the outdated {}", leftover.display()))?;
    }
    Ok(())
}

/// The naming of one video's scene files.
struct SceneFileName {
    stem: String,
    extension: String,
}

impl SceneFileName {
    fn of(video: &Path) -> Result<Self> {
        let stem = video
            .file_stem()
            .and_then(|stem| stem.to_str())
            .with_context(|| format!("{} has no usable file name", video.display()))?;
        let extension = video
            .extension()
            .and_then(|extension| extension.to_str())
            .with_context(|| format!("{} has no file extension", video.display()))?;
        Ok(Self {
            stem: stem.to_owned(),
            extension: extension.to_owned(),
        })
    }

    /// Whether `path` is named "<this video>_scene_<digits>.<extension>".
    fn is_scene_of_this_video(&self, path: &Path) -> bool {
        let same_extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(&self.extension));
        let scene_number = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.strip_prefix(&self.stem))
            .and_then(|rest| rest.strip_prefix(SCENE_NAME_MARKER));
        same_extension
            && scene_number.is_some_and(|number| {
                number.len() == SCENE_NUMBER_DIGITS
                    && number.chars().all(|digit| digit.is_ascii_digit())
            })
    }

    fn numbered(&self, number: usize) -> String {
        format!(
            "{}{SCENE_NAME_MARKER}{number:0SCENE_NUMBER_DIGITS$}.{}",
            self.stem, self.extension
        )
    }

    /// ffmpeg reads the output name as a printf pattern, so a literal percent
    /// sign in the video's name has to be doubled.
    fn ffmpeg_pattern(&self) -> String {
        format!(
            "{}{SCENE_NAME_MARKER}%0{SCENE_NUMBER_DIGITS}d.{}",
            self.stem.replace('%', "%%"),
            self.extension
        )
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

    #[test]
    fn scene_files_are_numbered_from_one() {
        let files = scene_file_paths(Path::new("videos/holiday.mp4"), Path::new("out"), 2).unwrap();
        assert_eq!(
            files,
            vec![
                Path::new("out").join("holiday_scene_001.mp4"),
                Path::new("out").join("holiday_scene_002.mp4"),
            ]
        );
    }

    #[test]
    fn the_ffmpeg_pattern_matches_the_numbered_names() {
        let name = SceneFileName::of(Path::new("100% fun.mkv")).unwrap();
        assert_eq!(name.ffmpeg_pattern(), "100%% fun_scene_%03d.mkv");
        assert_eq!(name.numbered(7), "100% fun_scene_007.mkv");
    }

    #[test]
    fn scenes_of_an_earlier_split_are_recognised() {
        assert!(is_scene_file(Path::new("videos/holiday_scene_001.mp4")));
        assert!(is_scene_file(Path::new("holiday_scene_123.mkv")));
    }

    #[test]
    fn ordinary_videos_are_not_mistaken_for_scenes() {
        assert!(!is_scene_file(Path::new("holiday.mp4")));
        assert!(!is_scene_file(Path::new("crime_scene_investigation.mp4")));
        assert!(!is_scene_file(Path::new("_scene_001.mp4")));
        assert!(!is_scene_file(Path::new("holiday_scene_1.mp4")));
    }

    #[test]
    fn no_earlier_scenes_in_a_folder_that_does_not_exist() {
        let earlier =
            scene_files_from_earlier_splits(Path::new("holiday.mp4"), Path::new("no/such/folder"))
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
                "holiday_scene_001.mp4",
                "holiday_scene_007.mp4",
                "holiday_scene_001.mkv",
                "holiday2_scene_001.mp4",
                "holiday_scene_notes.mp4",
            ],
        );
        let earlier =
            scene_files_from_earlier_splits(&folder.0.join("holiday.mp4"), &folder.0).unwrap();
        assert_eq!(
            earlier,
            vec![
                folder.0.join("holiday_scene_001.mp4"),
                folder.0.join("holiday_scene_007.mp4"),
            ]
        );
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
        let earlier =
            scene_files_from_earlier_splits(&folder.0.join("holiday.mp4"), &folder.0).unwrap();
        let new_scenes = [
            folder.0.join("holiday_scene_001.mp4"),
            folder.0.join("holiday_scene_002.mp4"),
        ];

        remove_leftover_scenes(&earlier, &new_scenes).unwrap();
        assert_eq!(
            folder.file_names(),
            vec![
                "holiday.mp4",
                "holiday_scene_001.mp4",
                "holiday_scene_002.mp4"
            ]
        );
    }
}
