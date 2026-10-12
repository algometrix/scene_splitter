//! Small synthetic videos for the tests that run a real ffmpeg.
//!
//! Each video is made of test patterns, so the tests need no footage and
//! know exactly where every scene change is.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ffmpeg::{Program, TimeLimit};
use crate::tools;

pub const FRAME_RATE: u32 = 25;
pub const FRAME_SECONDS: f64 = 1.0 / FRAME_RATE as f64;
/// How long each scene of a test video lasts.
pub const SCENE_SECONDS: f64 = 4.0;
/// How long the black screen between two scenes lasts.
pub const BLACK_SECONDS: f64 = 1.0;
const PICTURE_SIZE: &str = "320x180";
/// Noise makes every frame costly to compress, as real footage is. Without
/// it the test patterns compress as well as a black screen does.
const NOISE: &str = "noise=alls=20:allf=t";
/// Encoding nine seconds of small pictures takes about a second. Two
/// minutes allows for a machine busy running every other test at once.
const ENCODING_LIMIT: TimeLimit = TimeLimit::WholeRun(Duration::from_secs(120));

/// A keyframe on every whole second and nowhere else, and every picture
/// stored in the order it is shown. The 25 is `FRAME_RATE`, which a list of
/// fixed texts cannot spell out.
const PLAIN_PICTURE_ENCODING: [&str; 6] =
    ["-preset", "ultrafast", "-g", "25", "-sc_threshold", "0"];
pub const REORDERED_VIDEO_SECONDS: f64 = 9.0;
/// How many pictures lie between two keyframes of the reordered video.
pub const REORDERED_KEYFRAME_EVERY: usize = 50;
/// A keyframe every two seconds, with up to two pictures in a row that are
/// stored after the picture that follows them.
const REORDERED_PICTURE_ENCODING: [&str; 8] = [
    "-preset",
    "veryfast",
    "-bf",
    "2",
    "-g",
    "50",
    "-sc_threshold",
    "0",
];

const OPEN_GROUP_PICTURE_ENCODING: [&str; 10] = [
    "-preset",
    "veryfast",
    "-bf",
    "2",
    "-g",
    "50",
    "-sc_threshold",
    "0",
    "-x264-params",
    "open-gop=1",
];

/// A generated video in a folder of its own, removed when the test ends.
pub struct TestVideo {
    folder: PathBuf,
    pub path: PathBuf,
}

impl TestVideo {
    /// Four seconds of one pattern, a second of black, four seconds of
    /// another pattern. Keyframes fall on every whole second, so one sits
    /// on the first black frame.
    pub fn two_scenes_with_a_black_screen(test_name: &str) -> Self {
        Self::generate(
            test_name,
            &black_screen_patterns(),
            &black_screen_pictures(),
        )
    }

    /// Four seconds of one pattern followed at once by four seconds of
    /// another, with no black between them.
    pub fn two_scenes_with_a_hard_cut(test_name: &str) -> Self {
        let pictures = format!(
            "[0:v]{NOISE},format=yuv420p[first];\
             [1:v]{NOISE},format=yuv420p[second];\
             [first][second]concat=n=2:v=1:a=0[picture]"
        );
        Self::generate(
            test_name,
            &[
                pattern("testsrc", SCENE_SECONDS),
                pattern("smptebars", SCENE_SECONDS),
            ],
            &pictures,
        )
    }

    /// The video with a black screen between its two scenes, in a file of
    /// the given kind that also carries `chapters`.
    pub fn two_scenes_with_chapters(
        test_name: &str,
        container: ChapterContainer,
        chapters: &[TestChapter],
    ) -> Self {
        Self::generate_from(
            test_name,
            &Recipe {
                file_name: container.file_name(),
                patterns: &black_screen_patterns(),
                picture_graph: &black_screen_pictures(),
                picture_encoding: &PLAIN_PICTURE_ENCODING,
                chapters,
            },
        )
    }

    /// Nine seconds of one pattern with a keyframe every two seconds, and
    /// with pictures stored in another order than they are shown, as most
    /// real videos store them.
    pub fn with_reordered_pictures(test_name: &str) -> Self {
        let pictures = format!("[0:v]{NOISE},format=yuv420p[picture]");
        Self::generate_encoded(
            test_name,
            &[pattern("testsrc", REORDERED_VIDEO_SECONDS)],
            &pictures,
            &REORDERED_PICTURE_ENCODING,
        )
    }

    /// Like `with_reordered_pictures`, but only the first keyframe is a
    /// clean start. The others are pictures that pictures after them reach
    /// back across, as in videos made for discs and broadcast.
    pub fn with_open_picture_groups(test_name: &str) -> Self {
        let pictures = format!("[0:v]{NOISE},format=yuv420p[picture]");
        Self::generate_encoded(
            test_name,
            &[pattern("testsrc", REORDERED_VIDEO_SECONDS)],
            &pictures,
            &OPEN_GROUP_PICTURE_ENCODING,
        )
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// An MP4 video without chapters, which is what most tests want.
    fn generate(test_name: &str, patterns: &[String], picture_graph: &str) -> Self {
        Self::generate_from(
            test_name,
            &Recipe {
                file_name: "video.mp4",
                patterns,
                picture_graph,
                picture_encoding: &PLAIN_PICTURE_ENCODING,
                chapters: &[],
            },
        )
    }

    /// An MP4 video without chapters whose pictures are encoded in a
    /// particular way.
    fn generate_encoded(
        test_name: &str,
        patterns: &[String],
        picture_graph: &str,
        picture_encoding: &[&str],
    ) -> Self {
        Self::generate_from(
            test_name,
            &Recipe {
                file_name: "video.mp4",
                patterns,
                picture_graph,
                picture_encoding,
                chapters: &[],
            },
        )
    }

    /// Encodes the pictures of `recipe`, with a tone for sound, into a file
    /// in a folder of its own. Panics on failure, since a test cannot go on
    /// without its video.
    fn generate_from(test_name: &str, recipe: &Recipe) -> Self {
        let Recipe {
            file_name,
            patterns,
            picture_graph,
            picture_encoding,
            chapters,
        } = *recipe;
        let folder = std::env::temp_dir().join(format!(
            "scene_splitter_video_test_{}_{test_name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("could not create a folder for the test video");
        let path = folder.join(file_name);

        let mut ffmpeg = tools::command(Program::Ffmpeg).args(["-nostdin", "-v", "error", "-y"]);
        for pattern in patterns {
            ffmpeg = ffmpeg.args(["-f", "lavfi", "-i", pattern]);
        }
        let sound_input = patterns.len();
        ffmpeg = ffmpeg.args(["-f", "lavfi", "-i", "sine=frequency=440"]);
        if !chapters.is_empty() {
            // ffmpeg takes chapters from a text file it reads as one more
            // input, the one after the sound.
            let chapter_file = folder.join("chapters.txt");
            std::fs::write(&chapter_file, chapter_file_text(chapters))
                .expect("could not write the chapters of the test video");
            let chapter_input = (sound_input + 1).to_string();
            ffmpeg = ffmpeg
                .args(["-f", "ffmetadata", "-i"])
                .arg(&chapter_file)
                .args(["-map_chapters", &chapter_input]);
        }
        let encoded = ffmpeg
            .args(["-filter_complex", picture_graph])
            .args(["-map", "[picture]", "-map", &format!("{sound_input}:a")])
            .args(["-c:v", "libx264"])
            .args(picture_encoding)
            .args(["-c:a", "aac", "-shortest"])
            .arg(&path)
            .run(ENCODING_LIMIT);
        if let Err(error) = encoded {
            panic!("ffmpeg could not make the test video: {error}");
        }
        Self { folder, path }
    }
}

/// What a test video is made of.
#[derive(Clone, Copy)]
struct Recipe<'parts> {
    file_name: &'parts str,
    /// The inputs ffmpeg makes pictures from.
    patterns: &'parts [String],
    /// Joins the inputs into the stream named `[picture]`.
    picture_graph: &'parts str,
    /// The options for the picture encoder.
    picture_encoding: &'parts [&'parts str],
    chapters: &'parts [TestChapter],
}

impl Drop for TestVideo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The inputs of the video with a black screen: a pattern, black, another
/// pattern.
fn black_screen_patterns() -> [String; 3] {
    [
        pattern("testsrc", SCENE_SECONDS),
        pattern("color=c=black", BLACK_SECONDS),
        pattern("testsrc2", SCENE_SECONDS),
    ]
}

/// Joins the three inputs of the video with a black screen into its
/// pictures.
fn black_screen_pictures() -> String {
    format!(
        "[0:v]{NOISE},format=yuv420p[first];\
         [1:v]format=yuv420p[black];\
         [2:v]{NOISE},format=yuv420p[second];\
         [first][black][second]concat=n=3:v=1:a=0[picture]"
    )
}

/// The kinds of file a test video with chapters can be. They keep chapters
/// in different ways, which matters once the video is split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChapterContainer {
    /// Keeps chapters as a list beside the streams.
    Matroska,
    /// Keeps chapters, as ffmpeg writes them, in a stream of their own.
    Mp4,
}

impl ChapterContainer {
    fn file_name(self) -> &'static str {
        match self {
            Self::Matroska => "video.mkv",
            Self::Mp4 => "video.mp4",
        }
    }
}

/// One chapter of a test video.
pub struct TestChapter {
    pub start_seconds: f64,
    /// `None` for a chapter without a name.
    pub title: Option<&'static str>,
}

const MILLISECONDS_PER_SECOND: f64 = 1000.0;
/// How long the last chapter is said to last. A chapter must end somewhere,
/// and the tests only look at where chapters begin.
const LAST_CHAPTER_SECONDS: f64 = 1.0;

/// The chapters in the text format ffmpeg reads them from. Each chapter
/// runs until the next one begins.
fn chapter_file_text(chapters: &[TestChapter]) -> String {
    let milliseconds = |seconds: f64| (seconds * MILLISECONDS_PER_SECOND).round() as u64;
    let mut text = String::from(";FFMETADATA1\n");
    for (position, chapter) in chapters.iter().enumerate() {
        let end_seconds = chapters
            .get(position + 1)
            .map_or(chapter.start_seconds + LAST_CHAPTER_SECONDS, |next| {
                next.start_seconds
            });
        text.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\n",
            milliseconds(chapter.start_seconds),
            milliseconds(end_seconds)
        ));
        if let Some(title) = chapter.title {
            text.push_str(&format!("title={title}\n"));
        }
    }
    text
}

fn pattern(source: &str, seconds: f64) -> String {
    let separator = if source.contains('=') { ':' } else { '=' };
    format!("{source}{separator}duration={seconds}:size={PICTURE_SIZE}:rate={FRAME_RATE}")
}
