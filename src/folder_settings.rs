//! Detection settings a person asked the app to remember for one folder, so
//! videos from different sources are scanned in different ways.
//!
//! A video is scanned with the settings of the folder it is in when that
//! folder has some, and with the general settings otherwise. Only the exact
//! folder counts: a folder's settings do not reach into its subfolders. The
//! app never looks into subfolders when a folder is added either, and with
//! this rule the folder named on screen is always the whole answer to "why
//! was this video scanned like that".

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::detect::ScanSettings;
use crate::hardcut::HardCutSettings;

/// Everything that decides which scene changes a scan suggests: what counts
/// as a black screen, and how sudden picture changes are thinned.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectionSettings {
    pub scan: ScanSettings,
    pub hard_cuts: HardCutSettings,
}

impl DetectionSettings {
    /// Says what is wrong with values no scan could work with.
    pub fn validate(&self) -> Result<(), &'static str> {
        self.scan.criteria.validate()?;
        if !self.hard_cuts.min_scene_seconds.is_finite() || self.hard_cuts.min_scene_seconds <= 0.0
        {
            return Err("the shortest scene for hard cuts must be longer than zero");
        }
        Ok(())
    }
}

/// Whose detection settings a video is scanned with.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsScope {
    General,
    /// The settings remembered for this folder, named as it was remembered.
    Folder(PathBuf),
}

/// The detection settings in force for one video, and whose they are.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectionInUse {
    pub detection: DetectionSettings,
    pub scope: SettingsScope,
}

/// Whether two spellings of a folder that differ only in capitals are the
/// same folder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCase {
    Matters,
    Ignored,
}

/// Windows and macOS keep the capitals of a name but find a file whatever
/// capitals are used to ask for it. (A macOS disk can be set up otherwise,
/// which is rare.)
pub const SYSTEM_PATH_CASE: PathCase = if cfg!(any(windows, target_os = "macos")) {
    PathCase::Ignored
} else {
    PathCase::Matters
};

/// The folders that have detection settings of their own.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FolderSettings {
    /// At most one entry per folder, under any spelling of it.
    by_folder: BTreeMap<PathBuf, DetectionSettings>,
}

impl FolderSettings {
    pub fn is_empty(&self) -> bool {
        self.by_folder.is_empty()
    }

    /// Every folder with settings of its own, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&Path, &DetectionSettings)> {
        self.by_folder
            .iter()
            .map(|(folder, detection)| (folder.as_path(), detection))
    }

    /// The settings remembered for exactly this folder, with the folder as
    /// it was remembered. The settings of a folder above it do not count.
    pub fn for_folder(&self, folder: &Path) -> Option<(&Path, &DetectionSettings)> {
        self.for_folder_spelled(folder, SYSTEM_PATH_CASE)
    }

    fn for_folder_spelled(
        &self,
        folder: &Path,
        case: PathCase,
    ) -> Option<(&Path, &DetectionSettings)> {
        self.iter()
            .find(|(remembered, _)| is_same_folder(remembered, folder, case))
    }

    /// Remembers `detection` for `folder`, in place of what was remembered
    /// for it before.
    pub fn remember(&mut self, folder: &Path, detection: DetectionSettings) {
        self.remember_spelled(folder, detection, SYSTEM_PATH_CASE);
    }

    fn remember_spelled(&mut self, folder: &Path, detection: DetectionSettings, case: PathCase) {
        self.forget_spelled(folder, case);
        self.by_folder.insert(folder.to_path_buf(), detection);
    }

    /// From now on videos in `folder` are scanned with the general settings.
    pub fn forget(&mut self, folder: &Path) {
        self.forget_spelled(folder, SYSTEM_PATH_CASE);
    }

    fn forget_spelled(&mut self, folder: &Path, case: PathCase) {
        self.by_folder
            .retain(|remembered, _| !is_same_folder(remembered, folder, case));
    }
}

/// Whether two paths name the same folder, without asking the disk. A
/// trailing separator makes no difference.
fn is_same_folder(one: &Path, other: &Path, case: PathCase) -> bool {
    let is_same_part = |one: Component, other: Component| match case {
        PathCase::Matters => one == other,
        PathCase::Ignored => one
            .as_os_str()
            .to_string_lossy()
            .to_lowercase()
            .eq(&other.as_os_str().to_string_lossy().to_lowercase()),
    };
    let mut one_parts = one.components();
    let mut other_parts = other.components();
    loop {
        match (one_parts.next(), other_parts.next()) {
            (None, None) => return true,
            (Some(one_part), Some(other_part)) if is_same_part(one_part, other_part) => {}
            _ => return false,
        }
    }
}

/// The folder a video is in, written out in full, so that a video named by
/// a short path on the command line and the same video dropped on the window
/// belong to the same folder. `None` for a path that names no file in any
/// folder, such as an empty one.
pub fn folder_of(video: &Path) -> Option<PathBuf> {
    let written_out = std::path::absolute(video).ok()?;
    written_out.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::ScanMode;
    use crate::hardcut::Sensitivity;

    /// Settings no default would give, to tell them apart from the general
    /// ones.
    fn grainy_footage() -> DetectionSettings {
        let mut detection = DetectionSettings::default();
        detection.scan.mode = ScanMode::EveryFrame;
        detection.scan.criteria.max_dark_luma = 48;
        detection.hard_cuts.sensitivity = Sensitivity::High;
        detection
    }

    fn short_black_screens() -> DetectionSettings {
        let mut detection = DetectionSettings::default();
        detection.scan.criteria.min_seconds = 0.1;
        detection
    }

    fn remembered_for(folder: &str, detection: DetectionSettings) -> FolderSettings {
        let mut folders = FolderSettings::default();
        folders.remember_spelled(Path::new(folder), detection, PathCase::Matters);
        folders
    }

    fn found(folders: &FolderSettings, folder: &str, case: PathCase) -> Option<DetectionSettings> {
        folders
            .for_folder_spelled(Path::new(folder), case)
            .map(|(_, detection)| *detection)
    }

    #[test]
    fn a_folder_with_settings_of_its_own_gets_them() {
        let folders = remembered_for("videos/tapes", grainy_footage());
        assert_eq!(
            found(&folders, "videos/tapes", PathCase::Matters),
            Some(grainy_footage())
        );
    }

    #[test]
    fn a_folder_nothing_was_remembered_for_gets_nothing() {
        let folders = remembered_for("videos/tapes", grainy_footage());
        assert_eq!(found(&folders, "videos/films", PathCase::Matters), None);
        assert_eq!(
            found(&FolderSettings::default(), "videos", PathCase::Matters),
            None
        );
    }

    #[test]
    fn subfolders_do_not_inherit_and_neither_do_folders_above() {
        let folders = remembered_for("videos/tapes", grainy_footage());
        assert_eq!(
            found(&folders, "videos/tapes/1994", PathCase::Matters),
            None
        );
        assert_eq!(found(&folders, "videos", PathCase::Matters), None);
    }

    #[test]
    fn a_folder_whose_name_only_begins_like_another_is_another_folder() {
        let folders = remembered_for("videos/tapes", grainy_footage());
        assert_eq!(found(&folders, "videos/tapes2", PathCase::Matters), None);
    }

    #[test]
    fn a_trailing_separator_names_the_same_folder() {
        let folders = remembered_for("videos/tapes", grainy_footage());
        assert_eq!(
            found(&folders, "videos/tapes/", PathCase::Matters),
            Some(grainy_footage())
        );
    }

    #[test]
    fn capitals_tell_folders_apart_only_where_the_system_does() {
        let folders = remembered_for("Videos/Tapes", grainy_footage());
        assert_eq!(found(&folders, "videos/tapes", PathCase::Matters), None);
        assert_eq!(
            found(&folders, "videos/tapes", PathCase::Ignored),
            Some(grainy_footage())
        );
    }

    #[test]
    fn the_folder_is_named_as_it_was_remembered() {
        let folders = remembered_for("Videos/Tapes", grainy_footage());
        let (remembered, _) = folders
            .for_folder_spelled(Path::new("videos/tapes"), PathCase::Ignored)
            .unwrap();
        assert_eq!(remembered, Path::new("Videos/Tapes"));
    }

    #[test]
    fn remembering_again_replaces_what_was_remembered() {
        let mut folders = remembered_for("Videos/Tapes", grainy_footage());
        folders.remember_spelled(
            Path::new("videos/tapes"),
            short_black_screens(),
            PathCase::Ignored,
        );

        assert_eq!(folders.iter().count(), 1);
        assert_eq!(
            found(&folders, "Videos/Tapes", PathCase::Ignored),
            Some(short_black_screens())
        );
    }

    #[test]
    fn each_folder_keeps_its_own_settings() {
        let mut folders = remembered_for("videos/tapes", grainy_footage());
        folders.remember_spelled(
            Path::new("videos/films"),
            short_black_screens(),
            PathCase::Matters,
        );

        assert_eq!(
            found(&folders, "videos/tapes", PathCase::Matters),
            Some(grainy_footage())
        );
        assert_eq!(
            found(&folders, "videos/films", PathCase::Matters),
            Some(short_black_screens())
        );
    }

    #[test]
    fn a_forgotten_folder_has_no_settings_and_the_others_keep_theirs() {
        let mut folders = remembered_for("videos/tapes", grainy_footage());
        folders.remember_spelled(
            Path::new("videos/films"),
            short_black_screens(),
            PathCase::Matters,
        );

        folders.forget_spelled(Path::new("videos/tapes"), PathCase::Matters);
        assert_eq!(found(&folders, "videos/tapes", PathCase::Matters), None);
        assert_eq!(folders.iter().count(), 1);
        assert!(!folders.is_empty());

        // Forgetting a folder that has none is harmless.
        folders.forget_spelled(Path::new("videos/tapes"), PathCase::Matters);
        assert_eq!(folders.iter().count(), 1);
    }

    #[test]
    fn the_folder_of_a_video_is_written_out_in_full() {
        let folder = folder_of(Path::new("film.mp4")).unwrap();
        assert!(folder.is_absolute(), "{}", folder.display());
        assert_eq!(folder, std::env::current_dir().unwrap());

        let in_a_folder = std::env::temp_dir().join("tapes").join("film.mp4");
        assert_eq!(
            folder_of(&in_a_folder),
            Some(std::env::temp_dir().join("tapes"))
        );
    }

    #[test]
    fn folder_settings_are_stored_as_a_map_from_folder_to_settings() {
        let folders = remembered_for("videos/tapes", short_black_screens());
        let stored = serde_json::to_value(&folders).unwrap();
        assert_eq!(
            stored["videos/tapes"]["scan"]["criteria"]["min_seconds"],
            0.1
        );
        assert_eq!(
            serde_json::from_value::<FolderSettings>(stored).unwrap(),
            folders
        );
    }

    #[test]
    fn detection_settings_no_scan_could_work_with_are_refused() {
        assert!(DetectionSettings::default().validate().is_ok());

        let mut no_black_screen = DetectionSettings::default();
        no_black_screen.scan.criteria.min_seconds = 0.0;
        assert!(no_black_screen.validate().is_err());

        let mut no_scene = DetectionSettings::default();
        no_scene.hard_cuts.min_scene_seconds = -1.0;
        assert!(no_scene.validate().is_err());
    }
}
