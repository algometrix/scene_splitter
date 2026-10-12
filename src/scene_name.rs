//! How scene files are named: a pattern the person chooses, such as
//! "{name}_scene_{number}", and the names it gives the scenes of one video.
//!
//! The same naming answers two questions, so they cannot drift apart: what
//! a split calls its files, and which files in a folder an earlier split of
//! the same video left there.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::folder_settings::{PathCase, SYSTEM_PATH_CASE};
use crate::timecode::TimeSpan;

/// The pattern that gives the names this app has always written:
/// "<video name>_scene_001" and so on.
const DEFAULT_PATTERN: &str = "{name}_scene_{number}";
/// What stands between the video's name and the scene number in a name the
/// default pattern gives.
const DEFAULT_NAME_MARKER: &str = "_scene_";
/// Scenes are numbered from one.
pub const FIRST_SCENE_NUMBER: usize = 1;
/// Long enough for any sensible name, and short enough to be held in the
/// settings without an allocation, which keeps the settings cheap to copy
/// and compare in every frame.
pub const MAX_PATTERN_BYTES: usize = 100;
/// What most file systems allow for one file name.
const MAX_FILE_NAME_BYTES: usize = 255;
/// Scene numbers have at least this many digits, as they always had. A
/// split into a thousand scenes or more uses as many digits as its count,
/// so that the files still sort in scene order.
const MIN_NUMBER_DIGITS: usize = 3;
/// Not allowed in file names on Windows. The two slashes are named apart,
/// because they would put the scenes into another folder.
const CHARACTERS_WINDOWS_REFUSES: [char; 7] = ['<', '>', ':', '"', '|', '?', '*'];
const PATH_SEPARATORS: [char; 2] = ['/', '\\'];
/// Windows keeps these names for devices, whatever follows the first dot.
const NAMES_WINDOWS_RESERVES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
const SECONDS_PER_HOUR: u64 = 3600;
const SECONDS_PER_MINUTE: u64 = 60;

/// The values the example beside the pattern box is made of.
const EXAMPLE_VIDEO_NAME: &str = "My video";
const EXAMPLE_EXTENSION: &str = "mp4";
const EXAMPLE_SCENE_NUMBER: usize = 2;
const EXAMPLE_SCENE_COUNT: usize = 12;
const EXAMPLE_SCENE: TimeSpan = TimeSpan {
    start: 192.0,
    end: 437.0,
};

/// One thing a pattern can stand for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placeholder {
    /// The video's name without its extension.
    Name,
    /// The scene's number, counted from one, with leading zeros.
    Number,
    /// How many scene files the split writes.
    Count,
    /// Where the scene starts in the video, as "00h03m12s".
    Start,
    /// How long the scene is, as "00h04m05s".
    Length,
}

impl Placeholder {
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::Number,
        Self::Count,
        Self::Start,
        Self::Length,
    ];

    /// The word between the braces.
    fn word(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Number => "number",
            Self::Count => "count",
            Self::Start => "start",
            Self::Length => "length",
        }
    }

    /// The placeholder as it is typed: "{name}".
    pub fn typed(self) -> String {
        format!("{{{}}}", self.word())
    }

    /// What it stands for, in a few words, for the settings to say.
    pub fn meaning(self) -> &'static str {
        match self {
            Self::Name => "the video's name",
            Self::Number => "the scene's number",
            Self::Count => "how many scenes are written",
            Self::Start => "where the scene starts",
            Self::Length => "how long the scene is",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|placeholder| placeholder.word() == word)
    }
}

/// Why a pattern cannot be used. Each says what to change.
#[derive(Debug, Clone, PartialEq)]
pub enum PatternError {
    Empty,
    TooLong,
    /// Without it, every scene would get the same name.
    MissingNumber,
    /// Without it, the scenes of two videos in one folder would replace
    /// each other, and nobody could tell whose scene a file is.
    MissingName,
    UnknownPlaceholder(String),
    /// A brace that does not open or close a placeholder.
    StrayBrace,
    PathSeparator,
    CharacterNotAllowed(char),
    ReservedName(String),
    /// Windows drops a space or dot at the start or end of a name, or
    /// refuses the file.
    SpaceAtAnEnd,
}

impl fmt::Display for PatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "Type a name, such as {DEFAULT_PATTERN}."),
            Self::TooLong => write!(
                formatter,
                "The name is too long. It can have {MAX_PATTERN_BYTES} plain letters, or fewer with accents."
            ),
            Self::MissingNumber => write!(
                formatter,
                "Add {{number}}, or every scene would get the same name."
            ),
            Self::MissingName => write!(
                formatter,
                "Add {{name}}, or the scenes of two videos would replace each other."
            ),
            Self::UnknownPlaceholder(word) => write!(
                formatter,
                "{{{word}}} is not known. Use {{name}}, {{number}}, {{count}}, {{start}} or {{length}}."
            ),
            Self::StrayBrace => write!(
                formatter,
                "A brace is only for placeholders such as {{number}}."
            ),
            Self::PathSeparator => write!(
                formatter,
                "A file name cannot contain / or \\. The folder is chosen apart from the name."
            ),
            Self::CharacterNotAllowed(character) if character.is_control() => {
                write!(formatter, "A file name cannot contain a line break or tab.")
            }
            Self::CharacterNotAllowed(character) => {
                write!(
                    formatter,
                    "A file name cannot contain the character {character}."
                )
            }
            Self::ReservedName(name) => write!(
                formatter,
                "Windows keeps the name {name} for itself. Start the name with something else."
            ),
            Self::SpaceAtAnEnd => write!(
                formatter,
                "The name cannot start with a space, or end with a space or a dot."
            ),
        }
    }
}

impl std::error::Error for PatternError {}

/// A pattern for scene file names that has been checked: it gives every
/// scene of every video a different name, and a name that Windows, macOS
/// and Linux all accept. The video's extension is added to it.
///
/// It is held in place and not as a `String`, so the settings it is part of
/// stay `Copy`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SceneNamePattern {
    /// The pattern as typed, as UTF-8. The bytes after `length` are zero.
    text: [u8; MAX_PATTERN_BYTES],
    length: usize,
}

impl SceneNamePattern {
    pub fn new(typed: &str) -> Result<Self, PatternError> {
        if typed.is_empty() {
            return Err(PatternError::Empty);
        }
        if typed.len() > MAX_PATTERN_BYTES {
            return Err(PatternError::TooLong);
        }
        if typed.contains(PATH_SEPARATORS) {
            return Err(PatternError::PathSeparator);
        }
        let refused = typed.chars().find(|character| {
            character.is_control() || CHARACTERS_WINDOWS_REFUSES.contains(character)
        });
        if let Some(character) = refused {
            return Err(PatternError::CharacterNotAllowed(character));
        }
        if typed.starts_with(' ') || typed.ends_with([' ', '.']) {
            return Err(PatternError::SpaceAtAnEnd);
        }
        let pieces = parse(typed)?;
        let stands_for = |wanted: Placeholder| pieces.contains(&Piece::Stands(wanted));
        if !stands_for(Placeholder::Number) {
            return Err(PatternError::MissingNumber);
        }
        if !stands_for(Placeholder::Name) {
            return Err(PatternError::MissingName);
        }
        if let Some(reserved) = reserved_name_at_the_start(typed) {
            return Err(PatternError::ReservedName(reserved.to_owned()));
        }

        let mut text = [0; MAX_PATTERN_BYTES];
        text[..typed.len()].copy_from_slice(typed.as_bytes());
        Ok(Self {
            text,
            length: typed.len(),
        })
    }

    pub fn as_str(&self) -> &str {
        // The bytes were copied whole from a `&str` in `new`.
        std::str::from_utf8(&self.text[..self.length])
            .expect("a scene name pattern holds the text it was made from")
    }

    fn pieces(&self) -> Vec<Piece<'_>> {
        // Checked in `new`, the only way to make a pattern.
        parse(self.as_str()).expect("a scene name pattern was parsed when it was made")
    }

    /// What the second of twelve scenes of a video called "My video" would
    /// be named, to show beside the box the pattern is typed into.
    pub fn example(&self) -> String {
        let naming = SceneNaming {
            pattern: *self,
            video_name: EXAMPLE_VIDEO_NAME.to_owned(),
            extension: EXAMPLE_EXTENSION.to_owned(),
        };
        naming.file_name(EXAMPLE_SCENE_NUMBER, EXAMPLE_SCENE_COUNT, EXAMPLE_SCENE)
    }
}

impl Default for SceneNamePattern {
    fn default() -> Self {
        Self::new(DEFAULT_PATTERN).expect("the default scene name pattern is valid")
    }
}

impl fmt::Debug for SceneNamePattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SceneNamePattern({:?})", self.as_str())
    }
}

impl fmt::Display for SceneNamePattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for SceneNamePattern {
    type Err = PatternError;

    fn from_str(typed: &str) -> Result<Self, PatternError> {
        Self::new(typed)
    }
}

impl Serialize for SceneNamePattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SceneNamePattern {
    /// A pattern in a settings file is checked like one that is typed, so a
    /// hand-edited file cannot bring in a name that leaves the folder.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let typed = String::deserialize(deserializer)?;
        Self::new(&typed).map_err(|problem| {
            serde::de::Error::custom(format!(
                "the scene name {typed:?} cannot be used: {problem}"
            ))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Piece<'pattern> {
    Text(&'pattern str),
    Stands(Placeholder),
}

fn parse(typed: &str) -> Result<Vec<Piece<'_>>, PatternError> {
    let mut pieces = Vec::new();
    let mut rest = typed;
    while !rest.is_empty() {
        let Some(opening) = rest.find(['{', '}']) else {
            pieces.push(Piece::Text(rest));
            break;
        };
        if rest[opening..].starts_with('}') {
            return Err(PatternError::StrayBrace);
        }
        if opening > 0 {
            pieces.push(Piece::Text(&rest[..opening]));
        }
        let after_opening = &rest[opening + 1..];
        let closing = after_opening.find('}').ok_or(PatternError::StrayBrace)?;
        let word = &after_opening[..closing];
        if word.contains('{') {
            return Err(PatternError::StrayBrace);
        }
        let placeholder = Placeholder::from_word(word)
            .ok_or_else(|| PatternError::UnknownPlaceholder(word.to_owned()))?;
        pieces.push(Piece::Stands(placeholder));
        rest = &after_opening[closing + 1..];
    }
    Ok(pieces)
}

/// The reserved name a pattern would give every scene file, if it starts
/// with one. Windows looks only at what stands before the first dot.
fn reserved_name_at_the_start(typed: &str) -> Option<&'static str> {
    let before_first_dot = typed.split('.').next().unwrap_or(typed).trim_end();
    NAMES_WINDOWS_RESERVES
        .into_iter()
        .find(|reserved| reserved.eq_ignore_ascii_case(before_first_dot))
}

/// A moment or a length as it is written into a file name, where a colon
/// is not allowed: "00h03m12s", in whole seconds, rounded down.
fn time_for_file_name(seconds: f64) -> String {
    let whole_seconds = seconds.max(0.0) as u64;
    format!(
        "{:02}h{:02}m{:02}s",
        whole_seconds / SECONDS_PER_HOUR,
        whole_seconds / SECONDS_PER_MINUTE % SECONDS_PER_MINUTE,
        whole_seconds % SECONDS_PER_MINUTE
    )
}

/// Whether `text` could have been written by `time_for_file_name`.
fn is_time_for_file_name(text: &str) -> bool {
    let Some((hours, rest)) = text.split_once('h') else {
        return false;
    };
    let Some((minutes, rest)) = rest.split_once('m') else {
        return false;
    };
    let Some(seconds) = rest.strip_suffix('s') else {
        return false;
    };
    let all_digits =
        |part: &str| !part.is_empty() && part.chars().all(|digit| digit.is_ascii_digit());
    all_digits(hours) && hours.len() >= 2 && all_digits(minutes) && all_digits(seconds)
}

fn is_all_digits(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|digit| digit.is_ascii_digit())
}

/// Whether `text` could have been written for `{number}`: digits only, and
/// at least as many as every scene number has. "Film 2" and "Film 12" are
/// other videos, not scenes of "Film".
fn is_scene_number(text: &str) -> bool {
    text.len() >= MIN_NUMBER_DIGITS && is_all_digits(text)
}

/// Whether `text` could have been written for `{count}`, which has no
/// leading zeros.
fn is_scene_count(text: &str) -> bool {
    is_all_digits(text) && !text.starts_with('0')
}

/// The number of the first scene as a split writes it when its numbers have
/// `digits` digits: "001", or "0001" in a split into a thousand scenes.
fn first_scene_number(digits: usize) -> String {
    format!("{FIRST_SCENE_NUMBER:0digits$}")
}

/// `text` without `prefix` at its start, or `None` when it starts with
/// something else. Where the system finds a file whatever capitals are used
/// to ask for it, capitals do not tell two names apart here either.
fn strip_prefix_spelled<'text>(
    text: &'text str,
    prefix: &str,
    case: PathCase,
) -> Option<&'text str> {
    if case == PathCase::Matters {
        return text.strip_prefix(prefix);
    }
    let mut rest = text.chars();
    for wanted in prefix.chars() {
        let found = rest.next()?;
        if !found.to_lowercase().eq(wanted.to_lowercase()) {
            return None;
        }
    }
    Some(rest.as_str())
}

/// Whether two file names are the same file in one folder.
pub fn is_same_file_name(one: &str, other: &str, case: PathCase) -> bool {
    strip_prefix_spelled(one, other, case) == Some("")
}

/// Whether a file has the name the default pattern gives a scene, of
/// whatever video. Scenes sit next to the videos they came from, and a
/// scene should not be split a second time just because its folder was
/// added again.
///
/// This knows the default naming only. Which video a file with another name
/// came from cannot be told from the name alone, and guessing would hide
/// videos the person wants to add.
pub fn is_default_scene_file(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let Some((video_name, number)) = stem.rsplit_once(DEFAULT_NAME_MARKER) else {
        return false;
    };
    !video_name.is_empty() && is_scene_number(number)
}

fn digits_of(count: usize) -> usize {
    count.to_string().len()
}

/// Why the scenes of a video cannot be named.
#[derive(Debug, Clone, PartialEq)]
pub enum NamingError {
    /// The video's own name cannot be read as text, or it has none.
    NoUsableVideoName(String),
    NoExtension(String),
    FileNameTooLong(String),
    ReservedName(String),
}

impl fmt::Display for NamingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoUsableVideoName(video) => write!(formatter, "{video} has no usable file name"),
            Self::NoExtension(video) => write!(formatter, "{video} has no file extension"),
            Self::FileNameTooLong(file_name) => write!(
                formatter,
                "the scene file name {file_name} is longer than {MAX_FILE_NAME_BYTES} bytes. \
                 Choose a shorter name for scene files in Settings"
            ),
            Self::ReservedName(file_name) => write!(
                formatter,
                "the scene file name {file_name} is one Windows keeps for itself. \
                 Choose another name for scene files in Settings"
            ),
        }
    }
}

impl std::error::Error for NamingError {}

/// The naming of one video's scene files.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneNaming {
    pattern: SceneNamePattern,
    video_name: String,
    extension: String,
}

impl SceneNaming {
    pub fn of(video: &Path, pattern: SceneNamePattern) -> Result<Self, NamingError> {
        let video_name = video
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| NamingError::NoUsableVideoName(video.display().to_string()))?;
        let extension = video
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| NamingError::NoExtension(video.display().to_string()))?;
        Ok(Self {
            pattern,
            video_name: video_name.to_owned(),
            extension: extension.to_owned(),
        })
    }

    /// The names of the files a split writes for `scenes`, in scene order.
    /// These are the scenes that are written, numbered from one without
    /// gaps: a scene that is dropped is not among them.
    pub fn file_names(&self, scenes: &[TimeSpan]) -> Result<Vec<String>, NamingError> {
        scenes
            .iter()
            .enumerate()
            .map(|(position, scene)| {
                let number = FIRST_SCENE_NUMBER + position;
                let file_name = self.file_name(number, scenes.len(), *scene);
                check_file_name(&file_name).map(|()| file_name)
            })
            .collect()
    }

    fn file_name(&self, number: usize, count: usize, scene: TimeSpan) -> String {
        let digits = MIN_NUMBER_DIGITS.max(digits_of(count));
        let mut file_name = String::new();
        for piece in self.pattern.pieces() {
            match piece {
                Piece::Text(text) => file_name.push_str(text),
                Piece::Stands(Placeholder::Name) => file_name.push_str(&self.video_name),
                Piece::Stands(Placeholder::Number) => {
                    file_name.push_str(&format!("{number:0digits$}"));
                }
                Piece::Stands(Placeholder::Count) => file_name.push_str(&count.to_string()),
                Piece::Stands(Placeholder::Start) => {
                    file_name.push_str(&time_for_file_name(scene.start));
                }
                Piece::Stands(Placeholder::Length) => {
                    file_name.push_str(&time_for_file_name(scene.seconds()));
                }
            }
        }
        file_name.push('.');
        file_name.push_str(&self.extension);
        file_name
    }

    /// The video's own file name, as the naming read it.
    pub fn video_file_name(&self) -> String {
        format!("{}.{}", self.video_name, self.extension)
    }

    pub fn extension(&self) -> &str {
        &self.extension
    }

    /// Those of `files` that an earlier split of this video wrote, going by
    /// their names. Whatever is returned here a new split replaces or
    /// removes, so a name has to fit in every part to count.
    ///
    /// A number with more digits than scene numbers usually have is only
    /// written by a split into a thousand scenes or more, and such a split
    /// also wrote its first scene with that many digits. Without that first
    /// scene among `files`, a longer number is taken for part of another
    /// video's name: "Film 2049" is not scene 2049 of "Film".
    pub fn scenes_of_earlier_splits(&self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        self.scenes_of_earlier_splits_spelled(files, SYSTEM_PATH_CASE)
    }

    fn scenes_of_earlier_splits_spelled(
        &self,
        files: Vec<PathBuf>,
        case: PathCase,
    ) -> Vec<PathBuf> {
        let numbered: Vec<(PathBuf, Vec<String>)> = files
            .into_iter()
            .map(|file| {
                let numbers = self
                    .scene_numbers_in(&file, case)
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                (file, numbers)
            })
            .collect();
        let is_among_the_numbers = |wanted: &str| {
            numbered
                .iter()
                .any(|(_, numbers)| numbers.iter().any(|number| number == wanted))
        };
        let could_be_from_a_split = |number: &String| {
            number.len() == MIN_NUMBER_DIGITS
                || is_among_the_numbers(&first_scene_number(number.len()))
        };
        numbered
            .iter()
            .filter(|(_, numbers)| numbers.iter().any(could_be_from_a_split))
            .map(|(file, _)| file.clone())
            .collect()
    }

    /// Every scene number the name of `path` can be read as holding, when
    /// the name is one this naming could have given a scene of this video,
    /// whatever its number, its count and its times. Empty for any other
    /// name. There is more than one reading only where a pattern puts two
    /// numbers side by side, as "{number}{count}" does.
    fn scene_numbers_in<'path>(&self, path: &'path Path, case: PathCase) -> Vec<&'path str> {
        let same_extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(&self.extension));
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return Vec::new();
        };
        if !same_extension {
            return Vec::new();
        }
        let mut numbers = Vec::new();
        let reading = Reading {
            text: stem,
            number: None,
        };
        self.read_numbers(&self.pattern.pieces(), reading, case, &mut numbers);
        numbers
    }

    /// Adds to `numbers` the scene number of every way the rest of a name
    /// is what `pieces` could have written. A placeholder can stand for
    /// text of several lengths, so each length is tried.
    fn read_numbers<'text>(
        &self,
        pieces: &[Piece],
        reading: Reading<'text>,
        case: PathCase,
        numbers: &mut Vec<&'text str>,
    ) {
        let Reading { text, number } = reading;
        let Some((first, later_pieces)) = pieces.split_first() else {
            if let (true, Some(number)) = (text.is_empty(), number) {
                if !numbers.contains(&number) {
                    numbers.push(number);
                }
            }
            return;
        };
        let fixed_text = match first {
            Piece::Text(fixed) => Some(*fixed),
            Piece::Stands(Placeholder::Name) => Some(self.video_name.as_str()),
            Piece::Stands(_) => None,
        };
        if let Some(fixed) = fixed_text {
            if let Some(rest) = strip_prefix_spelled(text, fixed, case) {
                let reading = Reading { text: rest, number };
                self.read_numbers(later_pieces, reading, case, numbers);
            }
            return;
        }
        let is_the_scene_number = *first == Piece::Stands(Placeholder::Number);
        let is_what_it_stands_for: fn(&str) -> bool = match first {
            Piece::Stands(Placeholder::Start | Placeholder::Length) => is_time_for_file_name,
            Piece::Stands(Placeholder::Count) => is_scene_count,
            _ => is_scene_number,
        };
        let ends = text
            .char_indices()
            .map(|(end, character)| end + character.len_utf8())
            .filter(|&end| is_what_it_stands_for(&text[..end]));
        for end in ends {
            let (written, rest) = text.split_at(end);
            let number = match (is_the_scene_number, number) {
                (false, number) => number,
                (true, None) => Some(written),
                (true, Some(read_before)) if read_before == written => number,
                // A pattern that names the number twice writes the same
                // number twice.
                (true, Some(_another_number)) => continue,
            };
            let reading = Reading { text: rest, number };
            self.read_numbers(later_pieces, reading, case, numbers);
        }
    }
}

/// How far a file name has been read against a pattern.
#[derive(Clone, Copy)]
struct Reading<'text> {
    /// What of the name is still to be read.
    text: &'text str,
    /// The scene number read so far, once the pattern has named it.
    number: Option<&'text str>,
}

fn check_file_name(file_name: &str) -> Result<(), NamingError> {
    if file_name.len() > MAX_FILE_NAME_BYTES {
        return Err(NamingError::FileNameTooLong(file_name.to_owned()));
    }
    if reserved_name_at_the_start(file_name).is_some() {
        return Err(NamingError::ReservedName(file_name.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(typed: &str) -> SceneNamePattern {
        SceneNamePattern::new(typed).unwrap()
    }

    fn naming(video: &str, typed: &str) -> SceneNaming {
        SceneNaming::of(Path::new(video), pattern(typed)).unwrap()
    }

    /// Whether the name alone fits a scene of this video, with capitals
    /// counting.
    fn is_named_as_a_scene(naming: &SceneNaming, file_name: &str) -> bool {
        !naming
            .scene_numbers_in(Path::new(file_name), PathCase::Matters)
            .is_empty()
    }

    /// Which of `file_names` an earlier split of `video` wrote, going by
    /// the pattern `typed`.
    fn earlier_scenes(video: &str, typed: &str, file_names: &[&str]) -> Vec<String> {
        let files = file_names.iter().map(PathBuf::from).collect();
        naming(video, typed)
            .scenes_of_earlier_splits_spelled(files, PathCase::Matters)
            .into_iter()
            .map(|file| file.display().to_string())
            .collect()
    }

    fn scenes_of_a_minute(count: usize) -> Vec<TimeSpan> {
        (0..count)
            .map(|scene| TimeSpan {
                start: scene as f64 * 60.0,
                end: (scene + 1) as f64 * 60.0,
            })
            .collect()
    }

    #[test]
    fn the_default_pattern_gives_the_names_the_app_has_always_written() {
        let names = SceneNaming::of(Path::new("videos/holiday.mp4"), SceneNamePattern::default())
            .unwrap()
            .file_names(&scenes_of_a_minute(2))
            .unwrap();
        assert_eq!(names, ["holiday_scene_001.mp4", "holiday_scene_002.mp4"]);
    }

    #[test]
    fn every_placeholder_is_filled_in() {
        let names = naming(
            "film.mkv",
            "{name} {number} of {count} at {start} for {length}",
        )
        .file_names(&[
            TimeSpan {
                start: 0.0,
                end: 192.9,
            },
            TimeSpan {
                start: 192.9,
                end: 3800.0,
            },
        ])
        .unwrap();
        assert_eq!(
            names,
            [
                "film 001 of 2 at 00h00m00s for 00h03m12s.mkv",
                "film 002 of 2 at 00h03m12s for 01h00m07s.mkv",
            ]
        );
    }

    #[test]
    fn a_split_into_a_thousand_scenes_uses_four_digits_for_every_number() {
        let names = naming("film.mp4", "{name}_{number}")
            .file_names(&scenes_of_a_minute(1000))
            .unwrap();
        assert_eq!(names[0], "film_0001.mp4");
        assert_eq!(names[999], "film_1000.mp4");
    }

    #[test]
    fn a_percent_sign_and_dots_of_the_video_name_are_kept() {
        let names = naming("100% fun.v2.mkv", "{name}_scene_{number}")
            .file_names(&scenes_of_a_minute(1))
            .unwrap();
        assert_eq!(names, ["100% fun.v2_scene_001.mkv"]);
    }

    #[test]
    fn a_pattern_without_the_scene_number_is_refused() {
        assert_eq!(
            SceneNamePattern::new("{name} scene"),
            Err(PatternError::MissingNumber)
        );
        assert_eq!(
            SceneNamePattern::new("{name} {start}"),
            Err(PatternError::MissingNumber)
        );
    }

    #[test]
    fn a_pattern_without_the_video_name_is_refused() {
        assert_eq!(
            SceneNamePattern::new("scene {number}"),
            Err(PatternError::MissingName)
        );
    }

    #[test]
    fn a_pattern_that_would_leave_the_folder_is_refused() {
        for typed in [
            "../{name}_{number}",
            r"scenes\{name}_{number}",
            "/{name}{number}",
        ] {
            assert_eq!(
                SceneNamePattern::new(typed),
                Err(PatternError::PathSeparator),
                "{typed}"
            );
        }
    }

    #[test]
    fn characters_a_file_name_cannot_hold_are_refused_by_name() {
        for character in CHARACTERS_WINDOWS_REFUSES {
            assert_eq!(
                SceneNamePattern::new(&format!("{{name}}{character}{{number}}")),
                Err(PatternError::CharacterNotAllowed(character))
            );
        }
        assert_eq!(
            SceneNamePattern::new("{name}\n{number}"),
            Err(PatternError::CharacterNotAllowed('\n'))
        );
        assert_eq!(
            SceneNamePattern::new("{name}\0{number}"),
            Err(PatternError::CharacterNotAllowed('\0'))
        );
    }

    #[test]
    fn names_windows_keeps_for_itself_are_refused_in_any_case() {
        assert_eq!(
            SceneNamePattern::new("con.{name}{number}"),
            Err(PatternError::ReservedName("CON".to_owned()))
        );
        assert_eq!(
            SceneNamePattern::new("LPT1 .{name}{number}"),
            Err(PatternError::ReservedName("LPT1".to_owned()))
        );
        // Only the whole part before the first dot counts.
        assert!(SceneNamePattern::new("console {name} {number}").is_ok());
    }

    #[test]
    fn a_space_or_dot_at_an_end_is_refused() {
        for typed in [" {name}{number}", "{name}{number} ", "{name}{number}."] {
            assert_eq!(
                SceneNamePattern::new(typed),
                Err(PatternError::SpaceAtAnEnd),
                "{typed:?}"
            );
        }
    }

    #[test]
    fn an_empty_or_overlong_pattern_is_refused() {
        assert_eq!(SceneNamePattern::new(""), Err(PatternError::Empty));
        let overlong = format!("{{name}}{{number}}{}", "x".repeat(MAX_PATTERN_BYTES));
        assert_eq!(SceneNamePattern::new(&overlong), Err(PatternError::TooLong));
    }

    #[test]
    fn a_pattern_of_exactly_the_longest_length_is_taken() {
        let filler = "x".repeat(MAX_PATTERN_BYTES - "{name}{number}".len());
        let longest = format!("{{name}}{{number}}{filler}");
        assert_eq!(pattern(&longest).as_str(), longest);
    }

    #[test]
    fn an_unknown_placeholder_is_named() {
        assert_eq!(
            SceneNamePattern::new("{name}_{numbr}"),
            Err(PatternError::UnknownPlaceholder("numbr".to_owned()))
        );
        assert_eq!(
            SceneNamePattern::new("{name}_{Number}"),
            Err(PatternError::UnknownPlaceholder("Number".to_owned()))
        );
    }

    #[test]
    fn a_brace_that_closes_nothing_or_is_never_closed_is_refused() {
        for typed in [
            "{name}_{number",
            "{name}}_{number}",
            "{name}_{{number}}",
            "{}",
        ] {
            assert!(
                matches!(
                    SceneNamePattern::new(typed),
                    Err(PatternError::StrayBrace | PatternError::UnknownPlaceholder(_))
                ),
                "{typed}"
            );
        }
    }

    #[test]
    fn every_refusal_says_what_to_change_without_a_dash() {
        let refusals = [
            PatternError::Empty,
            PatternError::TooLong,
            PatternError::MissingNumber,
            PatternError::MissingName,
            PatternError::UnknownPlaceholder("numbr".to_owned()),
            PatternError::StrayBrace,
            PatternError::PathSeparator,
            PatternError::CharacterNotAllowed('?'),
            PatternError::CharacterNotAllowed('\n'),
            PatternError::ReservedName("CON".to_owned()),
            PatternError::SpaceAtAnEnd,
        ];
        for refusal in refusals {
            let words = refusal.to_string();
            assert!(!words.is_empty() && !words.contains('\u{2014}'), "{words}");
        }
        assert_eq!(
            PatternError::CharacterNotAllowed('?').to_string(),
            "A file name cannot contain the character ?."
        );
    }

    #[test]
    fn letters_beyond_ascii_are_welcome() {
        let names = naming("été.mp4", "{name} scène {number}")
            .file_names(&scenes_of_a_minute(1))
            .unwrap();
        assert_eq!(names, ["été scène 001.mp4"]);
    }

    #[test]
    fn the_example_shows_a_whole_file_name() {
        assert_eq!(
            SceneNamePattern::default().example(),
            "My video_scene_002.mp4"
        );
        assert_eq!(
            pattern("{name} ({number} of {count}) {start}").example(),
            "My video (002 of 12) 00h03m12s.mp4"
        );
    }

    #[test]
    fn a_pattern_is_saved_as_the_text_that_was_typed() {
        let saved = serde_json::to_string(&pattern("{name} part {number}")).unwrap();
        assert_eq!(saved, r#""{name} part {number}""#);
        let loaded: SceneNamePattern = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded, pattern("{name} part {number}"));
    }

    #[test]
    fn a_saved_pattern_that_would_leave_the_folder_does_not_load() {
        let loaded = serde_json::from_str::<SceneNamePattern>(r#""../{name}{number}""#);
        assert!(loaded.is_err());
    }

    #[test]
    fn equal_patterns_are_equal_whatever_was_made_before_them() {
        assert_eq!(pattern("{name}{number}"), pattern("{name}{number}"));
        assert_ne!(pattern("{name}{number}"), pattern("{name}_{number}"));
    }

    #[test]
    fn scenes_of_an_earlier_split_are_recognised_whatever_their_number() {
        let naming = naming("holiday.mp4", "{name}_scene_{number}");
        for scene_file in [
            "holiday_scene_001.mp4",
            "holiday_scene_1000.mp4",
            "out/holiday_scene_007.MP4",
        ] {
            assert!(is_named_as_a_scene(&naming, scene_file), "{scene_file}");
        }
    }

    #[test]
    fn other_files_are_not_taken_for_scenes_of_this_video() {
        let naming = naming("holiday.mp4", "{name}_scene_{number}");
        for other in [
            "holiday.mp4",
            "holiday_scene_001.mkv",
            "holiday2_scene_001.mp4",
            "holiday_scene_notes.mp4",
            "holiday_scene_.mp4",
            "holiday_scene_001_copy.mp4",
            // Scene numbers have three digits or more.
            "holiday_scene_1.mp4",
            "holiday_scene_12.mp4",
        ] {
            assert!(!is_named_as_a_scene(&naming, other), "{other}");
        }
    }

    #[test]
    fn every_name_a_pattern_writes_is_recognised_as_a_scene_again() {
        let patterns = [
            "{name}_scene_{number}",
            "{number} {name}",
            "{name} {number} of {count}",
            "{name} {start} {number} ({length})",
            "{number}{count}{name}",
        ];
        for typed in patterns {
            let naming = naming("My film 2.mp4", typed);
            for file_name in naming.file_names(&scenes_of_a_minute(12)).unwrap() {
                assert!(
                    is_named_as_a_scene(&naming, &file_name),
                    "{file_name} of {typed}"
                );
            }
            assert!(!is_named_as_a_scene(&naming, "My film 2.mp4"));
            let written: Vec<PathBuf> = naming
                .file_names(&scenes_of_a_minute(1200))
                .unwrap()
                .into_iter()
                .map(PathBuf::from)
                .collect();
            assert_eq!(
                naming.scenes_of_earlier_splits(written.clone()),
                written,
                "a split into more than a thousand scenes, named {typed}"
            );
        }
    }

    #[test]
    fn a_scene_file_name_that_is_too_long_is_refused_when_naming() {
        let long_name = format!("{}.mp4", "a".repeat(250));
        let error = naming(&long_name, "{name}_scene_{number}")
            .file_names(&scenes_of_a_minute(1))
            .unwrap_err();
        assert!(matches!(error, NamingError::FileNameTooLong(_)));
    }

    #[test]
    fn a_video_without_an_extension_cannot_be_named() {
        assert!(matches!(
            SceneNaming::of(Path::new("holiday"), SceneNamePattern::default()),
            Err(NamingError::NoExtension(_))
        ));
    }

    #[test]
    fn every_placeholder_can_be_typed_and_read_back() {
        for placeholder in Placeholder::ALL {
            let typed = placeholder.typed();
            assert_eq!(parse(&typed), Ok(vec![Piece::Stands(placeholder)]));
            assert!(!placeholder.meaning().is_empty());
        }
    }

    #[test]
    fn another_video_with_a_number_in_its_name_is_not_a_scene_of_this_one() {
        // Each pair is a video and another video beside it, which a split
        // of the first must never replace or remove.
        let videos_and_neighbours = [
            ("Rocky.mp4", "{name} {number}", "Rocky 2.mp4"),
            (
                "Blade Runner.mp4",
                "{name} {number}",
                "Blade Runner 2049.mp4",
            ),
            ("show.mp4", "{name}_{number}", "show_2.mp4"),
            ("clip.mp4", "{name}{number}", "clip1.mp4"),
            ("clip.mp4", "{name}{number}", "clip12.mp4"),
            ("film.mp4", "{name} - {number}", "film - 1984.mp4"),
        ];
        for (video, typed, neighbour) in videos_and_neighbours {
            assert_eq!(
                earlier_scenes(video, typed, &[video, neighbour]),
                Vec::<String>::new(),
                "{neighbour} beside {video}, named {typed}"
            );
        }
    }

    #[test]
    fn a_video_with_a_long_number_stays_out_of_a_split_with_three_digit_scenes() {
        assert_eq!(
            earlier_scenes(
                "Blade Runner.mp4",
                "{name} {number}",
                &[
                    "Blade Runner 001.mp4",
                    "Blade Runner 002.mp4",
                    "Blade Runner 2049.mp4"
                ]
            ),
            ["Blade Runner 001.mp4", "Blade Runner 002.mp4"]
        );
    }

    #[test]
    fn scenes_with_four_digits_count_when_their_first_scene_is_there() {
        assert_eq!(
            earlier_scenes(
                "film.mp4",
                "{name}_{number}",
                &["film_0001.mp4", "film_1000.mp4", "film_10000.mp4"]
            ),
            ["film_0001.mp4", "film_1000.mp4"]
        );
    }

    #[test]
    fn a_pattern_that_names_the_number_twice_wants_the_same_number_twice() {
        let naming = naming("film.mp4", "{number} {name} {number}");
        assert!(is_named_as_a_scene(&naming, "007 film 007.mp4"));
        assert!(!is_named_as_a_scene(&naming, "007 film 008.mp4"));
    }

    #[test]
    fn a_count_with_a_leading_zero_was_not_written_by_a_split() {
        let naming = naming("film.mp4", "{name} {number} of {count}");
        assert!(is_named_as_a_scene(&naming, "film 001 of 12.mp4"));
        assert!(!is_named_as_a_scene(&naming, "film 001 of 012.mp4"));
    }

    #[test]
    fn capitals_tell_scene_files_apart_only_where_the_system_does() {
        let naming = naming("Holiday.mp4", "{name} Part {number}");
        let retyped = Path::new("holiday part 001.mp4");
        assert!(naming
            .scene_numbers_in(retyped, PathCase::Matters)
            .is_empty());
        assert_eq!(naming.scene_numbers_in(retyped, PathCase::Ignored), ["001"]);
        // Letters beyond ASCII have capitals too.
        assert_eq!(
            strip_prefix_spelled("\u{c9}t\u{e9} 1", "\u{e9}T\u{c9}", PathCase::Ignored),
            Some(" 1")
        );
        assert_eq!(strip_prefix_spelled("abc", "abcd", PathCase::Ignored), None);
        assert!(is_same_file_name("Film.MP4", "film.mp4", PathCase::Ignored));
        assert!(!is_same_file_name(
            "Film.MP4",
            "film.mp4",
            PathCase::Matters
        ));
        assert!(!is_same_file_name(
            "film.mp4x",
            "film.mp4",
            PathCase::Ignored
        ));
    }

    #[test]
    fn the_default_marker_is_the_one_in_the_default_pattern() {
        assert_eq!(
            DEFAULT_PATTERN,
            format!("{{name}}{DEFAULT_NAME_MARKER}{{number}}")
        );
    }

    #[test]
    fn scenes_named_by_default_are_recognised_whatever_their_video() {
        assert!(is_default_scene_file(Path::new(
            "videos/holiday_scene_001.mp4"
        )));
        assert!(is_default_scene_file(Path::new("holiday_scene_123.mkv")));
        // A split into a thousand scenes or more numbers with four digits.
        assert!(is_default_scene_file(Path::new("holiday_scene_1000.mp4")));
        let names = naming("holiday.mp4", DEFAULT_PATTERN)
            .file_names(&scenes_of_a_minute(1000))
            .unwrap();
        assert!(names
            .iter()
            .all(|name| is_default_scene_file(Path::new(name))));
    }

    #[test]
    fn ordinary_videos_are_not_mistaken_for_scenes_named_by_default() {
        assert!(!is_default_scene_file(Path::new("holiday.mp4")));
        assert!(!is_default_scene_file(Path::new(
            "crime_scene_investigation.mp4"
        )));
        assert!(!is_default_scene_file(Path::new("_scene_001.mp4")));
        assert!(!is_default_scene_file(Path::new("holiday_scene_1.mp4")));
    }
}
