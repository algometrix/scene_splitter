//! Checks the scene files of an exact split against the original.
//!
//! A split that copies everything can be compared with the original packet
//! for packet. An exact split encodes the pictures around each cut again, and
//! those cannot be compared that way. So this check decodes the original and
//! every scene from start to end and proves what can be proven:
//!
//! - every scene decodes to its end without an error,
//! - each scene has exactly the pictures the plan gave it, so together the
//!   scenes have as many pictures as the original and each scene starts on
//!   the picture that was chosen,
//! - every picture that was copied decodes to the very same picture as in
//!   the original,
//! - every picture that was encoded again looks like its picture in the
//!   original, and not like the one before or after it, as far as small
//!   grey copies of them can tell,
//! - each scene is to be shown the way the original is: the same size,
//!   shape, turn and colours,
//! - the pictures of each scene are shown at the same pace as in the
//!   original, also across the joins,
//! - the scenes, one after another, hold every packet of the original's
//!   sound, unchanged and in order, each in the scene in which it starts,
//! - the sound of each scene is in step with its pictures as in the original.
//!
//! What it cannot prove is that the pictures that were encoded again look
//! the same as the original's down to the detail. The small copies tell a
//! wrong picture from the right one, not a slightly worse one from a good
//! one. Two pictures in a row that show the same thing cannot be told apart
//! either, which then does not matter to the eye.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::exact_cut::{ExactCutPlan, ScenePlan};
use crate::ffmpeg::{FfmpegError, NotUnderstood, Program, QUICK_ANSWER_LIMIT, STEADY_OUTPUT_LIMIT};
use crate::tools;

/// How far the time of a picture, measured from its scene's first picture,
/// may be from the same measure in the original. Matroska files keep times
/// in whole milliseconds, so two rounded times can differ this much.
const PICTURE_TIMING_TOLERANCE_SECONDS: f64 = 0.002;
/// How far the sound of a scene may be out of step with its pictures,
/// compared with the original. MP4 files keep the moment a track starts in
/// whole milliseconds, and Matroska files every time.
const SOUND_STEP_TOLERANCE_SECONDS: f64 = 0.002;

/// The first output stream of a read-back is the decoded picture, the rest
/// are the sound streams.
const PICTURE_STREAM: u32 = 0;

/// A small copy has this many cells along each side.
const SMALL_COPY_SIDE: usize = 16;
const SMALL_COPY_CELLS: usize = SMALL_COPY_SIDE * SMALL_COPY_SIDE;
/// How far a picture that was encoded again may be from its picture in the
/// original, as the average difference between the cells of their small
/// copies, in steps of brightness out of 255. Encoding the test videos again
/// gives 0.2. A picture with its range of brightness read wrongly, or
/// another picture altogether, is several steps away.
const LARGEST_DIFFERENCE_FROM_ITS_OWN_PICTURE: f64 = 3.0;
/// A picture that is this much more like the one before or after its own in
/// the original was taken from the wrong place. In the test videos the
/// picture next to the right one is 0.9 further away than the right one.
const CLEARLY_MORE_LIKE_A_NEIGHBOUR: f64 = 0.5;
/// What ffprobe says about how the pictures of a file are to be shown. The
/// lines that begin with one of these have to be the same for a scene and
/// the original.
const SHOWN_AS_PROPERTIES: [&str; 9] = [
    "width=",
    "height=",
    "sample_aspect_ratio=",
    "color_range=",
    "color_space=",
    "color_transfer=",
    "color_primaries=",
    // How far to turn the pictures for showing, as older and newer versions
    // of ffprobe print it.
    "TAG:rotate=",
    "rotation=",
];
const SMALL_COPIES_OF_THE_ORIGINAL_FILE: &str = "small_copies_original.gray";
/// The longest condition that names the pictures to make small copies of.
/// Every cut adds about 30 characters to it, and Windows takes a command
/// line of some 32,000 characters at most. A video with more cuts than fit
/// has all its pictures copied small instead, at 256 bytes a picture.
const LONGEST_SELECT_CONDITION: usize = 8_000;

/// One decoded picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodedPicture {
    pub shown_at: f64,
    /// Of the decoded pixels, so two pictures with the same checksum look
    /// the same.
    pub checksum: u32,
}

/// One packet of compressed sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundPacket {
    pub starts_at: f64,
    pub size_bytes: u32,
    pub checksum: u32,
}

impl SoundPacket {
    fn holds_the_same_sound_as(&self, other: &Self) -> bool {
        self.size_bytes == other.size_bytes && self.checksum == other.checksum
    }
}

/// A picture shrunk to a few grey cells, each the average brightness of its
/// part of the picture. Two pictures that show the same thing have nearly
/// the same small copy, however they were encoded.
#[derive(Debug, Clone, PartialEq)]
pub struct SmallCopy(Vec<u8>);

impl SmallCopy {
    /// The average difference between the cells of two small copies, in
    /// steps of brightness out of 255.
    fn difference_from(&self, other: &Self) -> f64 {
        let total: u32 = self
            .0
            .iter()
            .zip(&other.0)
            .map(|(&cell, &other_cell)| u32::from(cell.abs_diff(other_cell)))
            .sum();
        f64::from(total) / SMALL_COPY_CELLS as f64
    }
}

/// How the pictures of a file are to be shown: their size, shape, turn and
/// colours, as ffprobe states them, one property to a line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShownAs(Vec<String>);

impl ShownAs {
    /// The lines of ffprobe's description of a video stream that say how
    /// its pictures are to be shown.
    fn from_stream_description(description: &str) -> Self {
        let properties = description
            .lines()
            .map(str::trim)
            .filter(|line| {
                SHOWN_AS_PROPERTIES
                    .iter()
                    .any(|property| line.starts_with(property))
            })
            .map(str::to_owned)
            .collect();
        Self(properties)
    }

    /// The first property stated here that `other` does not state the same
    /// way, or else the first that only `other` states.
    fn first_difference_from<'properties>(
        &'properties self,
        other: &'properties Self,
    ) -> Option<&'properties str> {
        let only_here = self.0.iter().find(|property| !other.0.contains(property));
        let only_there = other.0.iter().find(|property| !self.0.contains(property));
        only_here.or(only_there).map(String::as_str)
    }
}

/// What reading a video from start to end gave: every picture decoded, and
/// every packet of every sound stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReadBack {
    pub pictures: Vec<DecodedPicture>,
    pub sound: BTreeMap<u32, Vec<SoundPacket>>,
    /// Small copies of the pictures that were asked for, by their place
    /// among the pictures of the file, counted from zero.
    pub small_copies: BTreeMap<usize, SmallCopy>,
    pub shown_as: ShownAs,
}

/// Where one cut was asked for and where the scene after it starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutLanding {
    pub asked_for: f64,
    pub landed_at: f64,
}

/// What the check of an exact split measured, all of it within the limits.
#[derive(Debug, Clone, PartialEq)]
pub struct ExactCutProof {
    /// Pictures in the original, and in the scenes together.
    pub pictures: usize,
    pub pictures_encoded_again: usize,
    /// How many scenes have no stretch that could be copied and so were
    /// encoded again from their first picture to their last.
    pub scenes_encoded_as_a_whole: usize,
    /// The most a picture that was encoded again differs from its picture
    /// in the original, as their small copies measure it.
    pub largest_picture_difference: f64,
    /// Packets of sound in the original, and in the scenes together.
    pub sound_packets: usize,
    pub cut_landings: Vec<CutLanding>,
    /// The most the sound of any scene is out of step with its pictures,
    /// compared with the original.
    pub largest_sound_shift_seconds: f64,
}

/// The first thing about the scene files that does not hold. Scenes and
/// pictures are counted from one, as people count them.
#[derive(Debug, Clone, PartialEq)]
pub enum NotProven {
    OriginalPictureCount {
        stored: usize,
        decoded: usize,
    },
    SceneCount {
        planned: usize,
        written: usize,
    },
    ScenePictureCount {
        scene: usize,
        planned: usize,
        decoded: usize,
    },
    CopiedPictureDiffers {
        scene: usize,
        picture: usize,
    },
    /// A picture that was encoded again was not among those the read-back
    /// made small copies of, so it could not be compared.
    EncodedPictureNotCompared {
        scene: usize,
        picture: usize,
    },
    EncodedPictureDiffers {
        scene: usize,
        picture: usize,
        difference: f64,
    },
    /// A picture that was encoded again is more like the picture before or
    /// after its own in the original than like its own.
    EncodedPictureIsAnother {
        scene: usize,
        picture: usize,
    },
    ShownDifferently {
        scene: usize,
        property: String,
    },
    PictureTiming {
        scene: usize,
        picture: usize,
        seconds_off: f64,
    },
    CutLanding {
        scene: usize,
        asked_for: f64,
        landed_at: f64,
    },
    SoundStreams {
        scene: usize,
        stream: u32,
    },
    SoundPacketCount {
        stream: u32,
        original: usize,
        scenes: usize,
    },
    SoundPacketDiffers {
        stream: u32,
        packet: usize,
    },
    SoundInWrongScene {
        scene: usize,
        stream: u32,
    },
    SoundOutOfStep {
        scene: usize,
        stream: u32,
        seconds_off: f64,
    },
}

impl fmt::Display for NotProven {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OriginalPictureCount { stored, decoded } => write!(
                formatter,
                "the original stores {stored} pictures but decodes to {decoded}"
            ),
            Self::SceneCount { planned, written } => write!(
                formatter,
                "{planned} scenes were planned but {written} were written"
            ),
            Self::ScenePictureCount {
                scene,
                planned,
                decoded,
            } => write!(
                formatter,
                "scene {scene} should have {planned} pictures but decodes to {decoded}"
            ),
            Self::CopiedPictureDiffers { scene, picture } => write!(
                formatter,
                "picture {picture} of scene {scene} was copied but does not decode to the same picture as in the original"
            ),
            Self::EncodedPictureNotCompared { scene, picture } => write!(
                formatter,
                "picture {picture} of scene {scene} was encoded again but could not be compared with the original"
            ),
            Self::EncodedPictureDiffers {
                scene,
                picture,
                difference,
            } => write!(
                formatter,
                "picture {picture} of scene {scene} was encoded again and does not look like its picture in the original (they differ by {difference:.1} of 255 steps of brightness)"
            ),
            Self::EncodedPictureIsAnother { scene, picture } => write!(
                formatter,
                "picture {picture} of scene {scene} was encoded again and looks like the picture next to its own in the original"
            ),
            Self::ShownDifferently { scene, property } => write!(
                formatter,
                "scene {scene} is not to be shown the way the original is ({property} is not the same in both)"
            ),
            Self::PictureTiming {
                scene,
                picture,
                seconds_off,
            } => write!(
                formatter,
                "picture {picture} of scene {scene} is shown {seconds_off:.4} seconds away from its time in the original"
            ),
            Self::CutLanding {
                scene,
                asked_for,
                landed_at,
            } => write!(
                formatter,
                "scene {scene} was to start at {asked_for:.3} seconds but starts at {landed_at:.3}"
            ),
            Self::SoundStreams { scene, stream } => write!(
                formatter,
                "scene {scene} has a sound stream {stream} that the original lacks"
            ),
            Self::SoundPacketCount {
                stream,
                original,
                scenes,
            } => write!(
                formatter,
                "sound stream {stream} has {original} packets in the original but {scenes} in the scenes"
            ),
            Self::SoundPacketDiffers { stream, packet } => write!(
                formatter,
                "packet {packet} of sound stream {stream} differs from the original"
            ),
            Self::SoundInWrongScene { scene, stream } => write!(
                formatter,
                "scene {scene} holds sound of stream {stream} that belongs to another scene"
            ),
            Self::SoundOutOfStep {
                scene,
                stream,
                seconds_off,
            } => write!(
                formatter,
                "sound stream {stream} of scene {scene} is {seconds_off:.4} seconds out of step with the picture"
            ),
        }
    }
}

/// Why an exact split could not be checked, or did not pass.
#[derive(Debug)]
pub enum VerifyError {
    /// The file could not be decoded from start to end, or not described.
    DoesNotDecode {
        file: PathBuf,
        source: FfmpegError,
    },
    /// The small copies the read-back wrote could not be read from disk.
    SmallCopiesUnreadable {
        file: PathBuf,
        source: io::Error,
    },
    NotProven(NotProven),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DoesNotDecode { file, source } => {
                write!(
                    formatter,
                    "{} could not be read back from start to end: {source}",
                    file.display()
                )
            }
            Self::SmallCopiesUnreadable { file, source } => {
                write!(formatter, "could not read {}: {source}", file.display())
            }
            Self::NotProven(what) => write!(formatter, "{what}"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// The scene files of an exact split, while they wait to be checked.
#[derive(Debug, Clone, Copy)]
pub struct WrittenScenes<'split> {
    /// One file for every scene of the video, in scene order.
    pub files: &'split [PathBuf],
    /// A folder the check may write into. Whoever made it removes it.
    pub scratch_folder: &'split Path,
}

/// Whether a picture that decodes with an error ends the reading.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DecodeErrors {
    /// For the original: it is what it is, and the comparison shows whether
    /// the scenes match it.
    ReadOn,
    /// For a scene file: it must decode cleanly.
    Fail,
}

/// Decodes the original and every scene file from start to end and checks
/// the scenes against the plan and the original. `cut_times` are the moments
/// the cuts were asked for, one per scene after the first.
pub fn verify_exact_split(
    original: &Path,
    written: WrittenScenes,
    plan: &ExactCutPlan,
    cut_times: &[f64],
) -> Result<ExactCutProof, VerifyError> {
    // Checked before anything is read: a scene file without a plan could not
    // be read back by one.
    if written.files.len() != plan.scenes().len() {
        return Err(VerifyError::NotProven(NotProven::SceneCount {
            planned: plan.scenes().len(),
            written: written.files.len(),
        }));
    }
    let original_read = read_back(&FileToReadBack {
        video: original,
        decode_errors: DecodeErrors::ReadOn,
        to_compare: PicturesToCompare::in_the_original(plan),
        small_copies_file: written
            .scratch_folder
            .join(SMALL_COPIES_OF_THE_ORIGINAL_FILE),
    })?;
    let scenes_read = written
        .files
        .par_iter()
        .zip(plan.scenes())
        .map(|(scene_file, scene_plan)| {
            read_back(&FileToReadBack {
                video: scene_file,
                decode_errors: DecodeErrors::Fail,
                to_compare: PicturesToCompare::in_a_scene(scene_plan),
                small_copies_file: scene_file.with_extension(SMALL_COPIES_EXTENSION),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    prove(plan, cut_times, &original_read, &scenes_read).map_err(VerifyError::NotProven)
}

/// The pictures of one file that a read-back makes small copies of, by
/// their place among the pictures of that file, counted from zero.
#[derive(Debug, Clone, PartialEq, Default)]
struct PicturesToCompare(BTreeSet<usize>);

impl PicturesToCompare {
    /// Every picture of the video that the plan encodes again, and the one
    /// before and after each, to tell a picture from its neighbours.
    fn in_the_original(plan: &ExactCutPlan) -> Self {
        let last_picture = plan.frame_count().saturating_sub(1);
        let pictures = plan
            .scenes()
            .iter()
            .flat_map(pictures_encoded_again)
            .flat_map(|picture| picture.saturating_sub(1)..=(picture + 1).min(last_picture))
            .collect();
        let around_the_cuts = Self(pictures);
        let fits_a_command_line = around_the_cuts
            .as_select_condition()
            .is_none_or(|condition| condition.len() <= LONGEST_SELECT_CONDITION);
        if fits_a_command_line {
            around_the_cuts
        } else {
            // Every picture is one run, which takes one short condition.
            Self((0..plan.frame_count()).collect())
        }
    }

    /// The pictures of one scene that were encoded again, counted from the
    /// first picture of the scene.
    fn in_a_scene(scene: &ScenePlan) -> Self {
        let first_of_scene = scene.frames.first();
        let pictures = pictures_encoded_again(scene)
            .map(|picture| picture - first_of_scene)
            .collect();
        Self(pictures)
    }

    /// The condition by which ffmpeg's `select` filter keeps exactly these
    /// pictures, as one `between` for every run of them. `None` when there
    /// is nothing to keep.
    fn as_select_condition(&self) -> Option<String> {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for &picture in &self.0 {
            match runs.last_mut() {
                Some((_, last)) if *last + 1 == picture => *last = picture,
                _ => runs.push((picture, picture)),
            }
        }
        let conditions: Vec<String> = runs
            .iter()
            .map(|(first, last)| format!("between(n,{first},{last})"))
            .collect();
        (!conditions.is_empty()).then(|| conditions.join("+"))
    }
}

/// The pictures of the video that a scene has encoded again, by their place
/// among the pictures of the whole video.
fn pictures_encoded_again(scene: &ScenePlan) -> impl Iterator<Item = usize> + '_ {
    scene
        .parts
        .iter()
        .filter(|part| !part.is_copied())
        .flat_map(|part| part.frames.frames())
}

const SMALL_COPIES_EXTENSION: &str = "gray";

/// One file to read back, and what to bring back from it.
struct FileToReadBack<'check> {
    video: &'check Path,
    decode_errors: DecodeErrors,
    to_compare: PicturesToCompare,
    /// Where ffmpeg writes the small copies before they are read in.
    small_copies_file: PathBuf,
}

/// Has ffmpeg decode every picture and list a checksum of it, and list a
/// checksum of every packet of sound without decoding it. In the same run
/// it writes small copies of the pictures that are to be compared by eye,
/// so to speak, since the video is decoded only once.
fn read_back(file: &FileToReadBack) -> Result<ReadBack, VerifyError> {
    let could_not_read = |source| VerifyError::DoesNotDecode {
        file: file.video.to_path_buf(),
        source,
    };
    let mut ffmpeg = tools::command(Program::Ffmpeg).args(["-nostdin", "-v", "error", "-y"]);
    if file.decode_errors == DecodeErrors::Fail {
        // ffmpeg is told to stop at the first error. It still carries on
        // after some pictures it could not decode properly and only writes
        // about them, so anything it writes counts as a failure too.
        ffmpeg = ffmpeg.arg("-xerror").failing_on_complaints();
    }
    // Every decoded picture is listed once, with the time the file gives
    // it. Without these ffmpeg drops or repeats pictures to even out their
    // times, and rounds the times to whole pictures.
    let picture_times = tools::picture_times_option().arguments();
    ffmpeg = ffmpeg
        .arg("-i")
        .arg(file.video)
        .args(["-map", "0:v:0", "-map", "0:a?", "-c:a", "copy"])
        .args(picture_times)
        .args(["-f", "framecrc", "-"]);
    let select_condition = file.to_compare.as_select_condition();
    if let Some(condition) = &select_condition {
        ffmpeg = ffmpeg
            .args(["-map", "0:v:0", "-vf", &small_copy_filters(condition)])
            .args(picture_times)
            .args(["-f", "rawvideo"])
            .arg(&file.small_copies_file);
    }

    let mut listing = ReadBackListing::default();
    ffmpeg
        .stream_lines(STEADY_OUTPUT_LIMIT, |line| {
            listing
                .take_line(line)
                .map_err(|problem| NotUnderstood(format!("checksum line {line:?}, {problem}")))
        })
        .map_err(could_not_read)?;
    let mut read = listing.read;

    if select_condition.is_some() {
        let cells = std::fs::read(&file.small_copies_file).map_err(|source| {
            VerifyError::SmallCopiesUnreadable {
                file: file.small_copies_file.clone(),
                source,
            }
        })?;
        read.small_copies = small_copies_from(&file.to_compare, &cells).map_err(|detail| {
            could_not_read(FfmpegError::OutputNotUnderstood {
                program: Program::Ffmpeg,
                detail,
            })
        })?;
    }
    read.shown_as = read_shown_as(file.video).map_err(could_not_read)?;
    Ok(read)
}

/// The filters that turn the pictures `select_condition` keeps into small
/// copies: shrunk by averaging, and grey.
fn small_copy_filters(select_condition: &str) -> String {
    format!(
        "select='{select_condition}',scale={SMALL_COPY_SIDE}:{SMALL_COPY_SIDE}:flags=area,format=gray"
    )
}

/// Gives each picture that was to be compared its small copy. ffmpeg wrote
/// them one after another, in the order the pictures are shown.
fn small_copies_from(
    compared: &PicturesToCompare,
    cells: &[u8],
) -> Result<BTreeMap<usize, SmallCopy>, String> {
    if cells.len() != compared.0.len() * SMALL_COPY_CELLS {
        return Err(format!(
            "expected small copies of {} pictures, which is {} bytes, and got {} bytes",
            compared.0.len(),
            compared.0.len() * SMALL_COPY_CELLS,
            cells.len()
        ));
    }
    // The length was just checked, so nothing is left over.
    let (small_copies, _left_over) = cells.as_chunks::<SMALL_COPY_CELLS>();
    Ok(compared
        .0
        .iter()
        .zip(small_copies)
        .map(|(&picture, cells)| (picture, SmallCopy(cells.to_vec())))
        .collect())
}

/// Asks ffprobe how the pictures of the video are to be shown.
fn read_shown_as(video: &Path) -> Result<ShownAs, FfmpegError> {
    let description = tools::command(Program::Ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_streams"])
        .arg(video)
        .run_for_text(QUICK_ANSWER_LIMIT)?;
    Ok(ShownAs::from_stream_description(&description))
}

/// Collects the lines of a checksum listing. Header lines say how long one
/// step of each stream's clock is, the other lines each describe a picture
/// or a packet.
#[derive(Default)]
struct ReadBackListing {
    seconds_per_tick: BTreeMap<u32, f64>,
    read: ReadBack,
}

impl ReadBackListing {
    fn take_line(&mut self, line: &str) -> Result<(), String> {
        if line.trim().is_empty() {
            return Ok(());
        }
        if let Some(header) = line.strip_prefix('#') {
            if let Some((stream, seconds_per_tick)) = parse_time_base(header)? {
                self.seconds_per_tick.insert(stream, seconds_per_tick);
            }
            return Ok(());
        }

        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let [stream, _decoded_at, shown_at, _duration, size, checksum, ..] = fields[..] else {
            return Err("expected at least six fields".to_owned());
        };
        let stream: u32 = stream.parse().map_err(|_| "stream is not a number")?;
        let ticks: i64 = shown_at.parse().map_err(|_| "time is not a number")?;
        let size_bytes: u32 = size.parse().map_err(|_| "size is not a number")?;
        let checksum = checksum
            .strip_prefix("0x")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .ok_or("checksum is not hexadecimal")?;
        let seconds_per_tick = self
            .seconds_per_tick
            .get(&stream)
            .ok_or("the stream's clock was not announced")?;
        let time = ticks as f64 * seconds_per_tick;

        if stream == PICTURE_STREAM {
            self.read.pictures.push(DecodedPicture {
                shown_at: time,
                checksum,
            });
        } else {
            self.read
                .sound
                .entry(stream)
                .or_default()
                .push(SoundPacket {
                    starts_at: time,
                    size_bytes,
                    checksum,
                });
        }
        Ok(())
    }
}

/// Reads a header line such as "tb 1: 1/44100" and returns the stream and
/// the seconds per step of its clock. `None` for any other header line.
fn parse_time_base(header: &str) -> Result<Option<(u32, f64)>, String> {
    let Some(time_base) = header.strip_prefix("tb ") else {
        return Ok(None);
    };
    let unreadable = || format!("unreadable clock {header:?}");
    let (stream, fraction) = time_base.split_once(':').ok_or_else(unreadable)?;
    let (numerator, denominator) = fraction.trim().split_once('/').ok_or_else(unreadable)?;
    let stream: u32 = stream.trim().parse().map_err(|_| unreadable())?;
    let numerator: f64 = numerator.parse().map_err(|_| unreadable())?;
    let denominator: f64 = denominator.parse().map_err(|_| unreadable())?;
    if numerator <= 0.0 || denominator <= 0.0 {
        return Err(unreadable());
    }
    Ok(Some((stream, numerator / denominator)))
}

/// Checks what was read back from the scenes against the plan and against
/// what was read back from the original. Returns the first thing that does
/// not hold.
pub fn prove(
    plan: &ExactCutPlan,
    cut_times: &[f64],
    original: &ReadBack,
    scenes: &[ReadBack],
) -> Result<ExactCutProof, NotProven> {
    if original.pictures.len() != plan.frame_count() {
        return Err(NotProven::OriginalPictureCount {
            stored: plan.frame_count(),
            decoded: original.pictures.len(),
        });
    }
    if scenes.len() != plan.scenes().len() {
        return Err(NotProven::SceneCount {
            planned: plan.scenes().len(),
            written: scenes.len(),
        });
    }
    prove_pictures(plan, original, scenes)?;
    let largest_picture_difference = prove_encoded_pictures(plan, original, scenes)?;
    prove_shown_alike(original, scenes)?;
    let cut_landings = prove_cut_landings(plan, cut_times, original)?;
    let largest_sound_shift_seconds = prove_sound(plan, original, scenes)?;
    Ok(ExactCutProof {
        pictures: original.pictures.len(),
        pictures_encoded_again: plan.frames_encoded_again(),
        scenes_encoded_as_a_whole: plan.scenes_encoded_as_a_whole(),
        largest_picture_difference,
        sound_packets: original.sound.values().map(Vec::len).sum(),
        cut_landings,
        largest_sound_shift_seconds,
    })
}

/// Each scene has the pictures the plan gave it, shown at the pace of the
/// original, and those that were copied are the original's own.
fn prove_pictures(
    plan: &ExactCutPlan,
    original: &ReadBack,
    scenes: &[ReadBack],
) -> Result<(), NotProven> {
    for (position, (scene_plan, scene)) in plan.scenes().iter().zip(scenes).enumerate() {
        let scene_number = position + 1;
        let planned = scene_plan.frames;
        if scene.pictures.len() != planned.len() {
            return Err(NotProven::ScenePictureCount {
                scene: scene_number,
                planned: planned.len(),
                decoded: scene.pictures.len(),
            });
        }
        let original_pictures = &original.pictures[planned.frames()];

        let scene_start = scene.pictures[0].shown_at;
        let original_start = original_pictures[0].shown_at;
        for (place, (in_scene, in_original)) in
            scene.pictures.iter().zip(original_pictures).enumerate()
        {
            let seconds_off =
                (in_scene.shown_at - scene_start) - (in_original.shown_at - original_start);
            if seconds_off.abs() > PICTURE_TIMING_TOLERANCE_SECONDS {
                return Err(NotProven::PictureTiming {
                    scene: scene_number,
                    picture: place + 1,
                    seconds_off,
                });
            }
        }

        for part in scene_plan.parts.iter().filter(|part| part.is_copied()) {
            let differing = part.frames.frames().find(|&frame| {
                scene.pictures[frame - planned.first()].checksum
                    != original.pictures[frame].checksum
            });
            if let Some(frame) = differing {
                return Err(NotProven::CopiedPictureDiffers {
                    scene: scene_number,
                    picture: frame - planned.first() + 1,
                });
            }
        }
    }
    Ok(())
}

/// Each picture that was encoded again looks like its picture in the
/// original, and not more like the one before or after that. Returns the
/// largest difference from its own picture that was found.
///
/// The count of pictures alone would not show an encoding that began one
/// picture early or late: it would still have as many pictures, shown at the
/// same pace. This is what shows it, wherever two pictures in a row differ,
/// as they do where a scene changes.
fn prove_encoded_pictures(
    plan: &ExactCutPlan,
    original: &ReadBack,
    scenes: &[ReadBack],
) -> Result<f64, NotProven> {
    let mut largest_difference: f64 = 0.0;
    for (position, (scene_plan, scene)) in plan.scenes().iter().zip(scenes).enumerate() {
        let scene_number = position + 1;
        let first_of_scene = scene_plan.frames.first();
        for picture in pictures_encoded_again(scene_plan) {
            let place_in_scene = picture - first_of_scene;
            let picture_number = place_in_scene + 1;
            let (Some(encoded), Some(its_own)) = (
                scene.small_copies.get(&place_in_scene),
                original.small_copies.get(&picture),
            ) else {
                return Err(NotProven::EncodedPictureNotCompared {
                    scene: scene_number,
                    picture: picture_number,
                });
            };
            let difference = encoded.difference_from(its_own);
            if difference > LARGEST_DIFFERENCE_FROM_ITS_OWN_PICTURE {
                return Err(NotProven::EncodedPictureDiffers {
                    scene: scene_number,
                    picture: picture_number,
                    difference,
                });
            }
            let neighbours = [picture.checked_sub(1), picture.checked_add(1)];
            let is_more_like_a_neighbour = neighbours
                .into_iter()
                .flatten()
                .filter_map(|neighbour| original.small_copies.get(&neighbour))
                .any(|neighbour| {
                    encoded.difference_from(neighbour) + CLEARLY_MORE_LIKE_A_NEIGHBOUR < difference
                });
            if is_more_like_a_neighbour {
                return Err(NotProven::EncodedPictureIsAnother {
                    scene: scene_number,
                    picture: picture_number,
                });
            }
            largest_difference = largest_difference.max(difference);
        }
    }
    Ok(largest_difference)
}

/// Each scene is to be shown the way the original is. A scene that begins
/// with pictures that were encoded again takes these properties from the
/// encoder, which is where they could come out differently.
fn prove_shown_alike(original: &ReadBack, scenes: &[ReadBack]) -> Result<(), NotProven> {
    for (position, scene) in scenes.iter().enumerate() {
        if let Some(property) = original.shown_as.first_difference_from(&scene.shown_as) {
            return Err(NotProven::ShownDifferently {
                scene: position + 1,
                property: property.to_owned(),
            });
        }
    }
    Ok(())
}

/// Each scene after the first starts on the first picture at or after the
/// moment its cut was asked for, so less than one picture after it.
fn prove_cut_landings(
    plan: &ExactCutPlan,
    cut_times: &[f64],
    original: &ReadBack,
) -> Result<Vec<CutLanding>, NotProven> {
    let scenes_after_the_first = plan.scenes().iter().enumerate().skip(1);
    let mut landings = Vec::new();
    for ((position, scene_plan), &asked_for) in scenes_after_the_first.zip(cut_times) {
        let first = scene_plan.frames.first();
        let landed_at = original.pictures[first].shown_at;
        let picture_before = original.pictures[first - 1].shown_at;
        let is_first_picture_at_or_after = landed_at + PICTURE_TIMING_TOLERANCE_SECONDS
            >= asked_for
            && picture_before < asked_for + PICTURE_TIMING_TOLERANCE_SECONDS;
        if !is_first_picture_at_or_after {
            return Err(NotProven::CutLanding {
                scene: position + 1,
                asked_for,
                landed_at,
            });
        }
        landings.push(CutLanding {
            asked_for,
            landed_at,
        });
    }
    Ok(landings)
}

/// The scenes hold the original's sound, packet for packet, each packet in
/// the scene in which it starts and in step with the pictures. Returns the
/// largest shift between sound and pictures that was found.
fn prove_sound(
    plan: &ExactCutPlan,
    original: &ReadBack,
    scenes: &[ReadBack],
) -> Result<f64, NotProven> {
    for (position, scene) in scenes.iter().enumerate() {
        if let Some(&stream) = scene
            .sound
            .keys()
            .find(|stream| !original.sound.contains_key(stream))
        {
            return Err(NotProven::SoundStreams {
                scene: position + 1,
                stream,
            });
        }
    }

    let mut largest_shift: f64 = 0.0;
    let no_packets = Vec::new();
    for (&stream, original_packets) in &original.sound {
        let packets_in_scenes: usize = scenes
            .iter()
            .map(|scene| scene.sound.get(&stream).map_or(0, Vec::len))
            .sum();
        if packets_in_scenes != original_packets.len() {
            return Err(NotProven::SoundPacketCount {
                stream,
                original: original_packets.len(),
                scenes: packets_in_scenes,
            });
        }

        let mut packets_before = 0;
        for (position, (scene_plan, scene)) in plan.scenes().iter().zip(scenes).enumerate() {
            let scene_number = position + 1;
            let in_scene = scene.sound.get(&stream).unwrap_or(&no_packets);
            let in_original = &original_packets[packets_before..packets_before + in_scene.len()];
            let differing = in_scene
                .iter()
                .zip(in_original)
                .position(|(scene, original)| !scene.holds_the_same_sound_as(original));
            if let Some(place) = differing {
                return Err(NotProven::SoundPacketDiffers {
                    stream,
                    packet: packets_before + place + 1,
                });
            }
            packets_before += in_scene.len();

            let (Some(first), Some(last)) = (in_original.first(), in_original.last()) else {
                continue;
            };
            let starts_too_early = scene_plan.sound_from.is_some_and(|from| {
                first.starts_at < from.seconds() - SOUND_STEP_TOLERANCE_SECONDS
            });
            let ends_too_late = scene_plan.sound_until.is_some_and(|until| {
                last.starts_at >= until.seconds() + SOUND_STEP_TOLERANCE_SECONDS
            });
            if starts_too_early || ends_too_late {
                return Err(NotProven::SoundInWrongScene {
                    scene: scene_number,
                    stream,
                });
            }

            // Every packet, not only the first and the last: sound that
            // drifted and came back by the end of the scene is out of step
            // all the same.
            let scene_start = scene.pictures[0].shown_at;
            let original_start = original.pictures[scene_plan.frames.first()].shown_at;
            for (packet_in_scene, packet_in_original) in in_scene.iter().zip(in_original) {
                let seconds_off = (packet_in_scene.starts_at - scene_start)
                    - (packet_in_original.starts_at - original_start);
                if seconds_off.abs() > SOUND_STEP_TOLERANCE_SECONDS {
                    return Err(NotProven::SoundOutOfStep {
                        scene: scene_number,
                        stream,
                        seconds_off,
                    });
                }
                largest_shift = largest_shift.max(seconds_off.abs());
            }
        }
    }
    Ok(largest_shift)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exact_cut::{plan_exact_cuts, FrameTable, StoredFrame};
    use crate::scenes::CutTimes;
    use crate::timecode::TimeSpan;

    const FRAME_SECONDS: f64 = 0.04;
    const FRAME_COUNT: usize = 225;
    const KEYFRAME_EVERY: usize = 50;
    const CUT_FRAME: usize = 73;
    const SOUND_STREAM: u32 = 1;
    const SOUND_PACKET_SECONDS: f64 = 0.025;
    /// The sound of the test video starts a little before its pictures, as
    /// the sound of real files often does.
    const SOUND_LEAD_SECONDS: f64 = 0.01;
    /// The small copy of each picture of the test video is evenly grey but
    /// for one bright cell, which is another cell in every picture.
    const GREY: u8 = 100;
    const BRIGHT: u8 = 200;
    /// How far encoding again moves every cell of a small copy. With this,
    /// a picture is 1.0 from its own in the original and 1.8 from the one
    /// next to that.
    const ENCODING_LOSS: u8 = 1;
    const SHOWN_AS_IN_THE_ORIGINAL: &str = "width=320\nheight=180\nsample_aspect_ratio=1:1\n\
        color_range=tv\ncolor_space=bt709\n";

    fn small_copy_of(frame: usize) -> SmallCopy {
        let mut cells = vec![GREY; SMALL_COPY_CELLS];
        cells[frame] = BRIGHT;
        SmallCopy(cells)
    }

    fn after_encoding_again(small_copy: &SmallCopy) -> SmallCopy {
        SmallCopy(
            small_copy
                .0
                .iter()
                .map(|cell| cell + ENCODING_LOSS)
                .collect(),
        )
    }

    fn time_of(frame: usize) -> f64 {
        frame as f64 * FRAME_SECONDS
    }

    fn cut_time() -> f64 {
        time_of(CUT_FRAME)
    }

    /// A plan for a video cut in two on picture 73, between keyframes.
    fn plan() -> ExactCutPlan {
        let stored = (0..FRAME_COUNT)
            .map(|frame| StoredFrame {
                shown_at: time_of(frame),
                decoded_at: Some(time_of(frame) - 2.0 * FRAME_SECONDS),
                reorder_ticks: Some(1024),
                is_keyframe: frame % KEYFRAME_EVERY == 0,
            })
            .collect();
        let table = FrameTable::from_stored_order(stored).unwrap();
        let whole_video = TimeSpan {
            start: 0.0,
            end: time_of(FRAME_COUNT),
        };
        let cuts = CutTimes::inside(whole_video, vec![cut_time()]).unwrap();
        plan_exact_cuts(&table, &cuts).unwrap()
    }

    fn original() -> ReadBack {
        let pictures = (0..FRAME_COUNT)
            .map(|frame| DecodedPicture {
                shown_at: time_of(frame),
                checksum: frame as u32,
            })
            .collect();
        let packet_count = (time_of(FRAME_COUNT) / SOUND_PACKET_SECONDS) as usize;
        let packets = (0..packet_count)
            .map(|packet| SoundPacket {
                starts_at: packet as f64 * SOUND_PACKET_SECONDS - SOUND_LEAD_SECONDS,
                size_bytes: 200,
                checksum: 1000 + packet as u32,
            })
            .collect();
        let small_copies = (0..FRAME_COUNT)
            .map(|frame| (frame, small_copy_of(frame)))
            .collect();
        ReadBack {
            pictures,
            sound: BTreeMap::from([(SOUND_STREAM, packets)]),
            small_copies,
            shown_as: ShownAs::from_stream_description(SHOWN_AS_IN_THE_ORIGINAL),
        }
    }

    /// The scene files a correct split of `original` at picture 73 would
    /// read back as. Each scene file counts time from its own start. The
    /// pictures that were encoded again get other checksums.
    fn correct_scenes(original: &ReadBack) -> Vec<ReadBack> {
        let encoded_again = KEYFRAME_EVERY..2 * KEYFRAME_EVERY;
        [(0, CUT_FRAME), (CUT_FRAME, FRAME_COUNT)]
            .into_iter()
            .map(|(first, end)| {
                let scene_start = time_of(first);
                let scene_end = time_of(end);
                let pictures = original.pictures[first..end]
                    .iter()
                    .enumerate()
                    .map(|(place, picture)| DecodedPicture {
                        shown_at: picture.shown_at - scene_start,
                        checksum: if encoded_again.contains(&(first + place)) {
                            picture.checksum + 500_000
                        } else {
                            picture.checksum
                        },
                    })
                    .collect();
                let is_first_scene = first == 0;
                let is_last_scene = end == FRAME_COUNT;
                let packets = original.sound[&SOUND_STREAM]
                    .iter()
                    .filter(|packet| {
                        (is_first_scene || packet.starts_at >= scene_start)
                            && (is_last_scene || packet.starts_at < scene_end)
                    })
                    .map(|packet| SoundPacket {
                        starts_at: packet.starts_at - scene_start,
                        ..*packet
                    })
                    .collect();
                let small_copies = (first..end)
                    .filter(|frame| encoded_again.contains(frame))
                    .map(|frame| (frame - first, after_encoding_again(&small_copy_of(frame))))
                    .collect();
                ReadBack {
                    pictures,
                    sound: BTreeMap::from([(SOUND_STREAM, packets)]),
                    small_copies,
                    shown_as: original.shown_as.clone(),
                }
            })
            .collect()
    }

    fn proof_of(scenes: &[ReadBack]) -> Result<ExactCutProof, NotProven> {
        prove(&plan(), &[cut_time()], &original(), scenes)
    }

    #[test]
    fn a_correct_split_is_proven_with_its_measurements() {
        let original = original();
        let proof = proof_of(&correct_scenes(&original)).unwrap();

        assert_eq!(proof.pictures, FRAME_COUNT);
        assert_eq!(proof.pictures_encoded_again, KEYFRAME_EVERY);
        assert_eq!(proof.sound_packets, original.sound[&SOUND_STREAM].len());
        assert_eq!(
            proof.cut_landings,
            vec![CutLanding {
                asked_for: cut_time(),
                landed_at: cut_time(),
            }]
        );
        assert!(proof.largest_sound_shift_seconds < 1e-9);
    }

    #[test]
    fn a_scene_with_a_picture_too_few_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        scenes[1].pictures.pop();
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::ScenePictureCount {
                scene: 2,
                planned: FRAME_COUNT - CUT_FRAME,
                decoded: FRAME_COUNT - CUT_FRAME - 1,
            })
        );
    }

    #[test]
    fn a_picture_moved_from_one_scene_to_the_next_is_not_proven() {
        // The scenes still have every picture between them, but the cut is
        // one picture early.
        let mut scenes = correct_scenes(&original());
        let moved = scenes[0].pictures.pop().unwrap();
        scenes[1].pictures.insert(0, moved);
        assert!(matches!(
            proof_of(&scenes),
            Err(NotProven::ScenePictureCount { scene: 1, .. })
        ));
    }

    #[test]
    fn a_copied_picture_that_decodes_differently_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        // Picture 120 of the video is the 48th of the second scene, copied.
        scenes[1].pictures[120 - CUT_FRAME].checksum += 1;
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::CopiedPictureDiffers {
                scene: 2,
                picture: 120 - CUT_FRAME + 1,
            })
        );
    }

    #[test]
    fn a_picture_that_was_encoded_again_may_decode_differently() {
        let mut scenes = correct_scenes(&original());
        scenes[1].pictures[3].checksum += 1;
        assert!(proof_of(&scenes).is_ok());
    }

    #[test]
    fn a_gap_in_time_at_a_join_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        // From the join at picture 100 on, everything is 10 ms late.
        for picture in &mut scenes[1].pictures[100 - CUT_FRAME..] {
            picture.shown_at += 0.010;
        }
        assert!(matches!(
            proof_of(&scenes),
            Err(NotProven::PictureTiming { scene: 2, picture, .. }) if picture == 100 - CUT_FRAME + 1
        ));
    }

    #[test]
    fn times_rounded_to_milliseconds_are_close_enough() {
        let mut scenes = correct_scenes(&original());
        for scene in &mut scenes {
            for picture in &mut scene.pictures {
                picture.shown_at = (picture.shown_at * 1000.0).round() / 1000.0;
            }
        }
        assert!(proof_of(&scenes).is_ok());
    }

    #[test]
    fn an_original_that_decodes_to_fewer_pictures_than_it_stores_is_not_proven() {
        let mut original = original();
        let scenes = correct_scenes(&original);
        original.pictures.pop();
        assert_eq!(
            prove(&plan(), &[cut_time()], &original, &scenes),
            Err(NotProven::OriginalPictureCount {
                stored: FRAME_COUNT,
                decoded: FRAME_COUNT - 1,
            })
        );
    }

    #[test]
    fn a_missing_scene_file_is_not_proven() {
        let scenes = correct_scenes(&original());
        assert_eq!(
            proof_of(&scenes[..1]),
            Err(NotProven::SceneCount {
                planned: 2,
                written: 1,
            })
        );
    }

    #[test]
    fn a_cut_that_was_asked_for_elsewhere_is_not_proven() {
        let original = original();
        let scenes = correct_scenes(&original);
        // The scene starts on picture 73, but the cut was asked for two
        // pictures later.
        let asked_for = time_of(CUT_FRAME + 2);
        assert_eq!(
            prove(&plan(), &[asked_for], &original, &scenes),
            Err(NotProven::CutLanding {
                scene: 2,
                asked_for,
                landed_at: cut_time(),
            })
        );
    }

    #[test]
    fn a_cut_asked_for_between_two_pictures_lands_on_the_later_one() {
        let original = original();
        let scenes = correct_scenes(&original);
        let asked_for = cut_time() - FRAME_SECONDS / 2.0;
        let proof = prove(&plan(), &[asked_for], &original, &scenes).unwrap();
        assert_eq!(proof.cut_landings[0].landed_at, cut_time());
    }

    #[test]
    fn a_lost_packet_of_sound_is_not_proven() {
        let original = original();
        let mut scenes = correct_scenes(&original);
        scenes[0].sound.get_mut(&SOUND_STREAM).unwrap().pop();
        let packets = original.sound[&SOUND_STREAM].len();
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::SoundPacketCount {
                stream: SOUND_STREAM,
                original: packets,
                scenes: packets - 1,
            })
        );
    }

    #[test]
    fn a_changed_packet_of_sound_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        let packets_in_first_scene = scenes[0].sound[&SOUND_STREAM].len();
        scenes[1].sound.get_mut(&SOUND_STREAM).unwrap()[4].checksum += 1;
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::SoundPacketDiffers {
                stream: SOUND_STREAM,
                packet: packets_in_first_scene + 5,
            })
        );
    }

    #[test]
    fn sound_that_went_into_the_wrong_scene_is_not_proven() {
        // Every packet is there and in order, but the first scene took two
        // packets that start in the second.
        let mut scenes = correct_scenes(&original());
        let scene_seconds = time_of(CUT_FRAME);
        let taken: Vec<SoundPacket> = scenes[1]
            .sound
            .get_mut(&SOUND_STREAM)
            .unwrap()
            .drain(..2)
            .collect();
        for packet in taken {
            scenes[0]
                .sound
                .get_mut(&SOUND_STREAM)
                .unwrap()
                .push(SoundPacket {
                    starts_at: packet.starts_at + scene_seconds,
                    ..packet
                });
        }
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::SoundInWrongScene {
                scene: 1,
                stream: SOUND_STREAM,
            })
        );
    }

    #[test]
    fn sound_that_is_late_against_the_picture_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        for packet in scenes[0].sound.get_mut(&SOUND_STREAM).unwrap() {
            packet.starts_at += 0.023;
        }
        assert!(matches!(
            proof_of(&scenes),
            Err(NotProven::SoundOutOfStep {
                scene: 1,
                stream: SOUND_STREAM,
                ..
            })
        ));
    }

    #[test]
    fn sound_that_drifts_in_the_middle_of_a_scene_is_not_proven() {
        // The first and the last packet of the scene are where they belong.
        let mut scenes = correct_scenes(&original());
        let packets = scenes[1].sound.get_mut(&SOUND_STREAM).unwrap();
        let in_the_middle = packets.len() / 2;
        packets[in_the_middle].starts_at += 0.010;
        assert!(matches!(
            proof_of(&scenes),
            Err(NotProven::SoundOutOfStep {
                scene: 2,
                stream: SOUND_STREAM,
                ..
            })
        ));
    }

    /// The place, in the second scene, of a picture of the video that was
    /// encoded again there.
    const ENCODED_IN_SECOND_SCENE: usize = 80 - CUT_FRAME;

    #[test]
    fn an_encoded_picture_that_looks_like_its_own_is_proven_with_how_far_it_is() {
        let proof = proof_of(&correct_scenes(&original())).unwrap();
        assert!((proof.largest_picture_difference - f64::from(ENCODING_LOSS)).abs() < 1e-9);
    }

    #[test]
    fn an_encoded_picture_that_looks_like_nothing_in_the_original_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        let white = SmallCopy(vec![u8::MAX; SMALL_COPY_CELLS]);
        scenes[1]
            .small_copies
            .insert(ENCODED_IN_SECOND_SCENE, white);
        assert!(matches!(
            proof_of(&scenes),
            Err(NotProven::EncodedPictureDiffers { scene: 2, picture, .. })
                if picture == ENCODED_IN_SECOND_SCENE + 1
        ));
    }

    #[test]
    fn pictures_encoded_from_one_picture_too_late_are_not_proven() {
        // The scene has as many pictures as planned, shown at the right
        // pace. Only what they show is the picture after their own.
        let mut scenes = correct_scenes(&original());
        let encoded_places: Vec<usize> = scenes[1].small_copies.keys().copied().collect();
        for place in encoded_places {
            let one_too_late = after_encoding_again(&small_copy_of(CUT_FRAME + place + 1));
            scenes[1].small_copies.insert(place, one_too_late);
        }
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::EncodedPictureIsAnother {
                scene: 2,
                picture: 1,
            })
        );
    }

    #[test]
    fn pictures_that_all_show_the_same_thing_are_proven_wherever_they_were_taken_from() {
        // Nothing moves in the video, so a picture is no more like its own
        // than like the next one. That is no reason to throw the scenes away.
        let still = SmallCopy(vec![GREY; SMALL_COPY_CELLS]);
        let mut original = original();
        for small_copy in original.small_copies.values_mut() {
            *small_copy = still.clone();
        }
        let mut scenes = correct_scenes(&original);
        for scene in &mut scenes {
            for small_copy in scene.small_copies.values_mut() {
                *small_copy = after_encoding_again(&still);
            }
        }
        assert!(prove(&plan(), &[cut_time()], &original, &scenes).is_ok());
    }

    #[test]
    fn an_encoded_picture_without_a_small_copy_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        scenes[0].small_copies.clear();
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::EncodedPictureNotCompared {
                scene: 1,
                picture: KEYFRAME_EVERY + 1,
            })
        );
    }

    #[test]
    fn a_scene_that_is_to_be_shown_another_way_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        scenes[1].shown_as = ShownAs::from_stream_description(
            "width=320\nheight=180\nsample_aspect_ratio=1:1\ncolor_range=pc\ncolor_space=bt709\n",
        );
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::ShownDifferently {
                scene: 2,
                property: "color_range=tv".to_owned(),
            })
        );
    }

    #[test]
    fn a_scene_that_is_turned_and_the_original_is_not_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        scenes[0].shown_as = ShownAs::from_stream_description(&format!(
            "{SHOWN_AS_IN_THE_ORIGINAL}[SIDE_DATA]\nrotation=-90\n[/SIDE_DATA]\n"
        ));
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::ShownDifferently {
                scene: 1,
                property: "rotation=-90".to_owned(),
            })
        );
    }

    #[test]
    fn how_a_video_is_shown_is_read_from_the_lines_that_say_so() {
        let description = "[STREAM]\nindex=0\ncodec_name=h264\nwidth=1920\nheight=1080\n\
            sample_aspect_ratio=1:1\ndisplay_aspect_ratio=16:9\npix_fmt=yuv420p\n\
            color_range=tv\ncolor_space=bt709\ncolor_transfer=bt709\ncolor_primaries=bt709\n\
            bit_rate=5000000\nTAG:rotate=90\nTAG:handler_name=VideoHandler\n\
            [SIDE_DATA]\nside_data_type=Display Matrix\nrotation=-90\n[/SIDE_DATA]\n[/STREAM]\n";
        assert_eq!(
            ShownAs::from_stream_description(description),
            ShownAs(
                [
                    "width=1920",
                    "height=1080",
                    "sample_aspect_ratio=1:1",
                    "color_range=tv",
                    "color_space=bt709",
                    "color_transfer=bt709",
                    "color_primaries=bt709",
                    "TAG:rotate=90",
                    "rotation=-90",
                ]
                .map(str::to_owned)
                .to_vec()
            )
        );
    }

    #[test]
    fn small_copies_are_asked_for_of_the_encoded_pictures_and_those_beside_them() {
        let plan = plan();
        // Pictures 50 to 99 are encoded again, 50 to 72 in the first scene.
        assert_eq!(
            PicturesToCompare::in_the_original(&plan)
                .as_select_condition()
                .as_deref(),
            Some("between(n,49,100)")
        );
        assert_eq!(
            PicturesToCompare::in_a_scene(&plan.scenes()[0])
                .as_select_condition()
                .as_deref(),
            Some("between(n,50,72)")
        );
        assert_eq!(
            PicturesToCompare::in_a_scene(&plan.scenes()[1])
                .as_select_condition()
                .as_deref(),
            Some("between(n,0,26)")
        );
    }

    #[test]
    fn with_more_cuts_than_a_command_line_holds_every_picture_is_copied_small() {
        // A keyframe every 50 pictures and a cut 23 pictures after every
        // third of them, so the pictures around each cut are a run of their
        // own with copied pictures before and after.
        const CUTS: usize = 600;
        const KEYFRAMES_PER_CUT: usize = 3;
        let frame_count = (CUTS + 1) * KEYFRAMES_PER_CUT * KEYFRAME_EVERY;
        let stored = (0..frame_count)
            .map(|frame| StoredFrame {
                shown_at: time_of(frame),
                decoded_at: Some(time_of(frame) - 2.0 * FRAME_SECONDS),
                reorder_ticks: Some(1024),
                is_keyframe: frame % KEYFRAME_EVERY == 0,
            })
            .collect();
        let table = FrameTable::from_stored_order(stored).unwrap();
        let whole_video = TimeSpan {
            start: 0.0,
            end: time_of(frame_count),
        };
        let cut_times = (1..=CUTS)
            .map(|cut| time_of(cut * KEYFRAMES_PER_CUT * KEYFRAME_EVERY + 23))
            .collect();
        let cuts = CutTimes::inside(whole_video, cut_times).unwrap();
        let plan = plan_exact_cuts(&table, &cuts).unwrap();

        let condition = PicturesToCompare::in_the_original(&plan)
            .as_select_condition()
            .unwrap();
        assert_eq!(condition, format!("between(n,0,{})", frame_count - 1));
    }

    #[test]
    fn pictures_apart_from_each_other_are_asked_for_run_by_run() {
        let apart = PicturesToCompare(BTreeSet::from([0, 1, 2, 7, 9, 10]));
        assert_eq!(
            apart.as_select_condition().as_deref(),
            Some("between(n,0,2)+between(n,7,7)+between(n,9,10)")
        );
        assert_eq!(PicturesToCompare::default().as_select_condition(), None);
        assert_eq!(
            small_copy_filters("between(n,0,2)"),
            "select='between(n,0,2)',scale=16:16:flags=area,format=gray"
        );
    }

    #[test]
    fn each_picture_gets_the_small_copy_written_in_its_place() {
        let compared = PicturesToCompare(BTreeSet::from([4, 9]));
        let mut cells = vec![10; SMALL_COPY_CELLS];
        cells.extend(vec![20; SMALL_COPY_CELLS]);

        let small_copies = small_copies_from(&compared, &cells).unwrap();
        assert_eq!(small_copies[&4], SmallCopy(vec![10; SMALL_COPY_CELLS]));
        assert_eq!(small_copies[&9], SmallCopy(vec![20; SMALL_COPY_CELLS]));
        assert!((small_copies[&4].difference_from(&small_copies[&9]) - 10.0).abs() < 1e-9);

        // A picture short, as when ffmpeg decoded fewer than were listed.
        assert!(small_copies_from(&compared, &cells[..SMALL_COPY_CELLS]).is_err());
    }

    #[test]
    fn sound_a_fraction_of_a_millisecond_off_is_in_step() {
        let mut scenes = correct_scenes(&original());
        for packet in scenes[1].sound.get_mut(&SOUND_STREAM).unwrap() {
            packet.starts_at -= 0.0007;
        }
        let proof = proof_of(&scenes).unwrap();
        assert!((proof.largest_sound_shift_seconds - 0.0007).abs() < 1e-9);
    }

    #[test]
    fn a_video_without_sound_is_proven_by_its_pictures() {
        let with_sound = original();
        let mut scenes = correct_scenes(&with_sound);
        for scene in &mut scenes {
            scene.sound.clear();
        }
        let silent = ReadBack {
            sound: BTreeMap::new(),
            ..with_sound
        };
        let proof = prove(&plan(), &[cut_time()], &silent, &scenes).unwrap();
        assert_eq!(proof.sound_packets, 0);
    }

    #[test]
    fn a_sound_stream_the_original_lacks_is_not_proven() {
        let mut scenes = correct_scenes(&original());
        scenes[0].sound.insert(2, Vec::new());
        assert_eq!(
            proof_of(&scenes),
            Err(NotProven::SoundStreams {
                scene: 1,
                stream: 2
            })
        );
    }

    #[test]
    fn a_listing_is_read_with_the_clock_of_each_stream() {
        let mut listing = ReadBackListing::default();
        for line in [
            "#software: Lavf58.76.100",
            "#tb 0: 1/12800",
            "#media_type 0: video",
            "#tb 1: 1/44100",
            "0,          0,        512,      512,    86400, 0x96846e05",
            "1,      -1024,      -1024,     1024,      206, 0xd9206012, F=0x5, S=1,       10",
            "",
        ] {
            listing.take_line(line).unwrap();
        }

        assert_eq!(
            listing.read.pictures,
            vec![DecodedPicture {
                shown_at: 0.04,
                checksum: 0x9684_6e05,
            }]
        );
        assert_eq!(
            listing.read.sound[&1],
            vec![SoundPacket {
                starts_at: -1024.0 / 44100.0,
                size_bytes: 206,
                checksum: 0xd920_6012,
            }]
        );
    }

    #[test]
    fn a_line_of_a_stream_without_a_clock_is_refused() {
        let mut listing = ReadBackListing::default();
        assert!(listing
            .take_line("0, 0, 512, 512, 86400, 0x96846e05")
            .is_err());
    }

    #[test]
    fn a_truncated_line_is_refused() {
        let mut listing = ReadBackListing::default();
        listing.take_line("#tb 0: 1/25").unwrap();
        assert!(listing.take_line("0, 0, 1").is_err());
        assert!(listing.take_line("0, 0, 1, 1, 86400, 96846e05").is_err());
        assert!(listing.take_line("#tb 0: 1/0").is_err());
    }
}
