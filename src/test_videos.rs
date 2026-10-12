//! Small synthetic videos for the tests that run a real ffmpeg.
//!
//! Each video is made of test patterns, so the tests need no footage and
//! know exactly where every scene change is.

use std::path::{Path, PathBuf};
use std::process::Stdio;

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

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Encodes the pictures built by `picture_graph` from `patterns`, with a
    /// tone for sound. Panics on failure, since a test cannot go on without
    /// its video.
    fn generate(test_name: &str, patterns: &[String], picture_graph: &str) -> Self {
        let folder = std::env::temp_dir().join(format!(
            "scene_splitter_video_test_{}_{test_name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("could not create a folder for the test video");
        let path = folder.join("video.mp4");

        let mut ffmpeg = tools::ffmpeg_command();
        ffmpeg.args(["-nostdin", "-v", "error", "-y"]);
        for pattern in patterns {
            ffmpeg.args(["-f", "lavfi", "-i", pattern]);
        }
        let sound_input = patterns.len().to_string();
        let output = ffmpeg
            .args(["-f", "lavfi", "-i", "sine=frequency=440"])
            .args(["-filter_complex", picture_graph])
            .args(["-map", "[picture]", "-map", &format!("{sound_input}:a")])
            // A keyframe on every whole second and nowhere else.
            .args(["-c:v", "libx264", "-preset", "ultrafast"])
            .args(["-g", &FRAME_RATE.to_string(), "-sc_threshold", "0"])
            .args(["-c:a", "aac", "-shortest"])
            .arg(&path)
            .stdin(Stdio::null())
            .output()
            .expect("could not start ffmpeg, is it on PATH?");
        assert!(
            output.status.success(),
            "ffmpeg could not make the test video: {}",
            String::from_utf8_lossy(&output.stderr)
        );
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
