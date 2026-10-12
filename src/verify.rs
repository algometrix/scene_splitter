//! Proves that a set of scene files holds everything the original video did.
//!
//! Scenes are written by copying compressed packets, so a correct split has
//! exactly the original's packets, byte for byte and in the same order. This
//! check reads every packet of the original and of the scenes and compares
//! them, which also catches truncated files and broken indexes. It also
//! refuses an original that holds streams the scenes do not carry, such as
//! subtitles. It is the gate in front of removing the original.
//!
//! A split with dropped scenes is checked just as strictly. The packets of
//! each dropped scene are noted before it is removed, and stand in its
//! place when the scenes are joined. The join only equals the original when
//! every kept file holds exactly the packets of its own stretch of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use crate::ffmpeg::{
    NotUnderstood, Program, TimeLimit, COPY_SCENE_STREAMS, QUICK_ANSWER_LIMIT, STEADY_OUTPUT_LIMIT,
};
use crate::tools;

/// How much of each scene is decoded to make sure a player can open it.
const DECODE_CHECK_SECONDS: &str = "2";

/// The decode check opens one scene and decodes its first two seconds, which
/// takes a fraction of a second. A minute covers a slow drive and a picture
/// that is costly to decode, with every scene being checked at once.
const DECODE_CHECK_LIMIT: TimeLimit = TimeLimit::WholeRun(Duration::from_secs(60));

/// The video stream comes first in every file this program reads or writes.
const VIDEO_STREAM: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct PacketChecksum {
    size_bytes: u32,
    checksum: u32,
}

/// The packets of each stream, in the order the container stores them.
type PacketsByStream = BTreeMap<u32, Vec<PacketChecksum>>;

/// The packets a dropped scene held, noted while its file still existed.
#[derive(Debug, PartialEq)]
pub struct DroppedScenePackets(PacketsByStream);

/// One scene of a split, as the check takes it.
#[derive(Debug, PartialEq)]
pub enum SceneToVerify {
    /// The scene is in this file, which is read and decoded.
    InFile(PathBuf),
    /// The scene was dropped. Its packets fill its place in the original.
    Dropped(DroppedScenePackets),
}

/// Notes the packets of a scene that is about to be removed.
pub fn record_dropped_scene(scene: &Path) -> Result<DroppedScenePackets> {
    read_packet_checksums(scene).map(DroppedScenePackets)
}

/// Succeeds only when the scenes, played one after another, contain every
/// video and audio packet of the original, unchanged, and each scene opens
/// and decodes.
pub fn verify_scenes(original: &Path, scene_files: &[PathBuf]) -> Result<()> {
    let every_scene_in_a_file: Vec<SceneToVerify> = scene_files
        .iter()
        .cloned()
        .map(SceneToVerify::InFile)
        .collect();
    verify_scenes_with_dropped(original, &every_scene_in_a_file)
}

/// The same check for a split that dropped scenes. `scenes` are all scenes
/// of the split in order, dropped ones included. Succeeds only when every
/// scene file holds exactly the packets of its own stretch of the original,
/// unchanged, and opens and decodes.
pub fn verify_scenes_with_dropped(original: &Path, scenes: &[SceneToVerify]) -> Result<()> {
    // Scenes carry the first video stream and every audio stream. Anything
    // else in the original, such as subtitles, exists nowhere but there.
    let left_behind = streams_left_behind(&list_streams(original)?);
    if !left_behind.is_empty() {
        bail!(
            "the original also holds {}, which the scenes do not carry",
            left_behind.join(", ")
        );
    }

    let original_packets = read_packet_checksums(original)?;
    let scene_files: Vec<&PathBuf> = scenes
        .iter()
        .filter_map(|scene| match scene {
            SceneToVerify::InFile(scene_file) => Some(scene_file),
            SceneToVerify::Dropped(_) => None,
        })
        .collect();
    let packets_per_scene_file = scene_files
        .par_iter()
        .map(|scene| read_packet_checksums(scene))
        .collect::<Result<Vec<_>>>()?;

    for (scene, packets) in scene_files.iter().zip(&packets_per_scene_file) {
        if packets.get(&VIDEO_STREAM).is_none_or(Vec::is_empty) {
            bail!("{} contains no video", scene.display());
        }
    }
    let joined = join_scenes(in_scene_order(scenes, &packets_per_scene_file));
    if let Some(difference) = first_difference(&original_packets, &joined) {
        bail!("{difference}");
    }

    scene_files
        .par_iter()
        .try_for_each(|scene| check_scene_decodes(scene))
}

/// The packets of every scene in scene order: those read from its file for
/// a scene that was written, those noted earlier for one that was dropped.
/// `packets_per_scene_file` holds one entry per written scene, in order.
fn in_scene_order<'packets>(
    scenes: &'packets [SceneToVerify],
    packets_per_scene_file: &'packets [PacketsByStream],
) -> Vec<&'packets PacketsByStream> {
    let mut read_from_files = packets_per_scene_file.iter();
    scenes
        .iter()
        .filter_map(|scene| match scene {
            SceneToVerify::InFile(_) => read_from_files.next(),
            SceneToVerify::Dropped(DroppedScenePackets(noted)) => Some(noted),
        })
        .collect()
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

/// Has ffmpeg list a checksum for every packet, without decoding anything.
fn read_packet_checksums(video: &Path) -> Result<PacketsByStream> {
    let mut packets = PacketsByStream::new();
    // The listing grows with the video and is printed as the file is read.
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error"])
        .arg("-i")
        .arg(video)
        .args(COPY_SCENE_STREAMS)
        .args(["-f", "framecrc", "-"])
        .stream_lines(STEADY_OUTPUT_LIMIT, |line| {
            if is_header_line(line) {
                return Ok(());
            }
            let (stream, packet) = parse_checksum_line(line)
                .map_err(|error| NotUnderstood(format!("checksum line {line:?}, {error:#}")))?;
            packets.entry(stream).or_default().push(packet);
            Ok(())
        })
        .with_context(|| format!("could not read the packets of {}", video.display()))?;
    Ok(packets)
}

fn is_header_line(line: &str) -> bool {
    line.starts_with('#') || line.trim().is_empty()
}

/// A line reads: stream, dts, pts, duration, size, checksum. The timestamps
/// are skipped, since every scene restarts its clock at zero.
fn parse_checksum_line(line: &str) -> Result<(u32, PacketChecksum)> {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let [stream, _dts, _pts, _duration, size, checksum, ..] = fields[..] else {
        bail!("expected at least six fields");
    };
    let checksum = checksum
        .strip_prefix("0x")
        .context("checksum is not hexadecimal")?;
    Ok((
        stream.parse().context("stream is not a number")?,
        PacketChecksum {
            size_bytes: size.parse().context("size is not a number")?,
            checksum: u32::from_str_radix(checksum, 16).context("checksum is not hexadecimal")?,
        },
    ))
}

fn join_scenes(packets_per_scene: Vec<&PacketsByStream>) -> PacketsByStream {
    let mut joined = PacketsByStream::new();
    for scene_packets in packets_per_scene {
        for (stream, packets) in scene_packets {
            joined.entry(*stream).or_default().extend(packets);
        }
    }
    joined
}

/// Describes the first way the scenes differ from the original, if any.
fn first_difference(original: &PacketsByStream, scenes: &PacketsByStream) -> Option<String> {
    if let Some(extra_stream) = scenes.keys().find(|stream| !original.contains_key(stream)) {
        return Some(format!(
            "the scenes have a stream {extra_stream} that the original lacks"
        ));
    }
    for (stream, original_packets) in original {
        let no_packets = Vec::new();
        let scene_packets = scenes.get(stream).unwrap_or(&no_packets);
        if original_packets.len() != scene_packets.len() {
            return Some(format!(
                "stream {stream} has {} packets in the original but {} in the scenes",
                original_packets.len(),
                scene_packets.len()
            ));
        }
        let changed_packet = original_packets
            .iter()
            .zip(scene_packets)
            .position(|(original, scene)| original != scene);
        if let Some(packet) = changed_packet {
            return Some(format!(
                "packet {packet} of stream {stream} differs from the original"
            ));
        }
    }
    None
}

/// Identical packets do not help if the file's headers are broken, so the
/// start of each scene is also decoded.
fn check_scene_decodes(scene: &Path) -> Result<()> {
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-xerror"])
        .arg("-i")
        .arg(scene)
        .args(["-t", DECODE_CHECK_SECONDS, "-f", "null", "-"])
        .run(DECODE_CHECK_LIMIT)
        .with_context(|| format!("{} does not decode", scene.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(size_bytes: u32, checksum: u32) -> PacketChecksum {
        PacketChecksum {
            size_bytes,
            checksum,
        }
    }

    fn streams(video: &[PacketChecksum], audio: &[PacketChecksum]) -> PacketsByStream {
        PacketsByStream::from([(0, video.to_vec()), (1, audio.to_vec())])
    }

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
    fn parses_a_checksum_line() {
        let (stream, parsed) =
            parse_checksum_line("0,       1024,       2048,      512,    13985, 0x9a3f01c2")
                .unwrap();
        assert_eq!(stream, 0);
        assert_eq!(parsed, packet(13985, 0x9a3f_01c2));
    }

    #[test]
    fn parses_a_line_with_side_data_after_the_checksum() {
        let (stream, parsed) =
            parse_checksum_line("1, 0, 0, 1024, 23, 0x0000002a, S=1, 10, 0x00000001").unwrap();
        assert_eq!(stream, 1);
        assert_eq!(parsed, packet(23, 0x2a));
    }

    #[test]
    fn rejects_a_truncated_line() {
        assert!(parse_checksum_line("0, 1, 2").is_err());
    }

    #[test]
    fn scenes_that_hold_every_packet_in_order_match() {
        let original = streams(
            &[packet(10, 1), packet(20, 2), packet(30, 3)],
            &[packet(5, 9)],
        );
        let scenes = join_scenes(vec![
            &streams(&[packet(10, 1)], &[packet(5, 9)]),
            &streams(&[packet(20, 2), packet(30, 3)], &[]),
        ]);
        assert_eq!(first_difference(&original, &scenes), None);
    }

    fn in_file(name: &str) -> SceneToVerify {
        SceneToVerify::InFile(PathBuf::from(name))
    }

    fn dropped(video: &[PacketChecksum], audio: &[PacketChecksum]) -> SceneToVerify {
        SceneToVerify::Dropped(DroppedScenePackets(streams(video, audio)))
    }

    /// An original of four video packets and two of sound, split in three
    /// with the scene in the middle dropped.
    fn original_of_three_scenes() -> PacketsByStream {
        streams(
            &[packet(10, 1), packet(20, 2), packet(30, 3), packet(40, 4)],
            &[packet(5, 8), packet(5, 9)],
        )
    }

    fn middle_scene_dropped() -> Vec<SceneToVerify> {
        vec![
            in_file("scene_001.mp4"),
            dropped(&[packet(20, 2), packet(30, 3)], &[packet(5, 9)]),
            in_file("scene_002.mp4"),
        ]
    }

    #[test]
    fn a_dropped_scene_fills_its_own_place_between_the_kept_ones() {
        let read_from_files = [
            streams(&[packet(10, 1)], &[packet(5, 8)]),
            streams(&[packet(40, 4)], &[]),
        ];
        let scenes = middle_scene_dropped();
        let joined = join_scenes(in_scene_order(&scenes, &read_from_files));
        assert_eq!(first_difference(&original_of_three_scenes(), &joined), None);
    }

    #[test]
    fn a_kept_scene_missing_a_packet_of_its_stretch_fails_with_scenes_dropped() {
        let read_from_files = [
            streams(&[packet(10, 1)], &[packet(5, 8)]),
            // The last packet of the video never reached the second file.
            streams(&[], &[]),
        ];
        let scenes = middle_scene_dropped();
        let joined = join_scenes(in_scene_order(&scenes, &read_from_files));
        assert_eq!(
            first_difference(&original_of_three_scenes(), &joined).unwrap(),
            "stream 0 has 4 packets in the original but 3 in the scenes"
        );
    }

    #[test]
    fn a_kept_scene_holding_part_of_a_dropped_one_fails() {
        let read_from_files = [
            // The first file runs on into the dropped scene.
            streams(&[packet(10, 1), packet(20, 2)], &[packet(5, 8)]),
            streams(&[packet(40, 4)], &[]),
        ];
        let scenes = middle_scene_dropped();
        let joined = join_scenes(in_scene_order(&scenes, &read_from_files));
        assert_eq!(
            first_difference(&original_of_three_scenes(), &joined).unwrap(),
            "stream 0 has 4 packets in the original but 5 in the scenes"
        );
    }

    #[test]
    fn kept_scenes_in_the_wrong_order_fail() {
        let read_from_files = [
            streams(&[packet(40, 4)], &[]),
            streams(&[packet(10, 1)], &[packet(5, 8)]),
        ];
        let scenes = middle_scene_dropped();
        let joined = join_scenes(in_scene_order(&scenes, &read_from_files));
        assert_eq!(
            first_difference(&original_of_three_scenes(), &joined).unwrap(),
            "packet 0 of stream 0 differs from the original"
        );
    }

    #[test]
    fn without_dropped_scenes_every_scene_is_read_from_its_file() {
        let read_from_files = [
            streams(&[packet(10, 1)], &[]),
            streams(&[packet(20, 2)], &[]),
        ];
        let scenes = [in_file("scene_001.mp4"), in_file("scene_002.mp4")];
        let in_order = in_scene_order(&scenes, &read_from_files);
        assert_eq!(in_order, vec![&read_from_files[0], &read_from_files[1]]);
    }

    #[test]
    fn a_missing_packet_is_reported() {
        let original = streams(&[packet(10, 1), packet(20, 2)], &[]);
        let scenes = streams(&[packet(10, 1)], &[]);
        assert_eq!(
            first_difference(&original, &scenes).unwrap(),
            "stream 0 has 2 packets in the original but 1 in the scenes"
        );
    }

    #[test]
    fn a_changed_packet_is_reported() {
        let original = streams(&[packet(10, 1), packet(20, 2)], &[]);
        let scenes = streams(&[packet(10, 1), packet(20, 7)], &[]);
        assert_eq!(
            first_difference(&original, &scenes).unwrap(),
            "packet 1 of stream 0 differs from the original"
        );
    }

    #[test]
    fn a_lost_audio_stream_is_reported() {
        let original = streams(&[packet(10, 1)], &[packet(5, 9)]);
        let scenes = PacketsByStream::from([(0, vec![packet(10, 1)])]);
        assert_eq!(
            first_difference(&original, &scenes).unwrap(),
            "stream 1 has 1 packets in the original but 0 in the scenes"
        );
    }
}
