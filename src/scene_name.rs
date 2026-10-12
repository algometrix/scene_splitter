//! How scene files are named: a pattern the person chooses, such as
//! "{name}_scene_{number}_of_{count}", and the names it gives the scenes of
//! one video.
//!
//! The same naming answers two questions, so they cannot drift apart: what
//! a split calls its files, and which files in a folder an earlier split of
//! the same video left there. The second answer decides what a new split
//! replaces and removes, so a name has to fit in every part to count, and
//! what is found is handed on one earlier split at a time (`EarlierSplit`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::folder_settings::{PathCase, SYSTEM_PATH_CASE};
use crate::timecode::TimeSpan;

/// The pattern in use unless the person chooses another one:
/// "<video name>_scene_1_of_5" and so on. The count is in the name so that
/// a scene file says how many scenes its split wrote.
pub const DEFAULT_PATTERN: &str = "{name}_scene_{number}_of_{count}";
/// The default before the count was part of it: "<video name>_scene_001".
/// Nothing is named this way by default any more, but the files are still
/// in people's folders, so an earlier split named this way is always
/// recognised.
const DEFAULT_PATTERN_BEFORE_THE_COUNT: &str = "{name}_scene_{number}";
/// Scenes are numbered from one.
pub const FIRST_SCENE_NUMBER: usize = 1;
/// Long enough for any sensible name, and short enough to be held in the
/// settings without an allocation, which keeps the settings cheap to copy
/// and compare in every frame.
pub const MAX_PATTERN_BYTES: usize = 100;
/// What most file systems allow for one file name.
const MAX_FILE_NAME_BYTES: usize = 255;
/// A scene number in a name that does not say the count has at least this
/// many digits. A split into a thousand scenes or more uses as many digits
/// as its count, so that the files still sort in scene order.
const MIN_NUMBER_DIGITS: usize = 3;
/// How many patterns that splits were written with are remembered. Enough
/// for someone who tries a few names before settling, and few enough to
/// keep the settings small.
pub const MOST_PATTERNS_REMEMBERED: usize = 8;
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
    /// The scene's number, counted from one, with leading zeros so that
    /// the files of one split sort in scene order.
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

    /// The default before the count was part of it.
    pub fn default_before_the_count() -> Self {
        Self::new(DEFAULT_PATTERN_BEFORE_THE_COUNT)
            .expect("the default scene name pattern of before is valid")
    }

    fn names_the_count(&self) -> bool {
        self.pieces().contains(&Piece::Stands(Placeholder::Count))
    }

    /// How many digits a split named by this pattern gives its scene
    /// numbers.
    fn number_digits(&self) -> NumberDigits {
        if self.names_the_count() {
            NumberDigits::AsTheCountHas
        } else {
            NumberDigits::AtLeastThree
        }
    }

    /// What the second of twelve scenes of a video called "My video" would
    /// be named, to show beside the box the pattern is typed into.
    pub fn example(&self) -> String {
        let naming = SceneNaming {
            pattern: *self,
            video: VideoFileName {
                name: EXAMPLE_VIDEO_NAME.to_owned(),
                extension: EXAMPLE_EXTENSION.to_owned(),
            },
        };
        naming.file_name(EXAMPLE_SCENE_NUMBER, EXAMPLE_SCENE_COUNT, EXAMPLE_SCENE)
    }
}

impl Default for SceneNamePattern {
    fn default() -> Self {
        Self::new(DEFAULT_PATTERN).expect("the default scene name pattern is valid")
    }
}

/// How many digits a split gives its scene numbers.
///
/// A name that says the count needs no more digits than the count has: "07"
/// of 12 sorts in scene order, and so does "1" of 5. A name that does not
/// say the count keeps three digits at least. Without the count beside it, a
/// short number is too easily part of another video's name: "Rocky 2" is not
/// the second scene of "Rocky".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumberDigits {
    AsTheCountHas,
    /// Also what every pattern wrote before the count decided the digits,
    /// so scene files named "film 001 of 5" are in people's folders.
    AtLeastThree,
}

impl NumberDigits {
    fn in_a_split_into(self, count: usize) -> usize {
        match self {
            Self::AsTheCountHas => digits_of(count),
            Self::AtLeastThree => MIN_NUMBER_DIGITS.max(digits_of(count)),
        }
    }
}

/// The patterns splits were written with, newest first, each once, and no
/// more than `MOST_PATTERNS_REMEMBERED` of them. They are how the scene
/// files of an earlier split are still known for what they are after the
/// person has chosen another name.
///
/// Held in place like a pattern, so what it is part of stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct PatternsUsed {
    /// The remembered patterns come first, with no gap between them.
    newest_first: [Option<SceneNamePattern>; MOST_PATTERNS_REMEMBERED],
}

impl PatternsUsed {
    /// Puts `pattern` first. The oldest one is forgotten when there is no
    /// room left.
    pub fn remember(&mut self, pattern: SceneNamePattern) {
        let older: Vec<SceneNamePattern> = self.iter().filter(|known| *known != pattern).collect();
        let newest_first = std::iter::once(pattern)
            .chain(older)
            .take(MOST_PATTERNS_REMEMBERED);
        self.newest_first = [None; MOST_PATTERNS_REMEMBERED];
        for (place, pattern) in self.newest_first.iter_mut().zip(newest_first) {
            *place = Some(pattern);
        }
    }

    /// The pattern of the newest split, if any was remembered.
    pub fn newest(&self) -> Option<SceneNamePattern> {
        self.iter().next()
    }

    pub fn iter(&self) -> impl Iterator<Item = SceneNamePattern> + '_ {
        self.newest_first.iter().flatten().copied()
    }
}

impl fmt::Debug for PatternsUsed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.iter()).finish()
    }
}

impl Serialize for PatternsUsed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for PatternsUsed {
    /// A list in a settings file is brought into shape like one that grew
    /// here: a pattern named twice counts once, and what is beyond the
    /// limit is left out.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let newest_first = Vec::<SceneNamePattern>::deserialize(deserializer)?;
        let mut patterns = Self::default();
        for pattern in newest_first.into_iter().rev() {
            patterns.remember(pattern);
        }
        Ok(patterns)
    }
}

/// One way the scenes of a video are named, or once were: a pattern, and
/// how many digits its scene numbers have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NameScheme {
    pattern: SceneNamePattern,
    number_digits: NumberDigits,
}

impl NameScheme {
    /// How a split names its scenes by `pattern` now.
    fn of(pattern: SceneNamePattern) -> Self {
        Self {
            pattern,
            number_digits: pattern.number_digits(),
        }
    }

    /// How a split named its scenes by `pattern` when every number had
    /// three digits at least.
    fn with_three_digits_at_least(pattern: SceneNamePattern) -> Self {
        Self {
            pattern,
            number_digits: NumberDigits::AtLeastThree,
        }
    }
}

/// The names scene files get now, and the names they may have got from an
/// earlier split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneNames {
    in_use: SceneNamePattern,
    used_before: PatternsUsed,
}

impl SceneNames {
    pub fn new(in_use: SceneNamePattern, used_before: PatternsUsed) -> Self {
        Self {
            in_use,
            used_before,
        }
    }

    /// The pattern a split names its scenes by now.
    pub fn in_use(&self) -> SceneNamePattern {
        self.in_use
    }

    /// Every way an earlier split may have named its scenes, each once.
    /// The first is how scenes are named now. Then come the patterns that
    /// were used before, and last the two defaults, which are known
    /// whatever was remembered.
    fn schemes(&self) -> Vec<NameScheme> {
        let defaults = [
            SceneNamePattern::default(),
            SceneNamePattern::default_before_the_count(),
        ];
        let patterns = std::iter::once(self.in_use)
            .chain(self.used_before.iter())
            .chain(defaults);
        let mut schemes = Vec::new();
        for pattern in patterns {
            let ways_it_was_written = [
                NameScheme::of(pattern),
                NameScheme::with_three_digits_at_least(pattern),
            ];
            for scheme in ways_it_was_written {
                if !schemes.contains(&scheme) {
                    schemes.push(scheme);
                }
            }
        }
        schemes
    }

    /// Whether a file has a name a split gives a scene, of whatever video.
    /// Scenes sit next to the videos they came from, and a scene should not
    /// be split a second time just because its folder was added again.
    ///
    /// Only names that say enough count: the two defaults, with "_scene_"
    /// in them, and patterns that name the count, where the number has to
    /// fit the count. Which video a file named "{name} {number}" came from
    /// cannot be told from the name alone, and guessing would hide videos
    /// the person wants to add.
    pub fn is_scene_file_of_some_video(&self, path: &Path) -> bool {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return false;
        };
        let default_before_the_count = SceneNamePattern::default_before_the_count();
        self.schemes()
            .into_iter()
            .filter(|scheme| {
                scheme.pattern.names_the_count() || scheme.pattern == default_before_the_count
            })
            .any(|scheme| {
                let reader = NameReader {
                    video_name: VideoName::OfAnyVideo,
                    number_digits: scheme.number_digits,
                    case: SYSTEM_PATH_CASE,
                };
                !reader
                    .readings_of(stem, &scheme.pattern.pieces())
                    .is_empty()
            })
    }
}

impl From<SceneNamePattern> for SceneNames {
    /// The names of someone who never split under another pattern.
    fn from(in_use: SceneNamePattern) -> Self {
        Self::new(in_use, PatternsUsed::default())
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

/// Whether `text` could have been written for `{count}`, which has no
/// leading zeros. So a count of 0 was never written either.
fn is_scene_count(text: &str) -> bool {
    is_all_digits(text) && !text.starts_with('0')
}

/// How many bytes of digits `text` starts with.
fn leading_digits(text: &str) -> usize {
    text.bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
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

/// Whether two file names are the same file in one folder. Also asked of
/// the endings of two names.
pub fn is_same_file_name(one: &str, other: &str, case: PathCase) -> bool {
    strip_prefix_spelled(one, other, case) == Some("")
}

/// What the name of a scene file says about the split that wrote it. There
/// is one only for a name that fits its naming in every part, so the number
/// is a scene's number in a split that could have been.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SceneFileReading {
    /// From one up to the count, where the name says the count.
    number: usize,
    /// How many digits the number is written with, leading zeros included.
    number_digits: usize,
    /// How many scenes the split wrote, where the name says so.
    count: Option<usize>,
}

impl SceneFileReading {
    /// What a name says in which `number` stands for the scene's number and
    /// `count`, if the name has one, for the count. `None` when no split
    /// that gives its numbers `digits` writes those two together:
    ///
    /// - a number of 0, or one too long to be a number at all,
    /// - a number above the count,
    /// - a number with more or fewer digits than its split gives it, which
    ///   is as many as the count says, or three at least without a count.
    fn of_written(number: &str, count: Option<&str>, digits: NumberDigits) -> Option<Self> {
        let number_digits = number.len();
        let number: usize = number.parse().ok()?;
        if number < FIRST_SCENE_NUMBER {
            return None;
        }
        let Some(count) = count else {
            return (number_digits >= MIN_NUMBER_DIGITS).then_some(Self {
                number,
                number_digits,
                count: None,
            });
        };
        let count: usize = count.parse().ok()?;
        if number > count || number_digits != digits.in_a_split_into(count) {
            return None;
        }
        Some(Self {
            number,
            number_digits,
            count: Some(count),
        })
    }
}

/// Whose scenes a name is read as.
#[derive(Debug, Clone, Copy)]
enum VideoName<'video> {
    /// The scenes of this video. Its name has to stand where `{name}` is,
    /// letter for letter.
    Known(&'video str),
    /// The scenes of whatever video. Any text stands for `{name}`, as long
    /// as there is some.
    OfAnyVideo,
}

/// Reads file names against one naming.
#[derive(Debug, Clone, Copy)]
struct NameReader<'video> {
    video_name: VideoName<'video>,
    number_digits: NumberDigits,
    case: PathCase,
}

/// How far a file name has been read against a pattern.
#[derive(Clone, Copy)]
struct Reading<'text> {
    /// What of the name is still to be read.
    rest: &'text str,
    /// What was read for each placeholder that has to say the same
    /// wherever a pattern names it twice.
    name: Option<&'text str>,
    number: Option<&'text str>,
    count: Option<&'text str>,
}

impl NameReader<'_> {
    /// Every way `stem`, a file name without its extension, is a name that
    /// `pieces` give a scene. Empty for any other name. There is more than
    /// one way only where a pattern puts two numbers side by side, as
    /// "{number}{count}" does, and seldom then.
    fn readings_of(&self, stem: &str, pieces: &[Piece]) -> Vec<SceneFileReading> {
        let nothing_read_yet = Reading {
            rest: stem,
            name: None,
            number: None,
            count: None,
        };
        let mut readings = Vec::new();
        self.read(pieces, nothing_read_yet, &mut readings);
        readings
    }

    /// Adds to `readings` every way the rest of a name is what `pieces`
    /// could have written. A placeholder can stand for text of several
    /// lengths, so each length is tried.
    fn read<'text>(
        &self,
        pieces: &[Piece],
        reading: Reading<'text>,
        readings: &mut Vec<SceneFileReading>,
    ) {
        let Some((first, later_pieces)) = pieces.split_first() else {
            self.finish(reading, readings);
            return;
        };
        let rest = reading.rest;
        match first {
            Piece::Text(fixed) => {
                if let Some(rest) = strip_prefix_spelled(rest, fixed, self.case) {
                    self.read(later_pieces, Reading { rest, ..reading }, readings);
                }
            }
            Piece::Stands(Placeholder::Name) => {
                for (name, rest) in self.names_at_the_start_of(rest, reading.name) {
                    let reading = Reading {
                        rest,
                        name: Some(name),
                        ..reading
                    };
                    self.read(later_pieces, reading, readings);
                }
            }
            Piece::Stands(Placeholder::Number) => {
                for (number, rest) in numbers_at_the_start_of(rest, reading.number) {
                    let reading = Reading {
                        rest,
                        number: Some(number),
                        ..reading
                    };
                    self.read(later_pieces, reading, readings);
                }
            }
            Piece::Stands(Placeholder::Count) => {
                let counts = numbers_at_the_start_of(rest, reading.count)
                    .into_iter()
                    .filter(|(count, _rest)| is_scene_count(count));
                for (count, rest) in counts {
                    let reading = Reading {
                        rest,
                        count: Some(count),
                        ..reading
                    };
                    self.read(later_pieces, reading, readings);
                }
            }
            Piece::Stands(Placeholder::Start | Placeholder::Length) => {
                let ends = rest
                    .char_indices()
                    .map(|(end, character)| end + character.len_utf8())
                    .filter(|&end| is_time_for_file_name(&rest[..end]));
                for end in ends {
                    let reading = Reading {
                        rest: &rest[end..],
                        ..reading
                    };
                    self.read(later_pieces, reading, readings);
                }
            }
        }
    }

    /// The pattern is read to its end. The name counts only when it is read
    /// to its end too, and what it says could have been written.
    fn finish(&self, reading: Reading, readings: &mut Vec<SceneFileReading>) {
        if !reading.rest.is_empty() {
            return;
        }
        // A pattern always names the number, so one was read.
        let Some(number) = reading.number else {
            return;
        };
        let Some(read) = SceneFileReading::of_written(number, reading.count, self.number_digits)
        else {
            return;
        };
        if !readings.contains(&read) {
            readings.push(read);
        }
    }

    /// Each way `text` starts with the video's name, with what follows it.
    /// A pattern that names the video twice has the same name twice.
    fn names_at_the_start_of<'text>(
        &self,
        text: &'text str,
        read_before: Option<&'text str>,
    ) -> Vec<(&'text str, &'text str)> {
        let known = match (self.video_name, read_before) {
            (VideoName::Known(video_name), _) => Some(video_name),
            (VideoName::OfAnyVideo, Some(read_before)) => Some(read_before),
            (VideoName::OfAnyVideo, None) => None,
        };
        if let Some(known) = known {
            return strip_prefix_spelled(text, known, self.case)
                .map(|rest| (&text[..text.len() - rest.len()], rest))
                .into_iter()
                .collect();
        }
        text.char_indices()
            .map(|(end, character)| end + character.len_utf8())
            .map(|end| text.split_at(end))
            .collect()
    }
}

/// Each run of digits `text` starts with, with what follows it. A pattern
/// that names a number twice writes the same digits twice, so after
/// `read_before` only those count.
fn numbers_at_the_start_of<'text>(
    text: &'text str,
    read_before: Option<&'text str>,
) -> Vec<(&'text str, &'text str)> {
    (1..=leading_digits(text))
        .map(|end| text.split_at(end))
        .filter(|(digits, _rest)| read_before.is_none_or(|before| before == *digits))
        .collect()
}

/// How the files of an earlier split are named, next to the names a new
/// split gives. The person is told when the two differ, since the new split
/// then replaces none of those files and removes all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarlierNaming {
    AsScenesAreNamedNow,
    AnotherWay,
}

/// The scene files one earlier split of a video left in a folder, going by
/// their names: all named by the same pattern, with numbers of the same
/// length, and all saying the same count where the pattern names it.
///
/// A new split may replace or remove what is in here and nothing else, so
/// this cannot be made from a list of files. It comes from
/// `earlier_splits_among` alone, which reads every name in full.
#[derive(Debug, Clone, PartialEq)]
pub struct EarlierSplit {
    naming: EarlierNaming,
    scene_count: Option<usize>,
    /// In name order, each once, and never empty.
    files: Vec<PathBuf>,
}

impl EarlierSplit {
    pub fn naming(&self) -> EarlierNaming {
        self.naming
    }

    /// How many scenes the split wrote, where its file names say so. This
    /// can be more than the files that are still there.
    #[cfg(test)]
    pub fn scene_count(&self) -> Option<usize> {
        self.scene_count
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// The names of the files without their folder, in name order.
    pub fn file_names(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|file| match file.file_name() {
                Some(name) => name.to_string_lossy().into_owned(),
                None => file.display().to_string(),
            })
            .collect()
    }
}

/// Which earlier split a scene file belongs to: files with the same key
/// were written by one split, as far as names can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EarlierSplitKey {
    /// The place of the naming among `SceneNames::schemes`.
    scheme: usize,
    count: Option<usize>,
    number_digits: usize,
}

/// The place of the naming in use among `SceneNames::schemes`.
const SCHEME_IN_USE: usize = 0;

impl EarlierSplitKey {
    fn naming(&self) -> EarlierNaming {
        if self.scheme == SCHEME_IN_USE {
            EarlierNaming::AsScenesAreNamedNow
        } else {
            EarlierNaming::AnotherWay
        }
    }

    /// Whether files with this key and these scene numbers could be what
    /// one split left. There has to be a file at all. And a number with
    /// more than three digits and no count beside it says too little alone:
    /// it counts only beside the first scene of its split.
    ///
    /// Where `numbers_needed` says so, the numbers also have to run from
    /// one without a gap.
    fn could_be_one_split(
        &self,
        numbers: impl Iterator<Item = usize>,
        numbers_needed: NumbersNeeded,
    ) -> bool {
        let numbers: BTreeSet<usize> = numbers.collect();
        let Some(&highest) = numbers.last() else {
            return false;
        };
        let runs_from_one_without_gaps =
            numbers.first() == Some(&FIRST_SCENE_NUMBER) && highest == numbers.len();
        if numbers_needed == NumbersNeeded::FromOneWithoutGaps && !runs_from_one_without_gaps {
            return false;
        }
        let says_enough_alone = self.count.is_some() || self.number_digits == MIN_NUMBER_DIGITS;
        says_enough_alone || numbers.contains(&FIRST_SCENE_NUMBER)
    }
}

/// How much the scene numbers of an earlier split have to show before its
/// files count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumbersNeeded {
    AnyOfOneSplit,
    /// Every number from one up to the highest. Asked of a pattern that is
    /// no longer in use and says little: no count, and not the "_scene_" of
    /// the old default. The person does not see such a pattern anywhere any
    /// more, and "{name}_{number}" would otherwise go on taking "IMG_007"
    /// beside "IMG" for a scene. A whole run of numbers is what a split
    /// leaves, and seldom what other videos are called.
    FromOneWithoutGaps,
}

impl NumbersNeeded {
    /// What is asked of files named by the scheme at `place` among those of
    /// `SceneNames::schemes`.
    fn of_scheme(place: usize, scheme: &NameScheme) -> Self {
        let says_little = !scheme.pattern.names_the_count()
            && scheme.pattern != SceneNamePattern::default_before_the_count();
        if place != SCHEME_IN_USE && says_little {
            Self::FromOneWithoutGaps
        } else {
            Self::AnyOfOneSplit
        }
    }
}

/// Those of `files` that earlier splits of `video` wrote, going by their
/// names, one `EarlierSplit` for each split. The splits named as scenes
/// are named now come first.
///
/// Whatever is returned here a new split replaces or removes, so:
///
/// - The video's name has to stand where `{name}` is, letter for letter,
///   and the file has to end like the video. Capitals count where the
///   system tells files apart by them.
/// - Nothing may be left over before, between or after the parts of the
///   pattern. "holiday_scene_1_of_50_extended" is another video.
/// - The number and the count have to be ones a split writes together (see
///   `SceneFileReading::of_written`).
/// - The video itself is never among them.
/// - A number with more than three digits and no count beside it is only
///   written by a split into a thousand scenes or more, and such a split
///   also wrote its first scene with that many digits. Without that first
///   scene among `files`, a longer number is taken for part of another
///   video's name: "Film 2049" is not scene 2049 of "Film".
///
/// A file is read by the first naming that fits it, the one in use first,
/// and is in one split only, so nothing is moved away twice.
pub fn earlier_splits_among(
    video: &Path,
    names: SceneNames,
    files: Vec<PathBuf>,
) -> Result<Vec<EarlierSplit>, NamingError> {
    earlier_splits_among_spelled(video, names, files, SYSTEM_PATH_CASE)
}

fn earlier_splits_among_spelled(
    video: &Path,
    names: SceneNames,
    files: Vec<PathBuf>,
    case: PathCase,
) -> Result<Vec<EarlierSplit>, NamingError> {
    let video = VideoFileName::of(video)?;
    let schemes = names.schemes();
    let mut numbered_files: BTreeMap<EarlierSplitKey, Vec<(usize, PathBuf)>> = BTreeMap::new();
    for file in files {
        let Some(stem) = video.stem_of_a_possible_scene(&file, case) else {
            continue;
        };
        let readings = schemes.iter().enumerate().find_map(|(place, scheme)| {
            let reader = NameReader {
                video_name: VideoName::Known(&video.name),
                number_digits: scheme.number_digits,
                case,
            };
            let readings = reader.readings_of(stem, &scheme.pattern.pieces());
            (!readings.is_empty()).then_some((place, readings))
        });
        let Some((scheme, readings)) = readings else {
            continue;
        };
        for reading in readings {
            let key = EarlierSplitKey {
                scheme,
                count: reading.count,
                number_digits: reading.number_digits,
            };
            numbered_files
                .entry(key)
                .or_default()
                .push((reading.number, file.clone()));
        }
    }

    // A name that can be read two ways would put its file into two splits.
    // Each file belongs to one: the first, in the order of the keys, that
    // holds up without the files other splits already have.
    let mut files_in_a_split: BTreeSet<PathBuf> = BTreeSet::new();
    let mut splits = Vec::new();
    for (key, numbered) in numbered_files {
        let numbered: BTreeSet<(usize, PathBuf)> = numbered
            .into_iter()
            .filter(|(_number, file)| !files_in_a_split.contains(file))
            .collect();
        let numbers = numbered.iter().map(|(number, _file)| *number);
        let numbers_needed = NumbersNeeded::of_scheme(key.scheme, &schemes[key.scheme]);
        if !key.could_be_one_split(numbers, numbers_needed) {
            continue;
        }
        let files: BTreeSet<PathBuf> = numbered.into_iter().map(|(_number, file)| file).collect();
        files_in_a_split.extend(files.iter().cloned());
        splits.push(EarlierSplit {
            naming: key.naming(),
            scene_count: key.count,
            files: files.into_iter().collect(),
        });
    }
    Ok(splits)
}

/// The file name of a video, in the two parts every scene name is made of.
#[derive(Debug, Clone, PartialEq)]
struct VideoFileName {
    /// Without the extension.
    name: String,
    extension: String,
}

impl VideoFileName {
    fn of(video: &Path) -> Result<Self, NamingError> {
        let name = video
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| NamingError::NoUsableVideoName(video.display().to_string()))?;
        let extension = video
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| NamingError::NoExtension(video.display().to_string()))?;
        Ok(Self {
            name: name.to_owned(),
            extension: extension.to_owned(),
        })
    }

    fn file_name(&self) -> String {
        format!("{}.{}", self.name, self.extension)
    }

    /// The name of `file` without its extension, when the file could be a
    /// scene of this video at all: it ends like the video, and it is not
    /// the video itself.
    fn stem_of_a_possible_scene<'file>(
        &self,
        file: &'file Path,
        case: PathCase,
    ) -> Option<&'file str> {
        let file_name = file.file_name()?.to_str()?;
        if is_same_file_name(file_name, &self.file_name(), case) {
            return None;
        }
        let extension = file.extension()?.to_str()?;
        if !is_same_file_name(extension, &self.extension, case) {
            return None;
        }
        file.file_stem()?.to_str()
    }
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
    video: VideoFileName,
}

impl SceneNaming {
    pub fn of(video: &Path, pattern: SceneNamePattern) -> Result<Self, NamingError> {
        Ok(Self {
            pattern,
            video: VideoFileName::of(video)?,
        })
    }

    /// The names of the files a split writes for `scenes`, in scene order.
    /// These are the scenes that are written, numbered from one without
    /// gaps: a scene that is dropped is not among them, and not counted.
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
        let digits = self.pattern.number_digits().in_a_split_into(count);
        let mut file_name = String::new();
        for piece in self.pattern.pieces() {
            match piece {
                Piece::Text(text) => file_name.push_str(text),
                Piece::Stands(Placeholder::Name) => file_name.push_str(&self.video.name),
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
        file_name.push_str(&self.video.extension);
        file_name
    }

    /// The video's own file name, as the naming read it.
    pub fn video_file_name(&self) -> String {
        self.video.file_name()
    }

    pub fn extension(&self) -> &str {
        &self.video.extension
    }
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

/// The earlier splits of `video` among files with these names, as someone
/// finds them who names scenes by default and never chose another name.
/// For tests elsewhere that need an `EarlierSplit` to show or to act on.
#[cfg(test)]
pub fn earlier_splits_for_test(video: &str, file_names: &[&str]) -> Vec<EarlierSplit> {
    let files = file_names.iter().map(PathBuf::from).collect();
    let names = SceneNames::from(SceneNamePattern::default());
    earlier_splits_among_spelled(Path::new(video), names, files, PathCase::Matters)
        .expect("the video of a test has a name and an extension")
}

/// The names the default pattern gives a split of `video` into `count`
/// scenes, for tests elsewhere.
#[cfg(test)]
pub fn default_names_for_test(video: &str, count: usize) -> Vec<String> {
    let scenes: Vec<TimeSpan> = (0..count)
        .map(|scene| TimeSpan {
            start: scene as f64,
            end: (scene + 1) as f64,
        })
        .collect();
    SceneNaming::of(Path::new(video), SceneNamePattern::default())
        .and_then(|naming| naming.file_names(&scenes))
        .expect("the video of a test can be named")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_DEFAULT: &str = DEFAULT_PATTERN_BEFORE_THE_COUNT;

    fn pattern(typed: &str) -> SceneNamePattern {
        SceneNamePattern::new(typed).unwrap()
    }

    fn naming(video: &str, typed: &str) -> SceneNaming {
        SceneNaming::of(Path::new(video), pattern(typed)).unwrap()
    }

    /// The names of someone who names scenes by `in_use` now and wrote
    /// splits under each of `used_before`, the newest first.
    fn names(in_use: &str, used_before: &[&str]) -> SceneNames {
        let mut remembered = PatternsUsed::default();
        for typed in used_before.iter().rev() {
            remembered.remember(pattern(typed));
        }
        SceneNames::new(pattern(in_use), remembered)
    }

    fn splits_among(
        video: &str,
        names: SceneNames,
        file_names: &[&str],
        case: PathCase,
    ) -> Vec<EarlierSplit> {
        let files = file_names.iter().map(PathBuf::from).collect();
        earlier_splits_among_spelled(Path::new(video), names, files, case).unwrap()
    }

    /// Every file an earlier split of `video` wrote, by any naming known to
    /// someone who names scenes by `in_use`, with capitals counting.
    fn earlier_scenes(video: &str, in_use: &str, file_names: &[&str]) -> Vec<String> {
        let mut found: Vec<String> =
            splits_among(video, names(in_use, &[]), file_names, PathCase::Matters)
                .iter()
                .flat_map(EarlierSplit::file_names)
                .collect();
        found.sort();
        found.dedup();
        found
    }

    fn is_named_as_a_scene(video: &str, in_use: &str, file_name: &str) -> bool {
        !earlier_scenes(video, in_use, &[file_name]).is_empty()
    }

    /// What one earlier split is, for comparing: its naming, its count and
    /// its files.
    fn described(splits: &[EarlierSplit]) -> Vec<(EarlierNaming, Option<usize>, Vec<String>)> {
        splits
            .iter()
            .map(|split| (split.naming(), split.scene_count(), split.file_names()))
            .collect()
    }

    fn owned(file_names: &[&str]) -> Vec<String> {
        file_names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn scenes_of_a_minute(count: usize) -> Vec<TimeSpan> {
        (0..count)
            .map(|scene| TimeSpan {
                start: scene as f64 * 60.0,
                end: (scene + 1) as f64 * 60.0,
            })
            .collect()
    }

    const AS_NOW: EarlierNaming = EarlierNaming::AsScenesAreNamedNow;
    const ANOTHER_WAY: EarlierNaming = EarlierNaming::AnotherWay;

    #[test]
    fn the_default_pattern_says_how_many_scenes_there_are() {
        let naming =
            SceneNaming::of(Path::new("videos/holiday.mp4"), SceneNamePattern::default()).unwrap();
        assert_eq!(
            naming.file_names(&scenes_of_a_minute(2)).unwrap(),
            ["holiday_scene_1_of_2.mp4", "holiday_scene_2_of_2.mp4"]
        );
        let of_five = naming.file_names(&scenes_of_a_minute(5)).unwrap();
        assert_eq!(of_five[0], "holiday_scene_1_of_5.mp4");
        assert_eq!(of_five[4], "holiday_scene_5_of_5.mp4");
    }

    #[test]
    fn a_number_beside_its_count_has_as_many_digits_as_the_count() {
        let naming = naming("holiday.mp4", DEFAULT_PATTERN);
        let of_twelve = naming.file_names(&scenes_of_a_minute(12)).unwrap();
        assert_eq!(of_twelve[6], "holiday_scene_07_of_12.mp4");
        assert_eq!(of_twelve[11], "holiday_scene_12_of_12.mp4");

        let of_nine = naming.file_names(&scenes_of_a_minute(9)).unwrap();
        assert_eq!(of_nine[8], "holiday_scene_9_of_9.mp4");
        let of_ten = naming.file_names(&scenes_of_a_minute(10)).unwrap();
        assert_eq!(of_ten[0], "holiday_scene_01_of_10.mp4");
        let of_a_hundred = naming.file_names(&scenes_of_a_minute(100)).unwrap();
        assert_eq!(of_a_hundred[0], "holiday_scene_001_of_100.mp4");
        let of_a_thousand = naming.file_names(&scenes_of_a_minute(1000)).unwrap();
        assert_eq!(of_a_thousand[0], "holiday_scene_0001_of_1000.mp4");
    }

    #[test]
    fn the_files_of_one_split_sort_in_scene_order() {
        for typed in [DEFAULT_PATTERN, OLD_DEFAULT, "{number} {name}"] {
            for count in [9, 10, 12, 100, 1200] {
                let written = naming("holiday.mp4", typed)
                    .file_names(&scenes_of_a_minute(count))
                    .unwrap();
                let mut sorted = written.clone();
                sorted.sort();
                assert_eq!(written, sorted, "{count} scenes named {typed}");
            }
        }
    }

    #[test]
    fn a_number_without_its_count_keeps_three_digits() {
        let names = naming("holiday.mp4", "{name} part {number}")
            .file_names(&scenes_of_a_minute(2))
            .unwrap();
        assert_eq!(names, ["holiday part 001.mp4", "holiday part 002.mp4"]);
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
                "film 1 of 2 at 00h00m00s for 00h03m12s.mkv",
                "film 2 of 2 at 00h03m12s for 01h00m07s.mkv",
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
        let names = naming("100% fun.v2.mkv", DEFAULT_PATTERN)
            .file_names(&scenes_of_a_minute(1))
            .unwrap();
        assert_eq!(names, ["100% fun.v2_scene_1_of_1.mkv"]);
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
        // The count is the same for every scene of a split.
        assert_eq!(
            SceneNamePattern::new("{name} of {count}"),
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
        assert_eq!(
            PatternError::Empty.to_string(),
            "Type a name, such as {name}_scene_{number}_of_{count}."
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
            "My video_scene_02_of_12.mp4"
        );
        assert_eq!(
            pattern("{name} ({number} of {count}) {start}").example(),
            "My video (02 of 12) 00h03m12s.mp4"
        );
        assert_eq!(pattern(OLD_DEFAULT).example(), "My video_scene_002.mp4");
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
    fn a_scene_file_name_that_is_too_long_is_refused_when_naming() {
        let long_name = format!("{}.mp4", "a".repeat(250));
        let error = naming(&long_name, DEFAULT_PATTERN)
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
        let names = SceneNames::from(SceneNamePattern::default());
        assert!(matches!(
            earlier_splits_among(Path::new("holiday"), names, Vec::new()),
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

    // What a split wrote is found again.

    #[test]
    fn the_scenes_of_a_split_named_by_default_are_found_as_one_split() {
        for count in [1, 5, 9, 10, 12, 100, 1200] {
            let written = default_names_for_test("holiday.mp4", count);
            let mut in_the_folder: Vec<&str> = written.iter().map(String::as_str).collect();
            in_the_folder.push("holiday.mp4");

            let found = splits_among(
                "holiday.mp4",
                names(DEFAULT_PATTERN, &[]),
                &in_the_folder,
                PathCase::Matters,
            );
            let mut in_name_order = written.clone();
            in_name_order.sort();
            assert_eq!(
                described(&found),
                [(AS_NOW, Some(count), in_name_order)],
                "a split into {count}"
            );
        }
    }

    #[test]
    fn every_name_a_pattern_writes_is_found_again_and_the_video_never_is() {
        let patterns = [
            DEFAULT_PATTERN,
            OLD_DEFAULT,
            "{number} {name}",
            "{name} {number} of {count}",
            "{name} {start} {number} ({length})",
            "{number}{count}{name}",
            "{count}{number}{name}",
            "{name}{number}",
            "{number}{start}{name}",
            "{name} {number} {name} {count} {count}",
        ];
        for typed in patterns {
            for count in [1, 2, 9, 10, 12, 99, 100, 999, 1000, 1200] {
                let video = "My film 2.mp4";
                let written = naming(video, typed)
                    .file_names(&scenes_of_a_minute(count))
                    .unwrap();
                let mut in_the_folder: Vec<&str> = written.iter().map(String::as_str).collect();
                in_the_folder.push(video);

                let found =
                    splits_among(video, names(typed, &[]), &in_the_folder, PathCase::Matters);
                let mut found_as_named_now: Vec<String> = found
                    .iter()
                    .filter(|split| split.naming() == AS_NOW)
                    .flat_map(EarlierSplit::file_names)
                    .collect();
                found_as_named_now.sort();
                found_as_named_now.dedup();
                let mut in_name_order = written.clone();
                in_name_order.sort();
                assert_eq!(
                    found_as_named_now, in_name_order,
                    "{count} scenes named {typed}"
                );
                let every_file_found: Vec<String> =
                    found.iter().flat_map(EarlierSplit::file_names).collect();
                assert!(!every_file_found.contains(&video.to_owned()));
                // No file is in two splits, or it would be moved away twice.
                assert_eq!(
                    every_file_found.len(),
                    written.len(),
                    "{count} scenes named {typed}"
                );
            }
        }
    }

    // Earlier splits under another naming.

    #[test]
    fn scenes_named_by_the_old_default_are_found_whatever_name_is_in_use() {
        let in_the_folder = [
            "holiday.mp4",
            "holiday_scene_001.mp4",
            "holiday_scene_002.mp4",
            "holiday_scene_003.mp4",
        ];
        for in_use in [DEFAULT_PATTERN, "{name} part {number}", "{number} {name}"] {
            let found = splits_among(
                "holiday.mp4",
                names(in_use, &[]),
                &in_the_folder,
                PathCase::Matters,
            );
            assert_eq!(
                described(&found),
                [(ANOTHER_WAY, None, owned(&in_the_folder[1..]))],
                "with {in_use} in use"
            );
        }
    }

    #[test]
    fn scenes_named_by_the_new_default_are_found_under_any_name_in_use() {
        let found = splits_among(
            "holiday.mp4",
            names("{name} part {number}", &[]),
            &["holiday_scene_1_of_2.mp4", "holiday_scene_2_of_2.mp4"],
            PathCase::Matters,
        );
        assert_eq!(
            described(&found),
            [(
                ANOTHER_WAY,
                Some(2),
                owned(&["holiday_scene_1_of_2.mp4", "holiday_scene_2_of_2.mp4"])
            )]
        );
    }

    #[test]
    fn scenes_named_by_a_pattern_used_before_are_found_only_when_it_is_remembered() {
        let in_the_folder = ["holiday part 001.mp4", "holiday part 002.mp4"];

        let forgotten = splits_among(
            "holiday.mp4",
            names(DEFAULT_PATTERN, &[]),
            &in_the_folder,
            PathCase::Matters,
        );
        assert_eq!(forgotten, []);

        let remembered = splits_among(
            "holiday.mp4",
            names(
                DEFAULT_PATTERN,
                &["{number} {name}", "{name} part {number}"],
            ),
            &in_the_folder,
            PathCase::Matters,
        );
        assert_eq!(
            described(&remembered),
            [(ANOTHER_WAY, None, owned(&in_the_folder))]
        );
    }

    #[test]
    fn a_pattern_no_longer_in_use_that_says_little_needs_every_number_from_one() {
        let remembered = names(DEFAULT_PATTERN, &["{name}_{number}"]);
        let found = |file_names: &[&str]| {
            described(&splits_among(
                "IMG.mp4",
                remembered,
                file_names,
                PathCase::Matters,
            ))
        };
        // Other videos of a camera, beside one called "IMG".
        assert_eq!(found(&["IMG_007.mp4"]), []);
        assert_eq!(found(&["IMG_001.mp4", "IMG_003.mp4"]), []);
        assert_eq!(found(&["IMG_002.mp4", "IMG_003.mp4"]), []);
        // What a split leaves.
        let whole_split = ["IMG_001.mp4", "IMG_002.mp4", "IMG_003.mp4"];
        assert_eq!(
            found(&whole_split),
            [(ANOTHER_WAY, None, owned(&whole_split))]
        );

        // While the pattern is in use, the person sees it, and the scenes
        // left of a split count as before.
        let in_use = splits_among(
            "IMG.mp4",
            names("{name}_{number}", &[]),
            &["IMG_001.mp4", "IMG_003.mp4"],
            PathCase::Matters,
        );
        assert_eq!(in_use.len(), 1);
        // The old default says "_scene_", which is enough.
        let old_default = splits_among(
            "IMG.mp4",
            remembered,
            &["IMG_scene_003.mp4"],
            PathCase::Matters,
        );
        assert_eq!(old_default.len(), 1);
    }

    #[test]
    fn scenes_written_when_every_number_had_three_digits_are_another_naming() {
        // The same pattern wrote "001 of 5" once and writes "1 of 5" now.
        let found = splits_among(
            "film.mp4",
            names("{name} {number} of {count}", &[]),
            &[
                "film 001 of 5.mp4",
                "film 005 of 5.mp4",
                "film 1 of 5.mp4",
                "film 2 of 5.mp4",
            ],
            PathCase::Matters,
        );
        assert_eq!(
            described(&found),
            [
                (
                    AS_NOW,
                    Some(5),
                    owned(&["film 1 of 5.mp4", "film 2 of 5.mp4"])
                ),
                (
                    ANOTHER_WAY,
                    Some(5),
                    owned(&["film 001 of 5.mp4", "film 005 of 5.mp4"])
                ),
            ]
        );
    }

    #[test]
    fn a_name_two_namings_give_alike_is_read_as_named_now() {
        // In a split into a hundred, three digits are what the count has.
        let found = splits_among(
            "film.mp4",
            names(DEFAULT_PATTERN, &[]),
            &["film_scene_001_of_100.mp4", "film_scene_100_of_100.mp4"],
            PathCase::Matters,
        );
        assert_eq!(
            described(&found),
            [(
                AS_NOW,
                Some(100),
                owned(&["film_scene_001_of_100.mp4", "film_scene_100_of_100.mp4"])
            )]
        );
    }

    #[test]
    fn each_earlier_split_is_found_apart_from_the_others() {
        let found = splits_among(
            "holiday.mp4",
            names(DEFAULT_PATTERN, &["{name} part {number}"]),
            &[
                "holiday.mp4",
                "holiday_scene_1_of_5.mp4",
                "holiday_scene_2_of_5.mp4",
                "holiday_scene_1_of_4.mp4",
                "holiday_scene_04_of_12.mp4",
                "holiday_scene_001.mp4",
                "holiday_scene_002.mp4",
                "holiday part 001.mp4",
                "notes.txt",
            ],
            PathCase::Matters,
        );
        assert_eq!(
            described(&found),
            [
                (AS_NOW, Some(4), owned(&["holiday_scene_1_of_4.mp4"])),
                (
                    AS_NOW,
                    Some(5),
                    owned(&["holiday_scene_1_of_5.mp4", "holiday_scene_2_of_5.mp4"])
                ),
                (AS_NOW, Some(12), owned(&["holiday_scene_04_of_12.mp4"])),
                (ANOTHER_WAY, None, owned(&["holiday part 001.mp4"])),
                (
                    ANOTHER_WAY,
                    None,
                    owned(&["holiday_scene_001.mp4", "holiday_scene_002.mp4"])
                ),
            ]
        );
    }

    // Files that only look like scenes.

    #[test]
    fn names_that_only_resemble_the_default_are_not_scenes_of_this_video() {
        let others = [
            // The video itself.
            "holiday.mp4",
            // Other videos whose names begin or end alike.
            "holiday_scene_1_of_50_extended.mp4",
            "holiday_scene_1_of_5_copy.mp4",
            "holiday_scene_1_of_5 (1).mp4",
            "holiday_scene_1_of_5 .mp4",
            "holiday2_scene_1_of_5.mp4",
            "my holiday_scene_1_of_5.mp4",
            " holiday_scene_1_of_5.mp4",
            "holiday_scene_1_of_5",
            // Another kind of file.
            "holiday_scene_1_of_5.mkv",
            "holiday_scene_1_of_5.mp4.part",
            "holiday_scene_1_of_5.mp4x",
            // No split numbers a scene 0, or writes a count of 0.
            "holiday_scene_0_of_5.mp4",
            "holiday_scene_1_of_0.mp4",
            "holiday_scene_0_of_0.mp4",
            "holiday_scene_000_of_5.mp4",
            // No split has a scene beyond its count.
            "holiday_scene_6_of_5.mp4",
            "holiday_scene_13_of_12.mp4",
            "holiday_scene_006_of_5.mp4",
            // A number with other digits than its split gives it.
            "holiday_scene_01_of_5.mp4",
            "holiday_scene_1_of_12.mp4",
            "holiday_scene_0001_of_12.mp4",
            "holiday_scene_01_of_100.mp4",
            "holiday_scene_001_of_1000.mp4",
            // A count is written without leading zeros.
            "holiday_scene_1_of_05.mp4",
            "holiday_scene_01_of_012.mp4",
            // Not numbers.
            "holiday_scene__of_5.mp4",
            "holiday_scene_1_of_.mp4",
            "holiday_scene_one_of_5.mp4",
            "holiday_scene_+1_of_5.mp4",
            "holiday_scene_-1_of_5.mp4",
            "holiday_scene_1.0_of_5.mp4",
            "holiday_scene_1_of_5e0.mp4",
            "holiday_scene_\u{661}_of_5.mp4",
            // Too long to be a count at all.
            "holiday_scene_00000000000000000000001_of_99999999999999999999999.mp4",
            // The old default wrote three digits or more, and no scene 0.
            "holiday_scene_1.mp4",
            "holiday_scene_12.mp4",
            "holiday_scene_000.mp4",
            "holiday_scene_notes.mp4",
            "holiday_scene_.mp4",
            "holiday_scene_001_copy.mp4",
            "holiday_scene_001.mkv",
            "holiday2_scene_001.mp4",
            // Capitals, where the system tells files apart by them.
            "Holiday_scene_1_of_5.mp4",
            "holiday_SCENE_1_of_5.mp4",
            "holiday_scene_1_of_5.MP4",
        ];
        for other in others {
            assert!(
                !is_named_as_a_scene("holiday.mp4", DEFAULT_PATTERN, other),
                "{other}"
            );
        }
        assert_eq!(
            earlier_scenes("holiday.mp4", DEFAULT_PATTERN, &others),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_separate_video_named_like_a_scene_and_then_some_is_left_alone() {
        assert_eq!(
            earlier_scenes(
                "holiday.mp4",
                DEFAULT_PATTERN,
                &[
                    "holiday.mp4",
                    "holiday_scene_1_of_5.mp4",
                    "holiday_scene_1_of_50_extended.mp4",
                ]
            ),
            ["holiday_scene_1_of_5.mp4"]
        );
    }

    #[test]
    fn a_video_whose_name_ends_like_a_scene_is_not_a_scene_of_its_neighbour() {
        // "x_scene_2" is a video of its own beside "x".
        assert_eq!(
            earlier_scenes("x.mp4", DEFAULT_PATTERN, &["x.mp4", "x_scene_2.mp4"]),
            Vec::<String>::new()
        );
        // Its scenes are its own, and not those of "x".
        let scenes_of_the_neighbour = ["x_scene_2_scene_001.mp4", "x_scene_2_scene_1_of_3.mp4"];
        assert_eq!(
            earlier_scenes("x_scene_2.mp4", DEFAULT_PATTERN, &scenes_of_the_neighbour),
            scenes_of_the_neighbour
        );
        assert_eq!(
            earlier_scenes("x.mp4", DEFAULT_PATTERN, &scenes_of_the_neighbour),
            Vec::<String>::new()
        );
        // The scenes of a scene are not scenes of the video it came from.
        assert_eq!(
            earlier_scenes(
                "x.mp4",
                DEFAULT_PATTERN,
                &[
                    "x_scene_1_of_2_scene_1_of_3.mp4",
                    "x_scene_001_scene_001.mp4"
                ]
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn signs_that_mean_something_in_a_search_pattern_are_plain_letters_here() {
        let video = "a.b+c (1) [x]$^*?.mp4";
        let written = default_names_for_test(video, 2);
        let written: Vec<&str> = written.iter().map(String::as_str).collect();
        assert_eq!(earlier_scenes(video, DEFAULT_PATTERN, &written), written);

        // A dot stands for a dot only, a plus for a plus, and so on.
        for other in [
            "aXb+c (1) [x]$^*?_scene_1_of_2.mp4",
            "a.bbc (1) [x]$^*?_scene_1_of_2.mp4",
            "a.b+c 1 [x]$^*?_scene_1_of_2.mp4",
            "a.b+c (1) x$^*?_scene_1_of_2.mp4",
            "a.b+c (1) [x]_scene_1_of_2.mp4",
            "a.b+c (1) [x]$^*?_scene_1_of_2Xmp4",
        ] {
            assert!(
                !is_named_as_a_scene(video, DEFAULT_PATTERN, other),
                "{other}"
            );
        }
        // Nor does a sign in the pattern itself.
        assert!(is_named_as_a_scene(
            "film.mp4",
            "{name} (.+) [{number}]",
            "film (.+) [001].mp4"
        ));
        assert!(!is_named_as_a_scene(
            "film.mp4",
            "{name} (.+) [{number}]",
            "film (abc) [001].mp4"
        ));
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
            ("film.mp4", "{name} {number} of {count}", "film 2 of 1.mp4"),
            ("film.mp4", "{name} {number}", "film 000.mp4"),
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
        let found = splits_among(
            "film.mp4",
            names("{name}_{number}", &[]),
            &["film_0001.mp4", "film_1000.mp4", "film_10000.mp4"],
            PathCase::Matters,
        );
        assert_eq!(
            described(&found),
            [(AS_NOW, None, owned(&["film_0001.mp4", "film_1000.mp4"]))]
        );
    }

    #[test]
    fn scenes_with_three_digits_and_scenes_with_four_are_two_splits() {
        let found = splits_among(
            "film.mp4",
            names("{name}_{number}", &[]),
            &["film_001.mp4", "film_0001.mp4", "film_0002.mp4"],
            PathCase::Matters,
        );
        assert_eq!(
            described(&found),
            [
                (AS_NOW, None, owned(&["film_001.mp4"])),
                (AS_NOW, None, owned(&["film_0001.mp4", "film_0002.mp4"])),
            ]
        );
    }

    #[test]
    fn a_name_that_can_be_read_two_ways_puts_its_file_into_one_split_only() {
        // Scene 100 of 1200 starts at 01h39m00s, and its name reads as
        // scene "0100" at "01h39m00s" or as scene "010" at "001h39m00s".
        let typed = "{number}{start}{name}";
        let ambiguous = "010001h39m00sfilm.mp4";
        let reader = NameReader {
            video_name: VideoName::Known("film"),
            number_digits: NumberDigits::AtLeastThree,
            case: PathCase::Matters,
        };
        let readings = reader.readings_of("010001h39m00sfilm", &pattern(typed).pieces());
        assert_eq!(readings.len(), 2, "{readings:?}");

        let found = splits_among(
            "film.mp4",
            names(typed, &[]),
            &["000100h00m00sfilm.mp4", ambiguous],
            PathCase::Matters,
        );
        let every_file: Vec<String> = found.iter().flat_map(EarlierSplit::file_names).collect();
        assert_eq!(
            every_file.iter().filter(|name| *name == ambiguous).count(),
            1,
            "{found:?}"
        );
        assert_eq!(every_file.len(), 2);
    }

    #[test]
    fn a_pattern_that_names_a_number_twice_wants_the_same_number_twice() {
        let twice = "{number} {name} {number}";
        assert!(is_named_as_a_scene("film.mp4", twice, "007 film 007.mp4"));
        assert!(!is_named_as_a_scene("film.mp4", twice, "007 film 008.mp4"));

        let count_twice = "{name} {number} of {count} ({count})";
        assert!(is_named_as_a_scene(
            "film.mp4",
            count_twice,
            "film 1 of 2 (2).mp4"
        ));
        assert!(!is_named_as_a_scene(
            "film.mp4",
            count_twice,
            "film 1 of 2 (3).mp4"
        ));
    }

    #[test]
    fn a_count_with_a_leading_zero_was_not_written_by_a_split() {
        let typed = "{name} {number} of {count}";
        assert!(is_named_as_a_scene("film.mp4", typed, "film 01 of 12.mp4"));
        assert!(!is_named_as_a_scene(
            "film.mp4",
            typed,
            "film 01 of 012.mp4"
        ));
        assert!(!is_named_as_a_scene(
            "film.mp4",
            typed,
            "film 001 of 012.mp4"
        ));
    }

    #[test]
    fn what_a_name_says_has_to_be_what_a_split_writes() {
        let of_written = SceneFileReading::of_written;
        let as_the_count = NumberDigits::AsTheCountHas;
        let three_at_least = NumberDigits::AtLeastThree;

        assert_eq!(
            of_written("07", Some("12"), as_the_count),
            Some(SceneFileReading {
                number: 7,
                number_digits: 2,
                count: Some(12)
            })
        );
        assert_eq!(of_written("7", Some("12"), as_the_count), None);
        assert_eq!(of_written("007", Some("12"), as_the_count), None);
        assert_eq!(of_written("13", Some("12"), as_the_count), None);
        assert_eq!(of_written("00", Some("12"), as_the_count), None);
        assert_eq!(of_written("0", Some("0"), as_the_count), None);
        assert_eq!(
            of_written("1", Some("1"), as_the_count).map(|read| read.number),
            Some(1)
        );

        // What the same patterns wrote before the count set the digits.
        assert!(of_written("007", Some("12"), three_at_least).is_some());
        assert_eq!(of_written("07", Some("12"), three_at_least), None);
        assert!(of_written("0007", Some("1200"), three_at_least).is_some());
        assert_eq!(of_written("007", Some("1200"), three_at_least), None);

        // Without a count, three digits at least, and never scene 0.
        assert!(of_written("007", None, three_at_least).is_some());
        assert!(of_written("1200", None, three_at_least).is_some());
        assert_eq!(of_written("07", None, three_at_least), None);
        assert_eq!(of_written("000", None, three_at_least), None);
        let too_long_to_be_a_number = "9".repeat(40);
        assert_eq!(
            of_written(&too_long_to_be_a_number, None, three_at_least),
            None
        );
    }

    #[test]
    fn capitals_tell_scene_files_apart_only_where_the_system_does() {
        let names = names("{name} Part {number}", &[]);
        let retyped = ["holiday part 001.mp4"];
        assert_eq!(
            splits_among("Holiday.mp4", names, &retyped, PathCase::Matters),
            []
        );
        assert_eq!(
            described(&splits_among(
                "Holiday.mp4",
                names,
                &retyped,
                PathCase::Ignored
            )),
            [(AS_NOW, None, owned(&retyped))]
        );
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
    fn where_capitals_do_not_count_the_ending_and_the_video_are_read_without_them() {
        let default_names = names(DEFAULT_PATTERN, &[]);
        let in_the_folder = ["HOLIDAY.MP4", "Holiday_Scene_1_of_2.MP4"];
        assert_eq!(
            described(&splits_among(
                "holiday.mp4",
                default_names,
                &in_the_folder,
                PathCase::Ignored
            )),
            [(AS_NOW, Some(2), owned(&["Holiday_Scene_1_of_2.MP4"]))]
        );
        assert_eq!(
            splits_among(
                "holiday.mp4",
                default_names,
                &in_the_folder,
                PathCase::Matters
            ),
            []
        );
    }

    #[test]
    fn a_scene_keeps_its_folder_and_is_found_once() {
        let files = vec![
            PathBuf::from("out").join("holiday_scene_1_of_2.mp4"),
            PathBuf::from("out").join("holiday_scene_1_of_2.mp4"),
        ];
        let found = earlier_splits_among_spelled(
            Path::new("videos/holiday.mp4"),
            names(DEFAULT_PATTERN, &[]),
            files,
            PathCase::Matters,
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].files(),
            [PathBuf::from("out").join("holiday_scene_1_of_2.mp4")]
        );
    }

    // The namings that are known.

    #[test]
    fn the_naming_in_use_comes_first_and_each_naming_is_known_once() {
        let schemes = names(DEFAULT_PATTERN, &[OLD_DEFAULT, DEFAULT_PATTERN]).schemes();
        assert_eq!(
            schemes,
            [
                NameScheme::of(pattern(DEFAULT_PATTERN)),
                NameScheme::with_three_digits_at_least(pattern(DEFAULT_PATTERN)),
                NameScheme::of(pattern(OLD_DEFAULT)),
            ]
        );
        assert_eq!(schemes[SCHEME_IN_USE].pattern, pattern(DEFAULT_PATTERN));

        let with_a_name_chosen = names("{number} {name}", &[]).schemes();
        assert_eq!(
            with_a_name_chosen[SCHEME_IN_USE].pattern,
            pattern("{number} {name}")
        );
        assert_eq!(with_a_name_chosen.len(), 4);
    }

    #[test]
    fn patterns_are_remembered_newest_first_and_each_once() {
        let mut used = PatternsUsed::default();
        assert_eq!(used.newest(), None);
        used.remember(pattern("{name} a {number}"));
        used.remember(pattern("{name} b {number}"));
        used.remember(pattern("{name} a {number}"));
        assert_eq!(
            used.iter().collect::<Vec<_>>(),
            [pattern("{name} a {number}"), pattern("{name} b {number}")]
        );
        assert_eq!(used.newest(), Some(pattern("{name} a {number}")));
    }

    #[test]
    fn the_oldest_pattern_is_forgotten_when_there_are_too_many() {
        let mut used = PatternsUsed::default();
        for position in 0..MOST_PATTERNS_REMEMBERED + 3 {
            used.remember(pattern(&format!("{{name}} {position} {{number}}")));
        }
        let remembered: Vec<SceneNamePattern> = used.iter().collect();
        assert_eq!(remembered.len(), MOST_PATTERNS_REMEMBERED);
        let newest = MOST_PATTERNS_REMEMBERED + 2;
        assert_eq!(
            remembered[0],
            pattern(&format!("{{name}} {newest} {{number}}"))
        );
        assert!(!remembered.contains(&pattern("{name} 0 {number}")));
        assert!(!remembered.contains(&pattern("{name} 2 {number}")));
        assert!(remembered.contains(&pattern("{name} 3 {number}")));
    }

    #[test]
    fn remembered_patterns_are_saved_as_a_list_and_come_back() {
        let mut used = PatternsUsed::default();
        used.remember(pattern("{name} a {number}"));
        used.remember(pattern("{name} b {number}"));
        let saved = serde_json::to_string(&used).unwrap();
        assert_eq!(saved, r#"["{name} b {number}","{name} a {number}"]"#);
        assert_eq!(serde_json::from_str::<PatternsUsed>(&saved).unwrap(), used);
        assert_eq!(
            serde_json::from_str::<PatternsUsed>("[]").unwrap(),
            PatternsUsed::default()
        );
    }

    #[test]
    fn a_saved_list_is_brought_into_shape_when_it_is_loaded() {
        let twice: PatternsUsed = serde_json::from_str(
            r#"["{name} b {number}", "{name} a {number}", "{name} b {number}"]"#,
        )
        .unwrap();
        assert_eq!(
            twice.iter().collect::<Vec<_>>(),
            [pattern("{name} b {number}"), pattern("{name} a {number}")]
        );

        let too_many: Vec<String> = (0..MOST_PATTERNS_REMEMBERED + 2)
            .map(|position| format!("{{name}} {position} {{number}}"))
            .collect();
        let loaded: PatternsUsed =
            serde_json::from_str(&serde_json::to_string(&too_many).unwrap()).unwrap();
        let kept: Vec<String> = loaded.iter().map(|kept| kept.to_string()).collect();
        assert_eq!(kept, too_many[..MOST_PATTERNS_REMEMBERED]);

        // Checked like the pattern in use: none leaves the folder.
        assert!(serde_json::from_str::<PatternsUsed>(r#"["../{name}{number}"]"#).is_err());
    }

    // Scene files of whatever video.

    #[test]
    fn scenes_named_by_either_default_are_recognised_whatever_their_video() {
        let names = names(DEFAULT_PATTERN, &[]);
        for scene_file in [
            "videos/holiday_scene_1_of_5.mp4",
            "holiday_scene_07_of_12.mkv",
            "holiday_scene_0001_of_1200.mp4",
            "a_scene_1_of_1.mp4",
            // The old default, and what the new one wrote with three digits.
            "videos/holiday_scene_001.mp4",
            "holiday_scene_123.mkv",
            "holiday_scene_1000.mp4",
            "holiday_scene_001_of_5.mp4",
            // A scene of a scene.
            "holiday_scene_1_of_5_scene_2_of_2.mp4",
        ] {
            assert!(
                names.is_scene_file_of_some_video(Path::new(scene_file)),
                "{scene_file}"
            );
        }
        for count in [1, 12, 1000] {
            for typed in [DEFAULT_PATTERN, OLD_DEFAULT] {
                let written = naming("holiday.mp4", typed)
                    .file_names(&scenes_of_a_minute(count))
                    .unwrap();
                assert!(written
                    .iter()
                    .all(|name| names.is_scene_file_of_some_video(Path::new(name))));
            }
        }
    }

    #[test]
    fn ordinary_videos_are_not_mistaken_for_scenes() {
        let names = names(DEFAULT_PATTERN, &[]);
        for video in [
            "holiday.mp4",
            "crime_scene_investigation.mp4",
            "_scene_001.mp4",
            "_scene_1_of_5.mp4",
            "holiday_scene_1.mp4",
            "holiday_scene_000.mp4",
            "holiday_scene_6_of_5.mp4",
            "holiday_scene_1_of_12.mp4",
            "holiday_scene_1_of_0.mp4",
            "holiday_scene_1_of_5_extended.mp4",
            "Rocky 002.mp4",
            "Part 1 of 2.mp4",
        ] {
            assert!(
                !names.is_scene_file_of_some_video(Path::new(video)),
                "{video}"
            );
        }
    }

    #[test]
    fn a_chosen_name_marks_scenes_of_any_video_only_when_it_says_the_count() {
        let with_the_count = names("{name} {number} of {count}", &[]);
        assert!(with_the_count.is_scene_file_of_some_video(Path::new("Lecture 1 of 2.mp4")));
        assert!(!with_the_count.is_scene_file_of_some_video(Path::new("Lecture 3 of 2.mp4")));
        assert!(!with_the_count.is_scene_file_of_some_video(Path::new(" 1 of 2.mp4")));

        // Used before, it still marks them.
        let used_before = names(DEFAULT_PATTERN, &["{name} {number} of {count}"]);
        assert!(used_before.is_scene_file_of_some_video(Path::new("Lecture 1 of 2.mp4")));

        // "Rocky 002" could be anyone's video.
        let without_the_count = names("{name} {number}", &[]);
        assert!(!without_the_count.is_scene_file_of_some_video(Path::new("Rocky 002.mp4")));
        assert!(without_the_count.is_scene_file_of_some_video(Path::new("Rocky_scene_002.mp4")));
    }
}
