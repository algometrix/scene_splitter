//! The list of a video's scenes as text another program can read: a table,
//! chapter marks for ffmpeg, or an edit decision list. Nothing here touches
//! a file. The window and the command line save what these functions return.
//!
//! Every format lists the scenes that a split would write, numbered as
//! their files are. A scene the person dropped is in none of them. Times
//! are those of the original video.

use std::fmt;
use std::path::Path;

use crate::fates::ScenesToWrite;
use crate::scene_name::{NamingError, SceneNamePattern, SceneNaming};
use crate::timecode::{format_timestamp, TimeSpan};

const CSV_HEADER: &str = "scene,start,end,length,file";
/// What RFC 4180 asks for between the lines of a table.
const CSV_LINE_END: &str = "\r\n";
const CHAPTER_FILE_HEADER: &str = ";FFMETADATA1";
/// Chapter times are whole milliseconds.
const CHAPTER_TIMEBASE: &str = "TIMEBASE=1/1000";
const MILLISECONDS_PER_SECOND: f64 = 1000.0;
/// These mean something to ffmpeg in a metadata file and are written with
/// a backslash before them.
const CHAPTER_SPECIAL_CHARACTERS: [char; 5] = ['=', ';', '#', '\\', '\n'];
/// The reel name editing programs use for a clip that comes from a file
/// and not from a tape.
const EDL_REEL: &str = "AX";
/// An edit decision list numbers its events with three digits.
const EDL_MOST_EVENTS: usize = 999;
const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;

/// The formats a scene list can be saved in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SceneListFormat {
    /// A table with one row per scene, for a spreadsheet.
    Csv,
    /// Chapter marks in the form ffmpeg reads with `-i chapters.txt
    /// -map_metadata 1`.
    Chapters,
    /// A CMX3600 edit decision list, for an editing program.
    Edl,
}

impl SceneListFormat {
    pub const ALL: [Self; 3] = [Self::Csv, Self::Chapters, Self::Edl];

    /// What the format is called where a person picks one.
    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "Table for a spreadsheet (CSV)",
            Self::Chapters => "Chapter marks for ffmpeg",
            Self::Edl => "Edit list for an editing program (EDL)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Chapters => "txt",
            Self::Edl => "edl",
        }
    }

    /// The format a file of this name is meant to hold, where its
    /// extension says so.
    pub fn for_file(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("ffmetadata") {
            return Some(Self::Chapters);
        }
        Self::ALL
            .into_iter()
            .find(|format| extension.eq_ignore_ascii_case(format.extension()))
    }
}

/// How many pictures the video shows each second. Always a positive number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRate(f64);

/// A frame rate that is zero, negative or not a number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotAFrameRate(pub f64);

impl fmt::Display for NotAFrameRate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} pictures a second is not a frame rate",
            self.0
        )
    }
}

impl FrameRate {
    pub fn per_second(frames_per_second: f64) -> Result<Self, NotAFrameRate> {
        if !frames_per_second.is_finite() || frames_per_second <= 0.0 {
            return Err(NotAFrameRate(frames_per_second));
        }
        Ok(Self(frames_per_second))
    }

    /// The rate of a video that shows each picture for `frame_seconds`.
    pub fn from_frame_seconds(frame_seconds: f64) -> Result<Self, NotAFrameRate> {
        Self::per_second(1.0 / frame_seconds)
    }

    /// The whole number timecodes count with: 30 for 29.97, 24 for 23.976.
    fn timecode_base(self) -> u64 {
        (self.0.round() as u64).max(1)
    }

    /// The frame showing at `seconds`, counted from the start of the video.
    fn frame_at(self, seconds: f64) -> u64 {
        (seconds.max(0.0) * self.0).round() as u64
    }

    /// A count of frames as "HH:MM:SS:FF". Frames are counted without
    /// skipping any number (non-drop frame), which the list's header says.
    fn timecode(self, frame: u64) -> String {
        let base = self.timecode_base();
        let whole_seconds = frame / base;
        format!(
            "{:02}:{:02}:{:02}:{:02}",
            whole_seconds / (SECONDS_PER_MINUTE * MINUTES_PER_HOUR),
            whole_seconds / SECONDS_PER_MINUTE % MINUTES_PER_HOUR,
            whole_seconds % SECONDS_PER_MINUTE,
            frame % base
        )
    }
}

/// One scene as a list names it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedScene {
    /// The scene's number among those that are written, from one.
    pub number: usize,
    /// Where it is in the original video.
    pub span: TimeSpan,
    /// What its file would be called.
    pub file_name: String,
}

/// The scenes a split of one video would write.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneList {
    /// The video's file name, as the title of the list.
    pub video_name: String,
    pub frame_rate: FrameRate,
    pub scenes: Vec<ListedScene>,
}

/// Why a list cannot be written in the format asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SceneListError {
    TooManyScenesForEdl { scenes: usize },
}

impl fmt::Display for SceneListError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyScenesForEdl { scenes } => write!(
                formatter,
                "An edit list holds at most {EDL_MOST_EVENTS} scenes, and this video has {scenes}. \
                 Save the list as a table instead."
            ),
        }
    }
}

impl std::error::Error for SceneListError {}

impl SceneList {
    /// The list of what a split of `video` would write: the scenes that are
    /// kept, with the names `scene_name` gives their files.
    pub fn of_split(
        video: &Path,
        scenes: &ScenesToWrite,
        frame_rate: FrameRate,
        scene_name: SceneNamePattern,
    ) -> Result<Self, NamingError> {
        let kept_scenes = scenes.kept_scenes();
        let file_names = SceneNaming::of(video, scene_name)?.file_names(&kept_scenes)?;
        let video_name = video
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self::of(video_name, frame_rate, &kept_scenes, file_names))
    }

    /// Numbers the scenes from one and pairs each with its file name.
    pub fn of(
        video_name: String,
        frame_rate: FrameRate,
        scenes: &[TimeSpan],
        file_names: Vec<String>,
    ) -> Self {
        let scenes = scenes
            .iter()
            .zip(file_names)
            .enumerate()
            .map(|(position, (&span, file_name))| ListedScene {
                number: position + 1,
                span,
                file_name,
            })
            .collect();
        Self {
            video_name,
            frame_rate,
            scenes,
        }
    }

    pub fn in_format(&self, format: SceneListFormat) -> Result<String, SceneListError> {
        match format {
            SceneListFormat::Csv => Ok(self.as_csv()),
            SceneListFormat::Chapters => Ok(self.as_chapters()),
            SceneListFormat::Edl => self.as_edl(),
        }
    }

    /// One row per scene: its number, start, end, length, and the name its
    /// file would get.
    fn as_csv(&self) -> String {
        let mut table = String::from(CSV_HEADER);
        table.push_str(CSV_LINE_END);
        for scene in &self.scenes {
            let row = [
                scene.number.to_string(),
                format_timestamp(scene.span.start),
                format_timestamp(scene.span.end),
                format_timestamp(scene.span.seconds()),
                scene.file_name.clone(),
            ];
            let fields: Vec<String> = row.iter().map(|field| csv_field(field)).collect();
            table.push_str(&fields.join(","));
            table.push_str(CSV_LINE_END);
        }
        table
    }

    /// One chapter per scene, named "Scene 1" and so on.
    fn as_chapters(&self) -> String {
        let mut chapters = format!(
            "{CHAPTER_FILE_HEADER}\ntitle={}\n",
            chapter_value(&self.video_name)
        );
        for scene in &self.scenes {
            chapters.push_str(&format!(
                "\n[CHAPTER]\n{CHAPTER_TIMEBASE}\nSTART={}\nEND={}\ntitle={}\n",
                milliseconds(scene.span.start),
                milliseconds(scene.span.end),
                chapter_value(&format!("Scene {}", scene.number))
            ));
        }
        chapters
    }

    /// One cut per scene. The source times are those of the scene in the
    /// original video. The record times put the scenes one after another,
    /// which is the video without what was dropped.
    fn as_edl(&self) -> Result<String, SceneListError> {
        if self.scenes.len() > EDL_MOST_EVENTS {
            return Err(SceneListError::TooManyScenesForEdl {
                scenes: self.scenes.len(),
            });
        }
        let rate = self.frame_rate;
        let mut list = format!(
            "TITLE: {}\nFCM: NON-DROP FRAME\n",
            on_one_line(&self.video_name)
        );
        let mut record_frame = 0;
        for scene in &self.scenes {
            let source_in = rate.frame_at(scene.span.start);
            // A scene too short to hold a frame still takes one.
            let source_out = rate.frame_at(scene.span.end).max(source_in + 1);
            let record_out = record_frame + (source_out - source_in);
            list.push_str(&format!(
                "\n{:03}  {EDL_REEL}       V     C        {} {} {} {}\n\
                 * FROM CLIP NAME: {}\n\
                 * TO FILE: {}\n",
                scene.number,
                rate.timecode(source_in),
                rate.timecode(source_out),
                rate.timecode(record_frame),
                rate.timecode(record_out),
                on_one_line(&self.video_name),
                on_one_line(&scene.file_name)
            ));
            record_frame = record_out;
        }
        Ok(list)
    }
}

/// A field as a CSV table holds it: in double quotes when it contains a
/// comma, a quote or a line break, with each quote inside doubled.
fn csv_field(field: &str) -> String {
    if !field.contains([',', '"', '\n', '\r']) {
        return field.to_owned();
    }
    format!("\"{}\"", field.replace('"', "\"\""))
}

/// A value as ffmpeg's metadata format holds it.
fn chapter_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if CHAPTER_SPECIAL_CHARACTERS.contains(&character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// An edit list is read line by line, so a name in it must not break one.
fn on_one_line(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn milliseconds(seconds: f64) -> u64 {
    (seconds.max(0.0) * MILLISECONDS_PER_SECOND).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(frames_per_second: f64) -> FrameRate {
        FrameRate::per_second(frames_per_second).unwrap()
    }

    fn span(start: f64, end: f64) -> TimeSpan {
        TimeSpan { start, end }
    }

    fn list_of(video_name: &str, frames_per_second: f64, scenes: &[TimeSpan]) -> SceneList {
        let file_names = (1..=scenes.len())
            .map(|number| format!("film_scene_{number:03}.mp4"))
            .collect();
        SceneList::of(
            video_name.to_owned(),
            rate(frames_per_second),
            scenes,
            file_names,
        )
    }

    fn two_scenes() -> SceneList {
        list_of("film.mp4", 25.0, &[span(0.0, 62.48), span(62.48, 3725.04)])
    }

    #[test]
    fn the_table_has_a_header_and_a_row_per_scene() {
        assert_eq!(
            two_scenes().in_format(SceneListFormat::Csv).unwrap(),
            "scene,start,end,length,file\r\n\
             1,00:00:00.000,00:01:02.480,00:01:02.480,film_scene_001.mp4\r\n\
             2,00:01:02.480,01:02:05.040,01:01:02.560,film_scene_002.mp4\r\n"
        );
    }

    #[test]
    fn a_file_name_with_a_comma_or_a_quote_is_quoted_in_the_table() {
        assert_eq!(csv_field("plain name.mp4"), "plain name.mp4");
        assert_eq!(csv_field("one, two.mp4"), "\"one, two.mp4\"");
        assert_eq!(csv_field("the \"best\".mp4"), "\"the \"\"best\"\".mp4\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
    }

    #[test]
    fn a_quoted_file_name_stays_one_field_of_its_row() {
        let list = SceneList::of(
            "a, b.mp4".to_owned(),
            rate(25.0),
            &[span(0.0, 1.0)],
            vec!["a, b_scene_001.mp4".to_owned()],
        );
        let table = list.in_format(SceneListFormat::Csv).unwrap();
        assert!(
            table.ends_with(",00:00:01.000,\"a, b_scene_001.mp4\"\r\n"),
            "{table}"
        );
    }

    #[test]
    fn chapters_are_given_in_whole_milliseconds() {
        assert_eq!(
            two_scenes().in_format(SceneListFormat::Chapters).unwrap(),
            ";FFMETADATA1\n\
             title=film.mp4\n\
             \n\
             [CHAPTER]\n\
             TIMEBASE=1/1000\n\
             START=0\n\
             END=62480\n\
             title=Scene 1\n\
             \n\
             [CHAPTER]\n\
             TIMEBASE=1/1000\n\
             START=62480\n\
             END=3725040\n\
             title=Scene 2\n"
        );
    }

    #[test]
    fn what_ffmpeg_would_misread_in_a_chapter_file_is_escaped() {
        assert_eq!(chapter_value(r"a=b; c#d \ e"), r"a\=b\; c\#d \\ e");
        assert_eq!(chapter_value("two\nlines"), "two\\\nlines");
        assert_eq!(chapter_value("Scene 1"), "Scene 1");
    }

    #[test]
    fn the_title_of_a_chapter_file_is_escaped_too() {
        let chapters = list_of("a=b #1.mp4", 25.0, &[span(0.0, 1.0)])
            .in_format(SceneListFormat::Chapters)
            .unwrap();
        assert!(chapters.contains("title=a\\=b \\#1.mp4\n"), "{chapters}");
    }

    #[test]
    fn timecodes_count_frames_at_the_rate_of_the_video() {
        assert_eq!(
            rate(25.0).timecode(rate(25.0).frame_at(62.48)),
            "00:01:02:12"
        );
        assert_eq!(
            rate(24.0).timecode(rate(24.0).frame_at(3600.5)),
            "01:00:00:12"
        );
        assert_eq!(
            rate(60.0).timecode(rate(60.0).frame_at(0.999)),
            "00:00:01:00"
        );
        assert_eq!(rate(25.0).timecode(0), "00:00:00:00");
    }

    #[test]
    fn a_rate_just_under_a_whole_number_counts_with_that_whole_number() {
        let ntsc = rate(30000.0 / 1001.0);
        assert_eq!(ntsc.timecode_base(), 30);
        // Frame 300 is ten seconds of timecode, a little over ten on the clock.
        assert_eq!(ntsc.frame_at(10.01), 300);
        assert_eq!(ntsc.timecode(300), "00:00:10:00");
        assert_eq!(rate(24000.0 / 1001.0).timecode_base(), 24);
    }

    #[test]
    fn an_edit_list_has_one_cut_per_scene_with_source_and_record_times() {
        assert_eq!(
            two_scenes().in_format(SceneListFormat::Edl).unwrap(),
            "TITLE: film.mp4\n\
             FCM: NON-DROP FRAME\n\
             \n\
             001  AX       V     C        00:00:00:00 00:01:02:12 00:00:00:00 00:01:02:12\n\
             * FROM CLIP NAME: film.mp4\n\
             * TO FILE: film_scene_001.mp4\n\
             \n\
             002  AX       V     C        00:01:02:12 01:02:05:01 00:01:02:12 01:02:05:01\n\
             * FROM CLIP NAME: film.mp4\n\
             * TO FILE: film_scene_002.mp4\n"
        );
    }

    #[test]
    fn an_edit_list_without_a_dropped_scene_closes_the_gap_on_the_record_side() {
        // The scene from 10 to 20 seconds was dropped.
        let kept = [span(0.0, 10.0), span(20.0, 30.0)];
        let list = list_of("film.mp4", 25.0, &kept)
            .in_format(SceneListFormat::Edl)
            .unwrap();
        assert!(
            list.contains(
                "002  AX       V     C        00:00:20:00 00:00:30:00 00:00:10:00 00:00:20:00"
            ),
            "{list}"
        );
    }

    #[test]
    fn the_list_of_a_split_names_the_kept_scenes_as_their_files_will_be() {
        use crate::fates::SceneFate;
        use crate::scenes::CutTimes;

        let cuts = CutTimes::inside(span(0.0, 30.0), vec![10.0, 20.0]).unwrap();
        let fates = vec![SceneFate::Keep, SceneFate::Drop, SceneFate::Keep];
        let scenes = ScenesToWrite::new(cuts, fates).unwrap();
        let list = SceneList::of_split(
            Path::new("videos/film.mp4"),
            &scenes,
            rate(25.0),
            SceneNamePattern::default(),
        )
        .unwrap();

        assert_eq!(list.video_name, "film.mp4");
        assert_eq!(
            list.scenes,
            vec![
                ListedScene {
                    number: 1,
                    span: span(0.0, 10.0),
                    file_name: "film_scene_001.mp4".to_owned(),
                },
                ListedScene {
                    number: 2,
                    span: span(20.0, 30.0),
                    file_name: "film_scene_002.mp4".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_line_break_in_a_name_does_not_break_a_line_of_an_edit_list() {
        let list = list_of("two\nlines.mp4", 25.0, &[span(0.0, 1.0)])
            .in_format(SceneListFormat::Edl)
            .unwrap();
        assert!(list.starts_with("TITLE: two lines.mp4\n"), "{list}");
    }

    #[test]
    fn an_edit_list_of_more_scenes_than_it_can_number_is_refused() {
        let scenes: Vec<TimeSpan> = (0..1000)
            .map(|scene| span(scene as f64, scene as f64 + 1.0))
            .collect();
        let list = list_of("film.mp4", 25.0, &scenes);
        assert_eq!(
            list.in_format(SceneListFormat::Edl),
            Err(SceneListError::TooManyScenesForEdl { scenes: 1000 })
        );
        // The table has no such limit.
        assert!(list.in_format(SceneListFormat::Csv).is_ok());
    }

    #[test]
    fn a_frame_rate_has_to_be_a_positive_number() {
        for not_a_rate in [0.0, -25.0, f64::NAN, f64::INFINITY] {
            assert!(FrameRate::per_second(not_a_rate).is_err(), "{not_a_rate}");
        }
        assert!(FrameRate::from_frame_seconds(0.0).is_err());
        assert_eq!(FrameRate::from_frame_seconds(0.04), Ok(rate(25.0)));
    }

    #[test]
    fn the_format_is_read_from_the_name_of_the_file() {
        let format_of = |name: &str| SceneListFormat::for_file(Path::new(name));
        assert_eq!(format_of("scenes.csv"), Some(SceneListFormat::Csv));
        assert_eq!(format_of("scenes.EDL"), Some(SceneListFormat::Edl));
        assert_eq!(format_of("chapters.txt"), Some(SceneListFormat::Chapters));
        assert_eq!(
            format_of("chapters.ffmetadata"),
            Some(SceneListFormat::Chapters)
        );
        assert_eq!(format_of("scenes.json"), None);
        assert_eq!(format_of("scenes"), None);
    }

    #[test]
    fn every_format_has_a_label_and_its_own_extension() {
        for format in SceneListFormat::ALL {
            assert!(!format.label().is_empty());
            assert_eq!(
                SceneListFormat::for_file(Path::new(&format!("list.{}", format.extension()))),
                Some(format)
            );
        }
    }
}
