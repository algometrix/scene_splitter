//! Where the app keeps its files on each operating system, following each
//! system's own convention.

use std::path::PathBuf;

const MACOS_APPLICATION_SUPPORT: &str = "Library/Application Support";

/// The per-user folder for settings, or `None` on a system that names no
/// home folder at all.
///
/// Windows: `%APPDATA%`. macOS: `~/Library/Application Support`.
/// Elsewhere: `$XDG_CONFIG_HOME`, or `~/.config`.
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return folder_named_by("APPDATA");
    }
    if cfg!(target_os = "macos") {
        return Some(home_dir()?.join(MACOS_APPLICATION_SUPPORT));
    }
    folder_named_by("XDG_CONFIG_HOME").or_else(|| Some(home_dir()?.join(".config")))
}

/// The per-user folder for files the app downloads, or `None` on a system
/// that names no home folder. Programs are run from here, so a folder shared
/// with other users is never used as a fallback.
///
/// Windows: `%LOCALAPPDATA%`. macOS: `~/Library/Application Support`.
/// Elsewhere: `$XDG_DATA_HOME`, or `~/.local/share`.
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        folder_named_by("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        home_dir().map(|home| home.join(MACOS_APPLICATION_SUPPORT))
    } else {
        folder_named_by("XDG_DATA_HOME")
            .or_else(|| home_dir().map(|home| home.join(".local").join("share")))
    }
}

fn home_dir() -> Option<PathBuf> {
    folder_named_by("HOME")
}

/// The folder an environment variable points at. An empty value counts as
/// not set, which is how the XDG convention reads it.
fn folder_named_by(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_folder_is_never_the_shared_temporary_folder() {
        if let Some(data) = data_dir() {
            assert_ne!(data, std::env::temp_dir());
        }
    }

    #[test]
    fn an_unset_variable_names_no_folder() {
        assert_eq!(
            folder_named_by("SCENE_SPLITTER_NO_SUCH_VARIABLE_FOR_TESTS"),
            None
        );
    }
}
