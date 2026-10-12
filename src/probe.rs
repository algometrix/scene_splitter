//! Reads the video's packet table with ffprobe. This only demuxes, so it costs
//! about a second even for a multi-hour file.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::chapters::Chapter;
use crate::ffmpeg::{NotUnderstood, Program, ToolCommand, QUICK_ANSWER_LIMIT, STEADY_OUTPUT_LIMIT};
use crate::timecode::TimeSpan;
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

    let mut packets = Vec::new();
    // ffprobe reads through the file and prints each packet as it finds it,
    // so the time this takes grows with the video while the output never
    // pauses for long.
    ffprobe_on(
        input,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,dts_time,size,flags",
            "-of",
            "csv=p=0",
        ],
    )
    .stream_lines(STEADY_OUTPUT_LIMIT, |line| {
        if line.trim().is_empty() {
            return Ok(());
        }
        let packet = parse_packet_line(line, container_start)
            .map_err(|error| NotUnderstood(format!("packet line {line:?}, {error:#}")))?;
        packets.push(packet);
        Ok(())
    })
    .with_context(|| format!("could not list the frames of {}", input.display()))?;

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
    let output = ffprobe_on(
        input,
        &["-show_entries", "format=start_time", "-of", "csv=p=0"],
    )
    .run_for_text(QUICK_ANSWER_LIMIT)
    .with_context(|| format!("could not open {}", input.display()))?;
    let field = output.trim().trim_end_matches(',');
    if field.is_empty() || field == "N/A" {
        return Ok(0.0);
    }
    field
        .parse()
        .with_context(|| format!("unexpected container start time {field:?}"))
}

/// The chapter marks of the file, in the order they begin. Most videos have
/// none, which is not an error.
pub fn read_chapters(input: &Path) -> Result<Vec<Chapter>> {
    let listing = ffprobe_on(
        input,
        &[
            "-show_chapters",
            "-show_entries",
            "format=start_time",
            "-of",
            "json",
        ],
    )
    .run_for_text(QUICK_ANSWER_LIMIT)
    .with_context(|| format!("could not list the chapters of {}", input.display()))?;
    parse_chapter_listing(&listing)
        .with_context(|| format!("could not understand the chapters of {}", input.display()))
}

/// What `ffprobe -show_chapters -show_entries format=start_time -of json`
/// prints, as far as this program reads it.
#[derive(Deserialize)]
struct ChapterListing {
    #[serde(default)]
    chapters: Vec<ListedChapter>,
    #[serde(default)]
    format: ListedFormat,
}

#[derive(Deserialize)]
struct ListedChapter {
    start_time: String,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
struct ListedFormat {
    /// Left out, or "N/A", for a container without a start time.
    start_time: Option<String>,
}

const CHAPTER_TITLE_TAG: &str = "title";

/// Chapters are listed on the same absolute clock as packets, so the
/// container's start time is taken off them as well.
fn parse_chapter_listing(listing: &str) -> Result<Vec<Chapter>> {
    let listing: ChapterListing =
        serde_json::from_str(listing).context("the listing is not what ffprobe prints")?;
    let container_start = match listing.format.start_time.as_deref() {
        None | Some("N/A") => 0.0,
        Some(start) => start
            .parse()
            .with_context(|| format!("unexpected container start time {start:?}"))?,
    };

    let mut chapters = Vec::with_capacity(listing.chapters.len());
    for listed in listing.chapters {
        let absolute_start: f64 = listed
            .start_time
            .parse()
            .with_context(|| format!("unexpected chapter start {:?}", listed.start_time))?;
        // Some containers write the tag in capitals. A chapter track without
        // names gives every chapter an empty title.
        let title = listed
            .tags
            .iter()
            .find(|(tag, _)| tag.eq_ignore_ascii_case(CHAPTER_TITLE_TAG))
            .map(|(_, title)| title.trim())
            .filter(|title| !title.is_empty())
            .map(str::to_owned);
        chapters.push(Chapter {
            start: absolute_start - container_start,
            title,
        });
    }
    chapters.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(chapters)
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

fn ffprobe_on(input: &Path, arguments: &[&str]) -> ToolCommand {
    tools::command(Program::Ffprobe)
        .args(["-v", "error"])
        .args(arguments)
        .arg(input)
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

    fn chapter(start: f64, title: Option<&str>) -> Chapter {
        Chapter {
            start,
            title: title.map(str::to_owned),
        }
    }

    #[test]
    fn chapters_are_read_with_their_titles() {
        let listing = r#"{
            "chapters": [
                {"id": 0, "time_base": "1/1000", "start": 0, "start_time": "0.000000",
                 "end": 2300, "end_time": "2.300000", "tags": {"title": "Opening"}},
                {"id": 1, "time_base": "1/1000", "start": 2300, "start_time": "2.300000",
                 "end": 4600, "end_time": "4.600000", "tags": {"TITLE": " The heist "}}
            ],
            "format": {"start_time": "0.000000"}
        }"#;
        assert_eq!(
            parse_chapter_listing(listing).unwrap(),
            vec![
                chapter(0.0, Some("Opening")),
                chapter(2.3, Some("The heist"))
            ]
        );
    }

    #[test]
    fn a_chapter_with_an_empty_title_or_none_has_no_title() {
        let listing = r#"{"chapters": [
            {"start_time": "1.000000", "tags": {"title": ""}},
            {"start_time": "2.000000"}
        ]}"#;
        assert_eq!(
            parse_chapter_listing(listing).unwrap(),
            vec![chapter(1.0, None), chapter(2.0, None)]
        );
    }

    #[test]
    fn a_video_without_chapters_has_an_empty_list() {
        let listing = r#"{"chapters": [], "format": {"start_time": "0.000000"}}"#;
        assert!(parse_chapter_listing(listing).unwrap().is_empty());
        assert!(parse_chapter_listing("{}").unwrap().is_empty());
    }

    #[test]
    fn chapters_are_put_on_the_clock_that_starts_with_the_container() {
        let listing = r#"{"chapters": [{"start_time": "11.500000"}],
            "format": {"start_time": "1.500000"}}"#;
        assert_eq!(
            parse_chapter_listing(listing).unwrap(),
            vec![chapter(10.0, None)]
        );
    }

    #[test]
    fn a_container_without_a_start_time_starts_at_zero() {
        let listing = r#"{"chapters": [{"start_time": "4.000000"}],
            "format": {"start_time": "N/A"}}"#;
        assert_eq!(
            parse_chapter_listing(listing).unwrap(),
            vec![chapter(4.0, None)]
        );
    }

    #[test]
    fn chapters_listed_out_of_order_come_out_in_order() {
        let listing = r#"{"chapters": [{"start_time": "9.0"}, {"start_time": "3.0"}]}"#;
        let starts: Vec<f64> = parse_chapter_listing(listing)
            .unwrap()
            .iter()
            .map(|chapter| chapter.start)
            .collect();
        assert_eq!(starts, vec![3.0, 9.0]);
    }

    #[test]
    fn a_chapter_listing_that_makes_no_sense_is_refused() {
        assert!(parse_chapter_listing("not json").is_err());
        assert!(parse_chapter_listing(r#"{"chapters": [{"start_time": "soon"}]}"#).is_err());
        assert!(parse_chapter_listing(r#"{"chapters": [{"id": 1}]}"#).is_err());
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

    const NO_CONTAINER_START: f64 = 0.0;

    fn packets_at(times: &[f64]) -> Vec<VideoPacket> {
        times
            .iter()
            .map(|&time| VideoPacket {
                time,
                size_bytes: 100,
                is_keyframe: false,
            })
            .collect()
    }

    #[test]
    fn rejects_a_packet_with_neither_timestamp() {
        assert!(parse_packet_line("N/A,N/A,24,__", NO_CONTAINER_START).is_err());
    }

    #[test]
    fn rejects_a_timestamp_that_is_not_a_number() {
        assert!(parse_packet_line("soon,1.000000,24,K_", NO_CONTAINER_START).is_err());
    }

    #[test]
    fn rejects_a_size_that_is_not_a_number() {
        assert!(parse_packet_line("1.000000,1.000000,large,K_", NO_CONTAINER_START).is_err());
    }

    #[test]
    fn rejects_an_empty_line() {
        assert!(parse_packet_line("", NO_CONTAINER_START).is_err());
    }

    #[test]
    fn ignores_extra_fields_after_the_flags() {
        // ffprobe appends side data columns to some packets.
        let packet = parse_packet_line("1.500000,1.500000,24,K_,extra,", NO_CONTAINER_START);
        assert_eq!(
            packet.unwrap(),
            VideoPacket {
                time: 1.5,
                size_bytes: 24,
                is_keyframe: true
            }
        );
    }

    #[test]
    fn a_discardable_packet_is_not_a_keyframe() {
        let packet = parse_packet_line("1.500000,1.500000,24,_D_", NO_CONTAINER_START).unwrap();
        assert!(!packet.is_keyframe);
    }

    #[test]
    fn a_packet_before_the_container_start_gets_a_negative_time() {
        const CONTAINER_START: f64 = 1.0;
        let packet = parse_packet_line("0.500000,0.500000,24,__", CONTAINER_START).unwrap();
        assert_eq!(packet.time, -0.5);
    }

    #[test]
    fn a_video_of_one_packet_ends_at_that_packet() {
        // With a single frame there is no typical frame duration to add.
        assert_eq!(estimate_video_end(&packets_at(&[2.5])), 2.5);
    }

    #[test]
    fn a_late_first_packet_does_not_lengthen_the_last_frame() {
        assert_eq!(estimate_video_end(&packets_at(&[10.0, 10.5, 11.0])), 11.5);
    }

    #[test]
    fn an_uneven_frame_rate_uses_the_average_frame_duration() {
        // One second over two intervals is half a second per frame.
        assert_eq!(estimate_video_end(&packets_at(&[0.0, 0.25, 1.0])), 1.5);
    }
}
