//! Works out which videos a path stands for.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::split;

/// Files in a folder with one of these extensions count as videos.
const VIDEO_EXTENSIONS: [&str; 12] = [
    "mp4", "mkv", "mov", "m4v", "avi", "webm", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg",
];

/// The videos a path stands for: the file itself, or the videos directly
/// inside a folder, in name order.
///
/// Subfolders are not searched. Scene files written by earlier splits are
/// left out, so naming a folder again does not split its scenes a second
/// time. A file named directly is always taken, whatever it is called.
pub fn videos_at(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        bail!("{} does not exist.", path.display());
    }
    let mut videos = Vec::new();
    let entries =
        std::fs::read_dir(path).with_context(|| format!("could not read {}", path.display()))?;
    for entry in entries {
        let entry_path = entry
            .with_context(|| format!("could not read {}", path.display()))?
            .path();
        if entry_path.is_file()
            && has_video_extension(&entry_path)
            && !split::is_scene_file(&entry_path)
        {
            videos.push(entry_path);
        }
    }
    videos.sort();
    Ok(videos)
}

fn has_video_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own for one test, removed when the test ends.
    struct Folder(PathBuf);

    impl Folder {
        fn with_files(name: &str, file_names: &[&str]) -> Self {
            let folder = std::env::temp_dir().join(format!(
                "scene_splitter_library_test_{}_{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(folder.join("subfolder")).unwrap();
            for file_name in file_names {
                std::fs::write(folder.join(file_name), b"").unwrap();
            }
            Self(folder)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn file_names(videos: &[PathBuf]) -> Vec<String> {
        videos
            .iter()
            .map(|video| video.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn video_files_are_recognised_whatever_the_case() {
        assert!(has_video_extension(Path::new("film.MP4")));
        assert!(has_video_extension(Path::new("clip.mkv")));
    }

    #[test]
    fn other_files_are_not_videos() {
        assert!(!has_video_extension(Path::new("notes.txt")));
        assert!(!has_video_extension(Path::new("no_extension")));
    }

    #[test]
    fn a_path_that_does_not_exist_is_an_error() {
        assert!(videos_at(Path::new("no/such/place.mp4")).is_err());
    }

    #[test]
    fn a_folder_gives_its_videos_in_name_order_without_earlier_scenes() {
        let folder = Folder::with_files(
            "folder",
            &[
                "b.mkv",
                "a.mp4",
                "notes.txt",
                "a_scene_001.mp4",
                "a_scene_002.mp4",
            ],
        );
        let videos = videos_at(&folder.0).unwrap();
        assert_eq!(file_names(&videos), vec!["a.mp4", "b.mkv"]);
    }

    #[test]
    fn a_file_named_directly_is_taken_as_it_is() {
        let folder = Folder::with_files("direct", &["a_scene_001.mp4"]);
        let scene = folder.0.join("a_scene_001.mp4");
        assert_eq!(videos_at(&scene).unwrap(), vec![scene]);
    }
}
