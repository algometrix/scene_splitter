//! Makes sure the scene files of a split are whole videos that play.
//!
//! Scenes are written by copying compressed packets, so what can go wrong
//! is the file around them: one that is cut short, has a broken index, or
//! opens to no picture. This check reads every scene file to its end and
//! decodes the start of its pictures. It also refuses an original that
//! holds streams the scenes do not carry, such as subtitles. It is the gate
//! in front of removing the original.
//!
//! It does not compare the scenes with the original. Damage the original
//! has itself, such as a broken packet of sound, is copied as it is and
//! does not fail the check.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use crate::ffmpeg::{
    NotUnderstood, Program, TimeLimit, COPY_SCENE_STREAMS, QUICK_ANSWER_LIMIT, STEADY_OUTPUT_LIMIT,
};
use crate::tools;

/// How much of each scene is decoded to make sure a player shows it.
const DECODE_CHECK_SECONDS: &str = "2";

/// The decode check opens one scene and decodes its first two seconds, which
/// takes a fraction of a second. A minute covers a slow drive and a picture
/// that is costly to decode, with every scene being checked at once.
const DECODE_CHECK_LIMIT: TimeLimit = TimeLimit::WholeRun(Duration::from_secs(60));

/// The video stream comes first in every file this program writes.
const VIDEO_STREAM: u32 = 0;

/// Hears how much of the check is done, from 0.0 to 1.0. Called from the
/// threads that read the scenes, so two reports close together can arrive
/// out of order.
pub type OnShareChecked<'a> = &'a (dyn Fn(f32) + Sync);

/// For callers that do not show how far the check is.
#[cfg(test)]
pub fn no_share_checked(_share: f32) {}

/// Succeeds only when the scenes carry every stream of the original, and
/// each scene file reads to its end and shows pictures.
pub fn verify_scenes(
    original: &Path,
    scene_files: &[PathBuf],
    on_share_checked: OnShareChecked,
) -> Result<()> {
    // Scenes carry the first video stream and every audio stream. Anything
    // else in the original, such as subtitles, exists nowhere but there.
    let left_behind = streams_left_behind(&list_streams(original)?);
    if !left_behind.is_empty() {
        bail!(
            "the original also holds {}, which the scenes do not carry",
            left_behind.join(", ")
        );
    }

    let read_so_far = BytesRead::of(scene_files, on_share_checked)?;
    scene_files
        .par_iter()
        .try_for_each(|scene| check_scene_plays(scene, &read_so_far))
}

/// How much of all scene files has been read, counted in bytes of packets.
/// The packets of a file are nearly all of it, so the share ends just short
/// of the whole.
struct BytesRead<'a> {
    bytes_to_read: u64,
    bytes_read: AtomicU64,
    on_share_checked: OnShareChecked<'a>,
}

impl<'a> BytesRead<'a> {
    fn of(scene_files: &[PathBuf], on_share_checked: OnShareChecked<'a>) -> Result<Self> {
        let mut bytes_to_read = 0;
        for scene in scene_files {
            bytes_to_read += std::fs::metadata(scene)
                .with_context(|| format!("could not find {}", scene.display()))?
                .len();
        }
        Ok(Self {
            bytes_to_read,
            bytes_read: AtomicU64::new(0),
            on_share_checked,
        })
    }

    fn add(&self, packet_bytes: u32) {
        let packet_bytes = u64::from(packet_bytes);
        let bytes_read = self.bytes_read.fetch_add(packet_bytes, Ordering::Relaxed) + packet_bytes;
        if self.bytes_to_read > 0 {
            (self.on_share_checked)(share_of(bytes_read, self.bytes_to_read));
        }
    }
}

fn share_of(bytes_read: u64, bytes_to_read: u64) -> f32 {
    (bytes_read as f64 / bytes_to_read as f64).min(1.0) as f32
}

fn check_scene_plays(scene: &Path, read_so_far: &BytesRead) -> Result<()> {
    let video_packets = read_to_the_end(scene, read_so_far)?;
    if video_packets == 0 {
        bail!("{} contains no video", scene.display());
    }
    check_start_shows_pictures(scene)
}

/// One line per stream of the video: its index and its kind.
fn list_streams(video: &Path) -> Result<String> {
    tools::command(Program::Ffprobe)
        .args(["-v", "error"])
        .args(["-show_entries", "stream=index,codec_type", "-of", "csv=p=0"])
        .arg(video)
        .run_for_text(QUICK_ANSWER_LIMIT)
        .with_context(|| format!("could not list the streams of {}", video.display()))
}

/// Names the streams a split does not copy: everything except the first
/// video stream and the audio streams, which is what `COPY_SCENE_STREAMS`
/// selects. Cover art counts as a second video stream, so it is named too.
fn streams_left_behind(stream_listing: &str) -> Vec<String> {
    let mut first_video_seen = false;
    let mut left_behind = Vec::new();
    for line in stream_listing
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let mut fields = line.split(',').map(str::trim);
        let index = fields.next().unwrap_or("?");
        let kind = fields.next().unwrap_or("unknown");
        let carried = match kind {
            "audio" => true,
            "video" if !first_video_seen => {
                first_video_seen = true;
                true
            }
            _ => false,
        };
        if !carried {
            left_behind.push(format!("{kind} stream {index}"));
        }
    }
    left_behind
}

/// Has ffmpeg read every packet of the scene without decoding any, and
/// counts those of the video. A file that is cut short or whose index is
/// broken stops ffmpeg with an error.
fn read_to_the_end(scene: &Path, read_so_far: &BytesRead) -> Result<usize> {
    let mut video_packets = 0;
    // The listing grows with the scene and is printed as the file is read.
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-xerror"])
        .arg("-i")
        .arg(scene)
        .args(COPY_SCENE_STREAMS)
        .args(["-f", "framecrc", "-"])
        .stream_lines(STEADY_OUTPUT_LIMIT, |line| {
            if is_header_line(line) {
                return Ok(());
            }
            let packet = parse_packet_line(line)
                .map_err(|error| NotUnderstood(format!("packet line {line:?}, {error:#}")))?;
            if packet.stream == VIDEO_STREAM {
                video_packets += 1;
            }
            read_so_far.add(packet.size_bytes);
            Ok(())
        })
        .with_context(|| format!("could not read {} to its end", scene.display()))?;
    Ok(video_packets)
}

fn is_header_line(line: &str) -> bool {
    line.starts_with('#') || line.trim().is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ListedPacket {
    stream: u32,
    size_bytes: u32,
}

/// A line reads: stream, dts, pts, duration, size, checksum.
fn parse_packet_line(line: &str) -> Result<ListedPacket> {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let [stream, _dts, _pts, _duration, size, _checksum, ..] = fields[..] else {
        bail!("expected at least six fields");
    };
    Ok(ListedPacket {
        stream: stream.parse().context("stream is not a number")?,
        size_bytes: size.parse().context("size is not a number")?,
    })
}

/// A file can hold every packet and still open to nothing, when what tells
/// a player how to decode them is broken. So the start of each scene is
/// decoded, and has to give at least one picture.
///
/// A decoder error alone does not fail this. The original may hold a
/// damaged packet, and a copy of it is as good as the original.
fn check_start_shows_pictures(scene: &Path) -> Result<()> {
    let mut pictures = 0;
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error"])
        .arg("-i")
        .arg(scene)
        .args(["-map", "0:v:0", "-t", DECODE_CHECK_SECONDS])
        .args(["-f", "framecrc", "-"])
        .stream_lines(DECODE_CHECK_LIMIT, |line| {
            if !is_header_line(line) {
                pictures += 1;
            }
            Ok(())
        })
        .with_context(|| format!("{} does not decode", scene.display()))?;
    if pictures == 0 {
        bail!("{} shows no picture when it is played", scene.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_with_only_picture_and_sound_leaves_nothing_behind() {
        assert!(streams_left_behind(
            "0,video
1,audio
2,audio
"
        )
        .is_empty());
    }

    #[test]
    fn subtitles_and_data_streams_are_left_behind() {
        assert_eq!(
            streams_left_behind(
                "0,video
1,audio
2,subtitle
3,data
"
            ),
            vec!["subtitle stream 2", "data stream 3"]
        );
    }

    #[test]
    fn a_second_video_stream_such_as_cover_art_is_left_behind() {
        assert_eq!(
            streams_left_behind(
                "0,video
1,audio
2,video
"
            ),
            vec!["video stream 2"]
        );
    }

    #[test]
    fn parses_a_packet_line() {
        assert_eq!(
            parse_packet_line("0,       1024,       2048,      512,    13985, 0x9a3f01c2").unwrap(),
            ListedPacket {
                stream: 0,
                size_bytes: 13985
            }
        );
    }

    #[test]
    fn parses_a_line_with_side_data_after_the_checksum() {
        assert_eq!(
            parse_packet_line("1, 0, 0, 1024, 23, 0x0000002a, S=1, 10, 0x00000001").unwrap(),
            ListedPacket {
                stream: 1,
                size_bytes: 23
            }
        );
    }

    #[test]
    fn rejects_a_truncated_line() {
        assert!(parse_packet_line("0, 1, 2").is_err());
    }

    #[test]
    fn the_share_read_never_passes_the_whole() {
        assert_eq!(share_of(50, 200), 0.25);
        assert_eq!(share_of(300, 200), 1.0);
    }
}
