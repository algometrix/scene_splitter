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
        let pictures = format!(
            "[0:v]{NOISE},format=yuv420p[first];\
             [1:v]format=yuv420p[black];\
             [2:v]{NOISE},format=yuv420p[second];\
             [first][black][second]concat=n=3:v=1:a=0[picture]"
        );
        Self::generate(
            test_name,
            &[
                pattern("testsrc", SCENE_SECONDS),
                pattern("color=c=black", BLACK_SECONDS),
                pattern("testsrc2", SCENE_SECONDS),
            ],
            &pictures,
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

    /// Encodes the pictures built by `picture_graph` from `patterns`, with a
    /// tone for sound. Panics on failure, since a test cannot go on without
    /// its video.
    fn generate(test_name: &str, patterns: &[String], picture_graph: &str) -> Self {
        Self::generate_encoded(test_name, patterns, picture_graph, &PLAIN_PICTURE_ENCODING)
    }

    fn generate_encoded(
        test_name: &str,
        patterns: &[String],
        picture_graph: &str,
        picture_encoding: &[&str],
    ) -> Self {
        let folder = std::env::temp_dir().join(format!(
            "scene_splitter_video_test_{}_{test_name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("could not create a folder for the test video");
        let path = folder.join("video.mp4");

        let mut ffmpeg = tools::command(Program::Ffmpeg).args(["-nostdin", "-v", "error", "-y"]);
        for pattern in patterns {
            ffmpeg = ffmpeg.args(["-f", "lavfi", "-i", pattern]);
        }
        let sound_input = patterns.len().to_string();
        let encoded = ffmpeg
            .args(["-f", "lavfi", "-i", "sine=frequency=440"])
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

impl Drop for TestVideo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn pattern(source: &str, seconds: f64) -> String {
    let separator = if source.contains('=') { ':' } else { '=' };
    format!("{source}{separator}duration={seconds}:size={PICTURE_SIZE}:rate={FRAME_RATE}")
}
