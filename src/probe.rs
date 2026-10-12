//! Reads the video's packet table with ffprobe. This only demuxes, so it costs
//! about a second even for a multi-hour file.

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::black::TimeSpan;
use crate::tools;

/// One compressed video frame, as stored in the container.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoPacket {
    /// Presentation time in seconds from the start of the file. This is the
    /// same clock that `ffmpeg -ss` and video players use.
    pub time: f64,
    pub size_bytes: u32,
    pub is_keyframe: bool,
}

/// Every video frame of the file in presentation order.
#[derive(Debug)]
pub struct PacketIndex {
    pub packets: Vec<VideoPacket>,
    /// From the first frame appearing to the last frame disappearing.
    pub video: TimeSpan,
}

impl PacketIndex {
    pub fn keyframe_times(&self) -> Vec<f64> {
        self.packets
            .iter()
            .filter(|packet| packet.is_keyframe)
            .map(|packet| packet.time)
            .collect()
    }
}

pub fn read_packet_index(input: &Path) -> Result<PacketIndex> {
    let container_start = read_container_start(input)?;
    let listing = run_ffprobe(
        input,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,dts_time,size,flags",
            "-of",
            "csv=p=0",
        ],
    )?;

    let mut packets = Vec::new();
    for line in listing.lines().filter(|line| !line.trim().is_empty()) {
        let packet = parse_packet_line(line, container_start)
            .with_context(|| format!("unexpected ffprobe packet line {line:?}"))?;
        packets.push(packet);
    }
    if packets.is_empty() {
        bail!("{} has no video frames", input.display());
    }
    packets.sort_by(|a, b| a.time.total_cmp(&b.time));

    let video = TimeSpan {
        start: packets[0].time,
        end: estimate_video_end(&packets),
    };
    Ok(PacketIndex { packets, video })
}

/// ffprobe reports absolute timestamps, while `ffmpeg -ss` counts from the
/// container's start time. Subtracting it puts every time on the seek clock.
fn read_container_start(input: &Path) -> Result<f64> {
    let output = run_ffprobe(
        input,
        &["-show_entries", "format=start_time", "-of", "csv=p=0"],
    )?;
    let field = output.trim().trim_end_matches(',');
    if field.is_empty() || field == "N/A" {
        return Ok(0.0);
    }
    field
        .parse()
        .with_context(|| format!("unexpected container start time {field:?}"))
}

/// ffprobe always prints these fields in the order pts_time, dts_time, size,
/// flags, whatever order they were requested in.
fn parse_packet_line(line: &str, container_start: f64) -> Result<VideoPacket> {
    let mut fields = line.split(',');
    let pts = fields.next().context("missing pts_time")?;
    let dts = fields.next().context("missing dts_time")?;
    let size = fields.next().context("missing size")?;
    let flags = fields.next().context("missing flags")?;

    // Some containers leave pts unset on a few packets, dts is the next best clock.
    let timestamp = if pts == "N/A" { dts } else { pts };
    let absolute_time: f64 = timestamp.parse().context("timestamp is not a number")?;

    Ok(VideoPacket {
        time: absolute_time - container_start,
        size_bytes: size.parse().context("size is not a number")?,
        is_keyframe: flags.starts_with('K'),
    })
}

/// The container does not say how long the last frame is shown, so assume it
/// lasts as long as a typical frame.
fn estimate_video_end(packets: &[VideoPacket]) -> f64 {
    let first = packets[0].time;
    let last = packets[packets.len() - 1].time;
    if packets.len() < 2 {
        return last;
    }
    let typical_frame_duration = (last - first) / (packets.len() - 1) as f64;
    last + typical_frame_duration
}

fn run_ffprobe(input: &Path, arguments: &[&str]) -> Result<String> {
    let output = tools::ffprobe_command()
        .args(["-v", "error"])
        .args(arguments)
        .arg(input)
        .output()
        .context("could not start ffprobe, is it on PATH?")?;
    if !output.status.success() {
        bail!(
            "ffprobe failed on {}: {}",
            input.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("ffprobe printed non UTF-8 output")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_keyframe_line() {
        let packet = parse_packet_line("3792.000000,3791.933333,13985,K_", 0.0).unwrap();
        assert_eq!(
            packet,
            VideoPacket {
                time: 3792.0,
                size_bytes: 13985,
                is_keyframe: true
            }
        );
    }

    #[test]
    fn falls_back_to_dts_and_removes_the_container_start() {
        let packet = parse_packet_line("N/A,11.500000,24,__", 1.5).unwrap();
        assert_eq!(packet.time, 10.0);
        assert!(!packet.is_keyframe);
    }

    #[test]
    fn rejects_a_truncated_line() {
        assert!(parse_packet_line("1.0,1.0", 0.0).is_err());
    }

    #[test]
    fn video_ends_one_frame_after_the_last_packet() {
        let packets: Vec<VideoPacket> = [0.0, 0.5, 1.0]
            .iter()
            .map(|&time| VideoPacket {
                time,
                size_bytes: 100,
                is_keyframe: false,
            })
            .collect();
        assert_eq!(estimate_video_end(&packets), 1.5);
    }
}
