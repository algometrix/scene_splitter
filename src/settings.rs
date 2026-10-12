//! The choices a person makes once and expects the app to remember: how
//! black screens are detected, how hard cuts are thinned, and where scenes
//! go. The window and the command line read the same file.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::detect::ScanSettings;
use crate::export::ExportOptions;
use crate::folders;
use crate::hardcut::HardCutSettings;

const APP_FOLDER: &str = "scene_splitter";
const SETTINGS_FILE: &str = "settings.json";

/// A missing part of the file falls back to its default, so a file written
/// by an older version keeps working.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub scan: ScanSettings,
    pub export: ExportOptions,
    pub hard_cuts: HardCutSettings,
}

impl Settings {
    /// The saved settings, or the defaults when nothing usable is saved.
    /// Settings are a convenience, so a damaged file is reported and
    /// ignored instead of stopping the app.
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        match Self::load_from(&path) {
            Ok(Some(settings)) => settings,
            Ok(None) => Self::default(),
            Err(error) => {
                eprintln!("Ignoring the settings in {}: {error:#}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path().context("this system has no folder for settings")?;
        self.save_to(&path)
    }

    /// Fails for values no scan could work with, whether they came from a
    /// hand-edited file or from the command line.
    pub fn validate(&self) -> Result<()> {
        if let Err(problem) = self.scan.criteria.validate() {
            bail!("{problem}");
        }
        if !self.hard_cuts.min_scene_seconds.is_finite() || self.hard_cuts.min_scene_seconds <= 0.0
        {
            bail!("the shortest scene for hard cuts must be longer than zero");
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
        let settings: Self = serde_json::from_str(&json).context("the file is not valid")?;
        settings.validate()?;
        Ok(Some(settings))
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

/// Where the settings are kept, in the system's own folder for settings.
/// `None` on a system that has no such folder.
pub fn settings_path() -> Option<PathBuf> {
    Some(folders::config_dir()?.join(APP_FOLDER).join(SETTINGS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::ScanMode;
    use crate::export::OriginalPolicy;
    use crate::hardcut::Sensitivity;
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
        settings.scan.mode = ScanMode::EveryFrame;
        settings.scan.criteria.min_seconds = 0.5;
        settings.export.scene_location = SceneLocation::NewFolder;
        settings.export.original_policy = OriginalPolicy::Keep;
        settings.hard_cuts.sensitivity = Sensitivity::High;
        settings.hard_cuts.min_scene_seconds = 300.0;
        settings
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
        assert_eq!(loaded.scan, ScanSettings::default());
        assert_eq!(loaded.hard_cuts, HardCutSettings::default());
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
        settings.scan.criteria.min_seconds = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = Settings::default();
        settings.scan.criteria.min_dark_fraction = 1.5;
        assert!(settings.validate().is_err());

        let mut settings = Settings::default();
        settings.hard_cuts.min_scene_seconds = -1.0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn the_defaults_are_valid() {
        assert!(Settings::default().validate().is_ok());
    }
}
