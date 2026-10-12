//! The choices a person makes once and expects the app to remember: how
//! black screens are detected, how hard cuts are thinned, where scenes go,
//! what their files are called, and whether the window is light or dark.
//! The window and the command line read the same file.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::export::{ExportOptions, OriginalPolicy};
use crate::folder_settings::{
    self, DetectionInUse, DetectionSettings, FolderSettings, SettingsScope,
};
use crate::folders;
use crate::scene_name::{PatternsUsed, SceneNamePattern, SceneNames};
use crate::theme::ThemeChoice;

const APP_FOLDER: &str = "scene_splitter";
const SETTINGS_FILE: &str = "settings.json";
/// The entry of the settings file that holds `Settings::scene_names_used`.
/// A file without it was written before the default name said the count.
const SCENE_NAMES_USED_ENTRY: &str = "scene_names_used";

/// Which version of the app wrote a settings file, as far as it matters
/// for the name of scene files.
#[derive(Debug, Clone, Copy, PartialEq)]
enum WrittenBy {
    /// The default name was "{name}_scene_{number}" then.
    AVersionBeforeTheCount,
    AVersionThatSaysTheCount,
}

/// A missing part of the file falls back to its default, so a file written
/// by an older version keeps working.
///
/// `detection` holds the general detection settings. A video in a folder
/// listed in `folders` is scanned with that folder's instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// One value, the same kind a folder remembers, so that a setting added
    /// to it is general and per folder at once. In the file its parts stand
    /// beside the other settings, as `scan` and `hard_cuts`, where they
    /// always stood.
    #[serde(flatten)]
    pub detection: DetectionSettings,
    pub export: ExportOptions,
    /// The folders whose videos are scanned with settings of their own.
    /// Empty in a file from before folders could have any.
    pub folders: FolderSettings,
    /// Whether the review screen explains how to use it.
    pub show_tips: bool,
    /// Whether the person has already been asked what should happen to the
    /// original of a video they split.
    pub recycling_question: RecyclingQuestion,
    /// Whether the window is light, dark, or as the system is. A file from
    /// before there was a choice follows the system.
    pub theme: ThemeChoice,
    /// The names splits were written with, the newest first. Not a choice
    /// the person makes: it is how the scene files of an earlier split are
    /// still found after the name for scene files was changed. Empty in a
    /// file from before names were remembered.
    pub scene_names_used: PatternsUsed,
}

/// The first split that would move an original to the Recycle Bin or Trash
/// is explained and confirmed. After that the person is not asked again.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum RecyclingQuestion {
    /// Also what a settings file from before the question existed means.
    #[default]
    NotAskedYet,
    Answered,
}

impl Default for Settings {
    /// Tips start on, so a first-time user is told what to do.
    fn default() -> Self {
        Self {
            detection: DetectionSettings::default(),
            export: ExportOptions::default(),
            folders: FolderSettings::default(),
            show_tips: true,
            recycling_question: RecyclingQuestion::NotAskedYet,
            theme: ThemeChoice::FollowSystem,
            scene_names_used: PatternsUsed::default(),
        }
    }
}

/// One change to the settings, asked for on a screen that only draws them.
/// Detection settings are not changed this way: whose they are depends on
/// the video, so that change names its video (`change_detection_for`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsChange {
    ExportOptions(ExportOptions),
    ShowTips,
    HideTips,
}

/// What loading the settings gave.
#[derive(Debug, PartialEq)]
pub struct LoadedSettings {
    pub settings: Settings,
    /// Why the saved file was not used, for the person to see.
    pub problem: Option<String>,
}

impl LoadedSettings {
    fn usable(settings: Settings) -> Self {
        Self {
            settings,
            problem: None,
        }
    }
}

impl Settings {
    /// The saved settings, or the defaults when nothing is saved.
    ///
    /// A file that cannot be used does not stop the app, but it must not
    /// quietly change what happens to originals either. The person may have
    /// chosen to keep them, and that choice is in the file that could not
    /// be read. So in that case originals are kept, and the caller is told.
    pub fn load() -> LoadedSettings {
        let Some(path) = settings_path() else {
            return LoadedSettings::usable(Self::default());
        };
        match Self::load_from(&path) {
            Ok(Some(settings)) => LoadedSettings::usable(settings),
            Ok(None) => LoadedSettings::usable(Self::default()),
            Err(error) => LoadedSettings {
                settings: Self::defaults_that_keep_originals(),
                problem: Some(format!(
                    "The saved settings in {} could not be used ({error:#}). \
                     The defaults are in use, and originals are kept until settings are saved again.",
                    path.display()
                )),
            },
        }
    }

    fn defaults_that_keep_originals() -> Self {
        let mut settings = Self::default();
        settings.export.original_policy = OriginalPolicy::Keep;
        settings
    }

    /// Whether a split started now has to wait for the person to say what
    /// happens to the original. Only a split that would recycle it does, and
    /// only until they have answered once.
    pub fn must_ask_before_recycling(&self) -> bool {
        // Asked of the split as it will be carried out: an exact split
        // keeps the original whatever the setting for originals says.
        self.export.split_method().original_policy() == OriginalPolicy::RecycleOnceVerified
            && self.recycling_question == RecyclingQuestion::NotAskedYet
    }

    /// The name scene files get now, with the names earlier splits gave
    /// theirs.
    pub fn scene_names(&self) -> SceneNames {
        SceneNames::new(self.export.scene_name, self.scene_names_used)
    }

    /// Notes that a split is being written with the name in use, so its
    /// scene files are still known as scenes after the name is changed.
    ///
    /// Remembered when a split is written and not when the name is typed:
    /// every stop on the way from one name to another is a usable name too,
    /// and remembering those would push out the names files really have.
    pub fn remember_scene_name_in_use(&mut self) {
        self.scene_names_used.remember(self.export.scene_name);
    }

    /// Every setting as it was at first, except what happens to originals
    /// and the names earlier splits were written with. What was remembered
    /// for folders is forgotten with the rest.
    ///
    /// The names are kept because they are no preference: the files written
    /// under them are still in the person's folders.
    ///
    /// What happens to originals is the person's answer to a question, not
    /// a preference with a default: someone who said "keep the original" and
    /// later resets the detection settings must not find their originals in
    /// the Recycle Bin. So their choice is kept, and with it the fact that
    /// they were asked.
    pub fn restored_to_defaults(&self) -> Self {
        let mut restored = Self {
            recycling_question: self.recycling_question,
            scene_names_used: self.scene_names_used,
            ..Self::default()
        };
        restored.export.original_policy = self.export.original_policy;
        restored
    }

    /// The settings after one change the person made on a screen.
    pub fn changed_by(&self, change: SettingsChange) -> Self {
        let mut changed = self.clone();
        match change {
            SettingsChange::ExportOptions(export) => changed.export = export,
            SettingsChange::ShowTips => changed.show_tips = true,
            SettingsChange::HideTips => changed.show_tips = false,
        }
        changed
    }

    /// The detection settings of videos whose folder has none of its own.
    pub fn general_detection(&self) -> DetectionSettings {
        self.detection
    }

    /// The detection settings a video is scanned with: those remembered for
    /// its folder, else the general ones.
    pub fn detection_for(&self, video: &Path) -> DetectionInUse {
        match folder_settings::folder_of(video) {
            Some(folder) => self.detection_for_folder(&folder),
            // A path that names no folder has no folder to have settings.
            None => self.general_detection_in_use(),
        }
    }

    /// The detection settings of the videos directly inside `folder`.
    pub fn detection_for_folder(&self, folder: &Path) -> DetectionInUse {
        match self.folders.for_folder(folder) {
            Some((remembered_folder, detection)) => DetectionInUse {
                detection: *detection,
                scope: SettingsScope::Folder(remembered_folder.to_path_buf()),
            },
            None => self.general_detection_in_use(),
        }
    }

    fn general_detection_in_use(&self) -> DetectionInUse {
        DetectionInUse {
            detection: self.general_detection(),
            scope: SettingsScope::General,
        }
    }

    /// Keeps the detection settings in force for `video` now as the settings
    /// of its folder, whatever the general ones become later.
    pub fn remember_detection_for_folder_of(&mut self, video: &Path) {
        let Some(folder) = folder_settings::folder_of(video) else {
            return;
        };
        let in_force = self.detection_for_folder(&folder).detection;
        self.folders.remember(&folder, in_force);
    }

    /// Forgets the settings of the folder `video` is in, so the general
    /// ones apply to it again.
    pub fn use_general_detection_for_folder_of(&mut self, video: &Path) {
        if let Some(folder) = folder_settings::folder_of(video) {
            self.folders.forget(&folder);
        }
    }

    /// Changes the detection settings `video` is scanned with: those of its
    /// folder when it has its own, and the general ones otherwise.
    pub fn change_detection_for(&mut self, video: &Path, detection: DetectionSettings) {
        match self.detection_for(video).scope {
            SettingsScope::Folder(folder) => self.folders.remember(&folder, detection),
            SettingsScope::General => self.detection = detection,
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path().context("this system has no folder for settings")?;
        self.save_to(&path)
    }

    /// Fails for values no scan could work with, whether they came from a
    /// hand-edited file or from the command line.
    pub fn validate(&self) -> Result<()> {
        if let Err(problem) = self.general_detection().validate() {
            bail!("{problem}");
        }
        for (folder, detection) in self.folders.iter() {
            if let Err(problem) = detection.validate() {
                bail!("in the settings for {}, {problem}", folder.display());
            }
        }
        Ok(())
    }

    /// Returns `None` when no settings were ever saved.
    fn load_from(path: &Path) -> Result<Option<Self>> {
        let json = match std::fs::read_to_string(path) {
            Ok(json) => json,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("the file could not be read"),
        };
        let saved: serde_json::Value =
            serde_json::from_str(&json).context("the file is not valid")?;
        let written_by = if saved.get(SCENE_NAMES_USED_ENTRY).is_some() {
            WrittenBy::AVersionThatSaysTheCount
        } else {
            WrittenBy::AVersionBeforeTheCount
        };
        let settings: Self = serde_json::from_value(saved).context("the file is not valid")?;
        settings.validate()?;
        Ok(Some(settings.with_the_default_name_of_today(written_by)))
    }

    /// The settings, with the default name of today where the file only
    /// holds the default name of its day.
    ///
    /// Settings are saved whole, so a file from before the default name
    /// said the count holds the old default name whether or not the person
    /// ever looked at it. Left as it is, they would never get the new
    /// names. A file this version wrote is taken at its word: someone who
    /// types the old default name now keeps it.
    fn with_the_default_name_of_today(mut self, written_by: WrittenBy) -> Self {
        let holds_the_old_default =
            self.export.scene_name == SceneNamePattern::default_before_the_count();
        if written_by == WrittenBy::AVersionBeforeTheCount && holds_the_old_default {
            self.export.scene_name = SceneNamePattern::default();
        }
        self
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)
                .with_context(|| format!("could not create {}", folder.display()))?;
        }
        let json = serde_json::to_string_pretty(self).context("could not encode the settings")?;

        // Written under a temporary name first, so a crash mid-write cannot
        // leave half a file where the settings were.
        let unfinished_path = path.with_extension("json.part");
        std::fs::write(&unfinished_path, json)
            .with_context(|| format!("could not write {}", unfinished_path.display()))?;
        std::fs::rename(&unfinished_path, path)
            .with_context(|| format!("could not write {}", path.display()))
    }
}

/// Notes in the saved settings that a split was written with `pattern`,
/// for a run that does not keep the settings open, as the command line is.
/// Only this one entry of the file changes, whatever the run was told to do
/// differently from the file.
///
/// A settings file that cannot be read is left as it is, and that is the
/// error: writing a new one over it would lose what it says about originals.
pub fn remember_scene_name_used_for_a_split(pattern: SceneNamePattern) -> Result<()> {
    let path = settings_path().context("this system has no folder for settings")?;
    remember_scene_name_in(&path, pattern)
}

fn remember_scene_name_in(path: &Path, pattern: SceneNamePattern) -> Result<()> {
    let mut saved = Settings::load_from(path)?.unwrap_or_default();
    if saved.scene_names_used.newest() == Some(pattern) {
        return Ok(());
    }
    saved.scene_names_used.remember(pattern);
    saved.save_to(path)
}

/// Where the settings are kept, in the system's own folder for settings.
/// `None` on a system that has no such folder.
pub fn settings_path() -> Option<PathBuf> {
    Some(folders::config_dir()?.join(APP_FOLDER).join(SETTINGS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{ScanMode, ScanSettings};
    use crate::exact_cut::CutPrecision;
    use crate::hardcut::{HardCutSettings, Sensitivity};
    use crate::split::SceneLocation;

    /// A settings file of its own for one test, removed when the test ends.
    struct SettingsFile(PathBuf);

    impl SettingsFile {
        fn for_test(name: &str) -> Self {
            let folder = std::env::temp_dir().join(format!(
                "scene_splitter_settings_test_{}_{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&folder);
            Self(folder.join(SETTINGS_FILE))
        }
    }

    impl Drop for SettingsFile {
        fn drop(&mut self) {
            if let Some(folder) = self.0.parent() {
                let _ = std::fs::remove_dir_all(folder);
            }
        }
    }

    fn changed_settings() -> Settings {
        let mut settings = Settings::default();
        settings.detection.scan.mode = ScanMode::EveryFrame;
        settings.detection.scan.criteria.min_seconds = 0.5;
        settings.export.scene_location = SceneLocation::NewFolder;
        settings.export.original_policy = OriginalPolicy::Keep;
        settings.export.scene_name = SceneNamePattern::new("{name} part {number}").unwrap();
        settings.detection.hard_cuts.sensitivity = Sensitivity::High;
        settings.detection.hard_cuts.min_scene_seconds = 300.0;
        settings.show_tips = false;
        settings.recycling_question = RecyclingQuestion::Answered;
        settings.theme = ThemeChoice::Dark;
        settings
            .scene_names_used
            .remember(SceneNamePattern::new("{number} {name}").unwrap());
        settings.remember_scene_name_in_use();
        settings
    }

    fn write_settings_file(file: &SettingsFile, json: &str) {
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(&file.0, json).unwrap();
    }

    #[test]
    fn the_names_splits_were_written_with_are_remembered_newest_first() {
        let used: Vec<String> = changed_settings()
            .scene_names_used
            .iter()
            .map(|pattern| pattern.to_string())
            .collect();
        assert_eq!(used, ["{name} part {number}", "{number} {name}"]);
        assert_eq!(
            changed_settings().scene_names(),
            SceneNames::new(
                SceneNamePattern::new("{name} part {number}").unwrap(),
                changed_settings().scene_names_used
            )
        );
    }

    #[test]
    fn restoring_the_defaults_keeps_the_names_earlier_splits_were_written_with() {
        let restored = changed_settings().restored_to_defaults();
        assert_eq!(
            restored.scene_names_used,
            changed_settings().scene_names_used
        );
    }

    #[test]
    fn a_file_from_before_names_were_remembered_loads_with_none() {
        let file = SettingsFile::for_test("before_names_used");
        write_settings_file(&file, SETTINGS_OF_AN_EARLIER_VERSION);
        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.scene_names_used, PatternsUsed::default());
    }

    #[test]
    fn the_old_default_name_in_a_file_from_before_becomes_the_default_of_today() {
        let file = SettingsFile::for_test("old_default_name");
        // Settings are saved whole, so the old default stands in every file
        // an earlier version saved.
        write_settings_file(
            &file,
            r#"{ "export": { "scene_name": "{name}_scene_{number}", "original_policy": "Keep" } }"#,
        );
        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.export.scene_name, SceneNamePattern::default());
        assert_eq!(
            loaded.export.scene_name.as_str(),
            "{name}_scene_{number}_of_{count}"
        );
        assert_eq!(loaded.export.original_policy, OriginalPolicy::Keep);
    }

    #[test]
    fn a_name_the_person_chose_before_is_kept() {
        let file = SettingsFile::for_test("chosen_name_before");
        write_settings_file(
            &file,
            r#"{ "export": { "scene_name": "{name} part {number}" } }"#,
        );
        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.export.scene_name.as_str(), "{name} part {number}");
    }

    #[test]
    fn the_old_default_name_typed_into_this_version_is_kept() {
        let file = SettingsFile::for_test("old_default_chosen_now");
        let mut settings = Settings::default();
        settings.export.scene_name = SceneNamePattern::default_before_the_count();
        settings.save_to(&file.0).unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded, settings);
        assert_eq!(loaded.export.scene_name.as_str(), "{name}_scene_{number}");
    }

    #[test]
    fn a_remembered_name_that_would_leave_the_folder_is_an_error() {
        let file = SettingsFile::for_test("remembered_name_with_folder");
        write_settings_file(&file, r#"{ "scene_names_used": ["../{name}_{number}"] }"#);
        assert!(Settings::load_from(&file.0).is_err());
    }

    #[test]
    fn a_run_without_the_window_adds_its_name_to_the_saved_settings_and_nothing_else() {
        let file = SettingsFile::for_test("remember_from_command_line");
        changed_settings().save_to(&file.0).unwrap();
        let for_this_run = SceneNamePattern::new("{name} take {number}").unwrap();

        remember_scene_name_in(&file.0, for_this_run).unwrap();
        let mut expected = changed_settings();
        expected.scene_names_used.remember(for_this_run);
        assert_eq!(
            Settings::load_from(&file.0).unwrap(),
            Some(expected.clone())
        );
        // The name in use is the saved one still, not that of the run.
        assert_eq!(expected.export.scene_name.as_str(), "{name} part {number}");

        // Said again, nothing changes.
        remember_scene_name_in(&file.0, for_this_run).unwrap();
        assert_eq!(Settings::load_from(&file.0).unwrap(), Some(expected));
    }

    #[test]
    fn a_name_is_remembered_even_when_no_settings_were_ever_saved() {
        let file = SettingsFile::for_test("remember_without_a_file");
        remember_scene_name_in(&file.0, SceneNamePattern::default()).unwrap();
        let mut expected = Settings::default();
        expected.remember_scene_name_in_use();
        assert_eq!(Settings::load_from(&file.0).unwrap(), Some(expected));
    }

    #[test]
    fn a_damaged_settings_file_is_not_written_over_to_remember_a_name() {
        let file = SettingsFile::for_test("remember_in_damaged_file");
        write_settings_file(&file, "{ not json");
        assert!(remember_scene_name_in(&file.0, SceneNamePattern::default()).is_err());
        assert_eq!(std::fs::read_to_string(&file.0).unwrap(), "{ not json");
    }

    #[test]
    fn saved_settings_come_back_unchanged() {
        let file = SettingsFile::for_test("round_trip");
        changed_settings().save_to(&file.0).unwrap();
        assert_eq!(
            Settings::load_from(&file.0).unwrap(),
            Some(changed_settings())
        );
    }

    #[test]
    fn nothing_saved_yet_is_not_an_error() {
        let file = SettingsFile::for_test("never_saved");
        assert_eq!(Settings::load_from(&file.0).unwrap(), None);
    }

    #[test]
    fn a_file_missing_a_section_fills_it_with_defaults() {
        let file = SettingsFile::for_test("older_version");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(
            &file.0,
            r#"{ "export": { "scene_location": "NewFolder", "original_policy": "Keep" } }"#,
        )
        .unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.export.scene_location, SceneLocation::NewFolder);
        assert_eq!(loaded.detection.scan, ScanSettings::default());
        assert_eq!(loaded.detection.hard_cuts, HardCutSettings::default());
        assert!(loaded.show_tips);
    }

    #[test]
    fn a_file_from_before_the_dark_theme_follows_the_system() {
        let file = SettingsFile::for_test("before_dark_theme");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(&file.0, SETTINGS_OF_AN_EARLIER_VERSION).unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.theme, ThemeChoice::FollowSystem);
        assert!(!loaded.show_tips);
    }

    #[test]
    fn every_theme_choice_is_saved_and_comes_back() {
        let file = SettingsFile::for_test("theme_choice");
        for theme in ThemeChoice::ALL {
            let settings = Settings {
                theme,
                ..Settings::default()
            };
            settings.save_to(&file.0).unwrap();
            assert_eq!(Settings::load_from(&file.0).unwrap(), Some(settings));
        }
    }

    #[test]
    fn a_theme_the_app_does_not_know_is_an_error() {
        let file = SettingsFile::for_test("unknown_theme");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(&file.0, r#"{ "theme": "Sepia" }"#).unwrap();
        assert!(Settings::load_from(&file.0).is_err());
    }

    #[test]
    fn restoring_the_defaults_follows_the_system_again() {
        let restored = changed_settings().restored_to_defaults();
        assert_eq!(restored.theme, ThemeChoice::FollowSystem);
    }

    #[test]
    fn a_file_from_before_the_recycling_question_loads_as_not_asked_yet() {
        let file = SettingsFile::for_test("before_recycling_question");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(
            &file.0,
            r#"{ "export": { "original_policy": "RecycleOnceVerified" }, "show_tips": false }"#,
        )
        .unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.recycling_question, RecyclingQuestion::NotAskedYet);
        assert!(loaded.must_ask_before_recycling());
        assert!(!loaded.show_tips);
    }

    #[test]
    fn a_file_from_before_scene_names_could_be_chosen_gets_the_default_name() {
        let file = SettingsFile::for_test("before_scene_names");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(
            &file.0,
            r#"{ "export": { "scene_location": "NewFolder", "original_policy": "Keep" } }"#,
        )
        .unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.export.scene_name, SceneNamePattern::default());
        assert_eq!(
            loaded.export.scene_name.as_str(),
            "{name}_scene_{number}_of_{count}"
        );
    }

    #[test]
    fn a_saved_scene_name_that_would_leave_the_folder_is_an_error() {
        let file = SettingsFile::for_test("scene_name_with_folder");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(
            &file.0,
            r#"{ "export": { "scene_name": "../{name}_{number}" } }"#,
        )
        .unwrap();
        assert!(Settings::load_from(&file.0).is_err());
    }

    #[test]
    fn restoring_the_defaults_brings_back_the_usual_scene_names() {
        let restored = changed_settings().restored_to_defaults();
        assert_eq!(restored.export.scene_name, SceneNamePattern::default());
    }

    #[test]
    fn the_recycling_question_starts_unanswered() {
        assert_eq!(
            Settings::default().recycling_question,
            RecyclingQuestion::NotAskedYet
        );
        assert!(Settings::default().must_ask_before_recycling());
    }

    #[test]
    fn the_recycling_question_is_asked_only_once() {
        let settings = Settings {
            recycling_question: RecyclingQuestion::Answered,
            ..Settings::default()
        };
        assert!(!settings.must_ask_before_recycling());
    }

    #[test]
    fn nothing_is_asked_when_the_original_is_kept_anyway() {
        let mut settings = Settings::default();
        settings.export.original_policy = OriginalPolicy::Keep;
        assert!(!settings.must_ask_before_recycling());
    }

    #[test]
    fn nothing_is_asked_before_an_exact_split_since_it_keeps_the_original() {
        let mut settings = Settings::default();
        settings.export.cut_precision = CutPrecision::ExactFrame;
        assert_eq!(
            settings.export.original_policy,
            OriginalPolicy::RecycleOnceVerified
        );
        assert!(!settings.must_ask_before_recycling());
    }

    #[test]
    fn a_file_from_before_exact_cuts_loads_and_cuts_on_keyframes() {
        let file = SettingsFile::for_test("before_exact_cuts");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(
            &file.0,
            r#"{ "export": { "scene_location": "NewFolder", "original_policy": "Keep" } }"#,
        )
        .unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.export.cut_precision, CutPrecision::NearestKeyframe);
        assert_eq!(loaded.export.original_policy, OriginalPolicy::Keep);
    }

    #[test]
    fn the_choice_of_exact_cuts_is_saved_and_loaded() {
        let file = SettingsFile::for_test("exact_cuts");
        let mut settings = Settings::default();
        settings.export.cut_precision = CutPrecision::ExactFrame;
        settings.save_to(&file.0).unwrap();
        assert_eq!(Settings::load_from(&file.0).unwrap(), Some(settings));
    }

    #[test]
    fn restoring_the_defaults_does_not_ask_the_recycling_question_again() {
        let restored = changed_settings().restored_to_defaults();
        assert_eq!(restored.recycling_question, RecyclingQuestion::Answered);
        assert!(!restored.must_ask_before_recycling());
    }

    #[test]
    fn restoring_the_defaults_keeps_what_the_person_chose_for_originals() {
        // They answered "Keep the original", then changed other settings.
        let chosen = changed_settings();
        assert_eq!(chosen.export.original_policy, OriginalPolicy::Keep);

        let restored = chosen.restored_to_defaults();
        assert_eq!(restored.export.original_policy, OriginalPolicy::Keep);
        // Everything else is as it was at first.
        let mut expected = Settings::default();
        expected.export.original_policy = OriginalPolicy::Keep;
        expected.recycling_question = RecyclingQuestion::Answered;
        expected.scene_names_used = chosen.scene_names_used;
        assert_eq!(restored, expected);
    }

    #[test]
    fn restoring_the_defaults_never_turns_recycling_on_unasked() {
        for recycling_question in [RecyclingQuestion::NotAskedYet, RecyclingQuestion::Answered] {
            let mut keeping = Settings {
                recycling_question,
                ..Settings::default()
            };
            keeping.export.original_policy = OriginalPolicy::Keep;
            assert_eq!(
                keeping.restored_to_defaults().export.original_policy,
                OriginalPolicy::Keep
            );
        }
    }

    #[test]
    fn a_change_made_on_a_screen_touches_only_what_it_names() {
        let before = Settings::default();

        let export = ExportOptions {
            scene_location: SceneLocation::NewFolder,
            original_policy: OriginalPolicy::Keep,
            scene_name: SceneNamePattern::new("{number} {name}").unwrap(),
            ..ExportOptions::default()
        };
        let changed = before.changed_by(SettingsChange::ExportOptions(export));
        assert_eq!(changed, Settings { export, ..before });
    }

    #[test]
    fn the_tips_can_be_hidden_and_shown_again() {
        let hidden = Settings::default().changed_by(SettingsChange::HideTips);
        assert!(!hidden.show_tips);
        assert!(hidden.changed_by(SettingsChange::ShowTips).show_tips);
    }

    #[test]
    fn a_file_missing_one_field_of_a_section_keeps_the_rest_of_that_section() {
        let file = SettingsFile::for_test("missing_field");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(&file.0, r#"{ "export": { "original_policy": "Keep" } }"#).unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert_eq!(loaded.export.original_policy, OriginalPolicy::Keep);
        assert_eq!(loaded.export.scene_location, SceneLocation::BesideVideo);
    }

    #[test]
    fn when_the_saved_choice_cannot_be_read_originals_are_kept() {
        let fallback = Settings::defaults_that_keep_originals();
        assert_eq!(fallback.export.original_policy, OriginalPolicy::Keep);
        assert_eq!(fallback.detection.scan, ScanSettings::default());
    }

    #[test]
    fn a_damaged_file_is_an_error() {
        let file = SettingsFile::for_test("damaged");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(&file.0, "{ not json").unwrap();
        assert!(Settings::load_from(&file.0).is_err());
    }

    #[test]
    fn impossible_values_are_refused() {
        let mut settings = Settings::default();
        settings.detection.scan.criteria.min_seconds = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = Settings::default();
        settings.detection.scan.criteria.min_dark_fraction = 1.5;
        assert!(settings.validate().is_err());

        let mut settings = Settings::default();
        settings.detection.hard_cuts.min_scene_seconds = -1.0;
        assert!(settings.validate().is_err());
    }

    /// A video in a folder of films, and one in a folder of old tapes.
    fn a_film() -> PathBuf {
        std::env::temp_dir().join("films").join("film.mp4")
    }

    fn a_tape() -> PathBuf {
        std::env::temp_dir().join("tapes").join("tape.mp4")
    }

    fn tapes_folder() -> PathBuf {
        std::env::temp_dir().join("tapes")
    }

    /// Detection settings that suit grainy tapes and differ from the
    /// defaults in both of their parts.
    fn for_grainy_tapes() -> DetectionSettings {
        let mut detection = DetectionSettings::default();
        detection.scan.mode = ScanMode::EveryFrame;
        detection.scan.criteria.max_dark_luma = 48;
        detection.hard_cuts.sensitivity = Sensitivity::High;
        detection
    }

    fn settings_with_tapes_remembered() -> Settings {
        let mut settings = Settings::default();
        settings
            .folders
            .remember(&tapes_folder(), for_grainy_tapes());
        settings
    }

    #[test]
    fn a_video_is_scanned_with_the_general_settings_unless_its_folder_has_its_own() {
        let settings = settings_with_tapes_remembered();

        assert_eq!(
            settings.detection_for(&a_film()),
            DetectionInUse {
                detection: DetectionSettings::default(),
                scope: SettingsScope::General,
            }
        );
        assert_eq!(
            settings.detection_for(&a_tape()),
            DetectionInUse {
                detection: for_grainy_tapes(),
                scope: SettingsScope::Folder(tapes_folder()),
            }
        );
    }

    #[test]
    fn a_video_in_a_subfolder_is_scanned_with_the_general_settings() {
        let settings = settings_with_tapes_remembered();
        let in_a_subfolder = tapes_folder().join("1994").join("tape.mp4");
        assert_eq!(
            settings.detection_for(&in_a_subfolder).scope,
            SettingsScope::General
        );
    }

    #[test]
    fn remembering_for_a_folder_keeps_the_settings_in_force_now() {
        let mut settings = Settings::default();
        settings.detection.scan.criteria.min_seconds = 0.5;
        settings.remember_detection_for_folder_of(&a_tape());
        let remembered = settings.detection_for(&a_tape());
        assert_eq!(remembered.scope, SettingsScope::Folder(tapes_folder()));
        assert_eq!(remembered.detection.scan.criteria.min_seconds, 0.5);

        // The general settings move on, the folder's stay.
        settings.detection.scan.criteria.min_seconds = 2.0;
        assert_eq!(
            settings
                .detection_for(&a_tape())
                .detection
                .scan
                .criteria
                .min_seconds,
            0.5
        );
        assert_eq!(
            settings
                .detection_for(&a_film())
                .detection
                .scan
                .criteria
                .min_seconds,
            2.0
        );
    }

    #[test]
    fn remembering_again_does_not_replace_a_folders_settings_with_the_general_ones() {
        let mut settings = settings_with_tapes_remembered();
        settings.remember_detection_for_folder_of(&a_tape());
        assert_eq!(
            settings.detection_for(&a_tape()).detection,
            for_grainy_tapes()
        );
    }

    #[test]
    fn using_the_general_settings_for_a_folder_forgets_its_own() {
        let mut settings = settings_with_tapes_remembered();
        settings.use_general_detection_for_folder_of(&a_tape());

        assert_eq!(
            settings.detection_for(&a_tape()).scope,
            SettingsScope::General
        );
        assert!(settings.folders.is_empty());
    }

    #[test]
    fn a_change_of_detection_goes_to_the_settings_the_video_is_scanned_with() {
        let mut settings = settings_with_tapes_remembered();
        let mut changed = for_grainy_tapes();
        changed.hard_cuts.min_scene_seconds = 90.0;

        // The tape's folder has its own settings, so those change.
        settings.change_detection_for(&a_tape(), changed);
        assert_eq!(settings.detection_for(&a_tape()).detection, changed);
        assert_eq!(settings.general_detection(), DetectionSettings::default());

        // The film's folder has none, so the general ones change.
        settings.change_detection_for(&a_film(), changed);
        assert_eq!(settings.general_detection(), changed);
        assert_eq!(settings.detection.scan, changed.scan);
        assert_eq!(settings.detection.hard_cuts, changed.hard_cuts);
    }

    #[test]
    fn folder_settings_are_saved_and_come_back() {
        let file = SettingsFile::for_test("folders_round_trip");
        let settings = settings_with_tapes_remembered();
        settings.save_to(&file.0).unwrap();
        assert_eq!(Settings::load_from(&file.0).unwrap(), Some(settings));
    }

    /// Every part the version before scene names, exact cuts and folder
    /// settings wrote, with none of its values at their default.
    const SETTINGS_OF_AN_EARLIER_VERSION: &str = r#"{
        "scan": {"mode": "EveryFrame", "criteria":
            {"max_dark_luma": 40, "min_dark_fraction": 0.9, "min_seconds": 0.5}},
        "export": {"scene_location": "NewFolder", "original_policy": "Keep"},
        "hard_cuts": {"sensitivity": "High", "min_scene_seconds": 300.0},
        "show_tips": false,
        "recycling_question": "Answered"
    }"#;

    #[test]
    fn a_whole_file_of_an_earlier_version_loads_and_behaves_as_it_did() {
        let file = SettingsFile::for_test("whole_earlier_file");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(&file.0, SETTINGS_OF_AN_EARLIER_VERSION).unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();

        // What the file says is kept.
        assert_eq!(loaded.detection.scan.mode, ScanMode::EveryFrame);
        assert_eq!(loaded.detection.hard_cuts.min_scene_seconds, 300.0);
        assert_eq!(loaded.export.scene_location, SceneLocation::NewFolder);
        assert_eq!(loaded.export.original_policy, OriginalPolicy::Keep);
        assert!(!loaded.show_tips);
        assert_eq!(loaded.recycling_question, RecyclingQuestion::Answered);
        // What it could not say yet is as the app does it by default: the
        // default names, cuts on keyframes, and no folder of its own.
        assert_eq!(loaded.export.scene_name, SceneNamePattern::default());
        assert_eq!(loaded.export.cut_precision, CutPrecision::NearestKeyframe);
        assert!(loaded.folders.is_empty());

        // Saved by this version, it comes back the same.
        loaded.save_to(&file.0).unwrap();
        assert_eq!(Settings::load_from(&file.0).unwrap(), Some(loaded));
    }

    #[test]
    fn a_folder_remembers_how_to_find_scenes_and_nothing_about_writing_them() {
        let mut settings = Settings::default();
        settings.export.cut_precision = CutPrecision::ExactFrame;
        settings.export.scene_name = SceneNamePattern::new("{number} {name}").unwrap();
        settings.remember_detection_for_folder_of(&a_tape());

        let saved = serde_json::to_value(&settings.folders).unwrap().to_string();
        assert!(saved.contains("hard_cuts"), "{saved}");
        for written_per_split in ["cut_precision", "scene_name", "original_policy"] {
            assert!(!saved.contains(written_per_split), "{saved}");
        }
    }

    #[test]
    fn a_file_from_before_folder_settings_loads_with_none() {
        let file = SettingsFile::for_test("before_folders");
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        // Every part an earlier version wrote, and nothing about folders.
        std::fs::write(
            &file.0,
            r#"{
                "scan": {"mode": "EveryFrame", "criteria":
                    {"max_dark_luma": 40, "min_dark_fraction": 0.9, "min_seconds": 0.5}},
                "export": {"scene_location": "NewFolder", "original_policy": "Keep"},
                "hard_cuts": {"sensitivity": "High", "min_scene_seconds": 300.0},
                "show_tips": false,
                "recycling_question": "Answered"
            }"#,
        )
        .unwrap();

        let loaded = Settings::load_from(&file.0).unwrap().unwrap();
        assert!(loaded.folders.is_empty());
        assert_eq!(loaded.detection.scan.mode, ScanMode::EveryFrame);
        assert_eq!(loaded.detection.scan.criteria.max_dark_luma, 40);
        assert_eq!(loaded.detection.hard_cuts.min_scene_seconds, 300.0);
        assert_eq!(
            loaded.detection_for(&a_tape()).scope,
            SettingsScope::General
        );
    }

    #[test]
    fn impossible_values_remembered_for_a_folder_are_refused_with_the_folder_named() {
        let mut impossible = DetectionSettings::default();
        impossible.scan.criteria.min_seconds = 0.0;
        let mut settings = Settings::default();
        settings.folders.remember(&tapes_folder(), impossible);

        let problem = settings.validate().unwrap_err().to_string();
        assert!(problem.contains("tapes"), "{problem}");
    }

    #[test]
    fn restoring_the_defaults_forgets_what_was_remembered_for_folders() {
        let restored = settings_with_tapes_remembered().restored_to_defaults();
        assert!(restored.folders.is_empty());
    }

    #[test]
    fn the_defaults_are_valid() {
        assert!(Settings::default().validate().is_ok());
    }

    #[test]
    fn the_general_detection_settings_are_saved_where_they_always_were() {
        // One value in the program, two entries of their own in the file,
        // which is the layout every earlier version wrote and reads.
        let saved = serde_json::to_value(Settings::default()).unwrap();
        assert!(saved.get("scan").is_some(), "{saved}");
        assert!(saved.get("hard_cuts").is_some(), "{saved}");
        assert!(saved.get("detection").is_none(), "{saved}");
    }
}
