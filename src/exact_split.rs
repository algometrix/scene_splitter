//! Writes scenes that start on the exact picture that was chosen.
//!
//! Each scene is put together from parts, as `exact_cut` plans them: the
//! pictures between a cut and the nearest keyframe are encoded again with an
//! encoder set up like the original's, everything else is copied, and the
//! parts are joined without touching them again. Sound is never encoded: a
//! scene gets every packet of sound that starts while its pictures are shown.
//!
//! The scenes are checked against the original (`exact_verify`) while they
//! are still in the unfinished folder. Scenes that do not pass are thrown
//! away, so nothing that could not be proven good is ever left looking like
//! a finished scene.
//!
//! Supported: H.264 video with 8-bit 4:2:0 colour, not interlaced, in MP4,
//! MOV or Matroska files. Anything else is refused before a file is written.
//! Whoever asked for the split then cuts the video the normal way instead
//! (`export`), for the reason `ExactCutError::calls_for_a_normal_cut` gives.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::exact_cut::{
    plan_exact_cuts, CopyStart, ExactCutPlan, FfmpegTime, FrameTable, PartKind, PlanError,
    ScenePart, ScenePlan, StoredFrame, EXACT_CUT_SETTING,
};
use crate::exact_fallback::{ExactCutLook, WhyNotExact};
use crate::exact_verify::{self, ExactCutProof, VerifyError};
use crate::fates::ScenesToWrite;
use crate::ffmpeg::{
    FfmpegError, NotUnderstood, PictureTimesOption, Program, TimeLimit, Turn, TurnOption,
    QUICK_ANSWER_LIMIT, STEADY_OUTPUT_LIMIT,
};
use crate::probe;
use crate::scene_name::SceneNames;
use crate::split::{self, UnfinishedScenes};
use crate::tools;

/// The encoder for the pictures around a cut. It is the one H.264 encoder
/// that can be set up to match what it is joined to, and both the ffmpeg the
/// app downloads and most others carry it.
const ENCODER: &str = "libx264";
/// The bitstream filter that gives the encoded pictures the decode times of
/// the copied ones they are joined to. ffmpeg has it from version 4.4 on.
const TIME_FILTER: &str = "setts";

/// The encoder an exact split looks for in the ffmpeg in use before it
/// writes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoder(&'static str);

impl Encoder {
    pub const LIBX264: Self = Self(ENCODER);
    /// Stands for the encoder on a machine whose ffmpeg was built without
    /// it, so the tests can see what such a machine does.
    #[cfg(test)]
    pub const ONE_NO_FFMPEG_HAS: Self = Self("an_encoder_no_ffmpeg_has");
}

/// The quality the pictures around a cut are encoded at. At 16 a picture
/// cannot be told from its source by eye, and the few seconds concerned make
/// the size of the result no matter.
const ENCODER_QUALITY: &str = "16";
const ENCODER_PRESET: &str = "medium";

/// The containers an exact cut is made for, by file extension.
const SUPPORTED_EXTENSIONS: [&str; 4] = ["mp4", "m4v", "mov", "mkv"];
const SUPPORTED_CODEC: &str = "h264";
/// 8-bit 4:2:0, with the limited and the full range of values.
const SUPPORTED_PIXEL_FORMATS: [&str; 2] = ["yuv420p", "yuvj420p"];
/// What ffprobe calls a video that is not interlaced, or does not say.
const PROGRESSIVE_FIELD_ORDERS: [&str; 2] = ["progressive", "unknown"];
/// The H.264 levels, as ffprobe numbers them: 30 is level 3.0.
const KNOWN_LEVELS: std::ops::RangeInclusive<u32> = 10..=62;

/// Newer versions of ffprobe also call a picture a keyframe when the video
/// stream itself says decoding can start there. Many of those are not clean
/// starts: pictures after them still lean on pictures before them, and a
/// copy that starts on one does not decode to the original's pictures. With
/// the stream parsers off, ffprobe reports only the keyframes the container
/// lists, which is also what older versions report.
const KEYFRAMES_AS_THE_CONTAINER_MARKS_THEM: [&str; 2] = ["-fflags", "+noparse"];

/// Parts are written as MP4 whatever the original is, because MP4 keeps the
/// times of pictures in the original's own steps.
const PART_FORMAT: &str = "mp4";
/// Sound waits for the join in a NUT file, the one container that keeps any
/// codec's packets and their times exactly as they were.
const SOUND_FORMAT: &str = "nut";
const SOUND_FILE: &str = "sound.nut";
const PARTS_LIST_FILE: &str = "parts.txt";
const PARTS_FOLDER: &str = "parts";

/// ffmpeg jumps to a keyframe some way before the place asked for and reads
/// on from there. Asking for a place this much too early, and dropping what
/// comes before the real one, makes sure the jump never lands after it.
const SEEK_PREROLL: FfmpegTime = FfmpegTime::from_whole_seconds(1);
/// Sound of a scene is stored this much late while it waits for the join,
/// and moved back when joined. A file's first packets of sound often start
/// a little before time zero, which a file of sound alone cannot hold.
const SOUND_PAD: FfmpegTime = FfmpegTime::from_whole_seconds(1);

/// Encoding and copying report their progress about twice a second, also
/// while ffmpeg works its way up to the first picture to encode. Two minutes
/// of silence allows for a sleeping disk or a slow network share.
const EXACT_CUT_STALL_LIMIT: TimeLimit = TimeLimit::Silence(Duration::from_secs(120));

/// What about a video an exact cut is not made for.
#[derive(Debug, Clone, PartialEq)]
pub enum Unsupported {
    Container {
        extension: String,
    },
    Codec {
        name: String,
    },
    PixelFormat {
        name: String,
    },
    Profile {
        name: String,
    },
    Interlaced {
        field_order: String,
    },
    /// The video stream's clock does not tick in a whole fraction of a
    /// second, so MP4 parts could not keep its times.
    Clock {
        time_base: String,
    },
    /// The file stores pictures that players leave out, as a video does
    /// that was trimmed without encoding. The stored pictures can then not
    /// be counted off against the pictures that are shown.
    PicturesNeverShown {
        count: usize,
    },
}

impl fmt::Display for Unsupported {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Container { extension } => write!(
                formatter,
                "exact cuts work on {} files, not on .{extension} files",
                SUPPORTED_EXTENSIONS.join(", ")
            ),
            Self::Codec { name } => {
                write!(
                    formatter,
                    "exact cuts work on H.264 video, this video is {name}"
                )
            }
            Self::PixelFormat { name } => write!(
                formatter,
                "exact cuts work on 8-bit 4:2:0 pictures, this video has {name}"
            ),
            Self::Profile { name } => {
                write!(
                    formatter,
                    "the H.264 profile {name:?} cannot be matched by the encoder"
                )
            }
            Self::Interlaced { field_order } => {
                write!(formatter, "the video is interlaced ({field_order})")
            }
            Self::Clock { time_base } => {
                write!(
                    formatter,
                    "the video's clock ({time_base}) cannot be kept exactly"
                )
            }
            Self::PicturesNeverShown { count } => write!(
                formatter,
                "the file stores {count} pictures that are marked as never shown"
            ),
        }
    }
}

/// The steps of an exact split that run ffmpeg or ffprobe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    LookForEncoder,
    LookForTimeFilter,
    ReadFormat,
    ListPictures,
    EncodePart,
    CopyPart,
    CopySound,
    JoinScene,
}

impl Step {
    fn in_words(self) -> &'static str {
        match self {
            Self::LookForEncoder => "could not list the encoders of ffmpeg",
            Self::LookForTimeFilter => "could not list the bitstream filters of ffmpeg",
            Self::ReadFormat => "could not read how the video is encoded",
            Self::ListPictures => "could not list the pictures of the video",
            Self::EncodePart => "could not encode the pictures around a cut",
            Self::CopyPart => "could not copy a part of a scene",
            Self::CopySound => "could not copy the sound of a scene",
            Self::JoinScene => "could not join the parts of a scene",
        }
    }

    /// Whether the step writes a part of a scene. The steps before those
    /// only read the video and ask ffmpeg what it can do.
    fn writes_a_scene(self) -> bool {
        match self {
            Self::EncodePart | Self::CopyPart | Self::CopySound | Self::JoinScene => true,
            Self::LookForEncoder
            | Self::LookForTimeFilter
            | Self::ReadFormat
            | Self::ListPictures => false,
        }
    }
}

/// Everything that can stop an exact split. Whichever it is, the original
/// video was not changed and no scene file was put in place.
#[derive(Debug)]
pub enum ExactCutError {
    Unsupported(Unsupported),
    /// The ffmpeg in use was built without the encoder.
    EncoderMissing,
    /// The ffmpeg in use is too old to set the decode times of the pictures
    /// it encodes.
    TimeFilterMissing,
    Plan(PlanError),
    Ffmpeg {
        step: Step,
        source: FfmpegError,
    },
    Disk {
        doing: String,
        source: io::Error,
    },
    /// The scenes were written but could not be proven good, and were thrown
    /// away.
    NotProven(VerifyError),
}

impl ExactCutError {
    /// What happened and what to do, for a person who knows nothing of
    /// video. `Display` gives the technical reason.
    pub fn in_plain_words(&self) -> String {
        let what_happened = match self {
            Self::Unsupported(_) => "Exact cuts do not work for this kind of video.",
            Self::EncoderMissing => {
                "Exact cuts need an encoder that this copy of ffmpeg does not have."
            }
            Self::TimeFilterMissing => "This copy of ffmpeg is too old for exact cuts.",
            Self::NotProven(_) => {
                "The scenes were cut on the exact pictures, but they did not pass the check and were thrown away."
            }
            Self::Plan(_) | Self::Ffmpeg { .. } | Self::Disk { .. } => {
                "The video could not be cut on the exact pictures."
            }
        };
        format!(
            "{what_happened} {ORIGINAL_UNTOUCHED} {}",
            normal_cut_offer()
        )
    }

    /// Why the video is to be cut the normal way instead, at the nearest
    /// points where it can be cut. `None` when a normal cut would meet the
    /// same trouble, so the failure itself is what the person has to hear.
    ///
    /// A failure calls for a normal cut when it says something about exact
    /// cuts and this video: the video is of a kind they are not made for,
    /// the ffmpeg in use cannot make them, or the try was made and did not
    /// hold up. By then nothing of the try is left: its scenes never left
    /// the unfinished folder, which is removed whenever a split fails.
    ///
    /// A full disk, an ffmpeg that does not start and a drive that stopped
    /// answering say nothing about exact cuts. A second try on top of those
    /// would only make the person wait for the same failure.
    pub fn calls_for_a_normal_cut(&self) -> Option<WhyNotExact> {
        match self {
            Self::Unsupported(what) => Some(WhyNotExact::KindOfVideo(what.clone())),
            Self::EncoderMissing => Some(WhyNotExact::EncoderMissing),
            Self::TimeFilterMissing => Some(WhyNotExact::FfmpegTooOld),
            Self::Plan(problem) => Some(WhyNotExact::Pictures(problem.clone())),
            Self::NotProven(VerifyError::NotProven(_)) => Some(WhyNotExact::DidNotPassTheCheck {
                details: self.to_string(),
            }),
            Self::NotProven(VerifyError::DoesNotDecode { source, .. })
                if ran_and_found_fault(source) =>
            {
                Some(WhyNotExact::DidNotPassTheCheck {
                    details: self.to_string(),
                })
            }
            Self::Ffmpeg { step, source }
                if step.writes_a_scene() && ran_and_found_fault(source) =>
            {
                Some(WhyNotExact::CouldNotBeWritten {
                    details: self.to_string(),
                })
            }
            Self::NotProven(
                VerifyError::DoesNotDecode { .. } | VerifyError::SmallCopiesUnreadable { .. },
            )
            | Self::Ffmpeg { .. }
            | Self::Disk { .. } => None,
        }
    }
}

/// Whether ffmpeg ran and found fault with what it was given, which says
/// something about the video. That it could not be started, went silent or
/// was lost says something about the machine.
fn ran_and_found_fault(error: &FfmpegError) -> bool {
    match error {
        FfmpegError::Failed { .. } | FfmpegError::OutputNotUnderstood { .. } => true,
        FfmpegError::CouldNotStart { .. }
        | FfmpegError::TimedOut { .. }
        | FfmpegError::LostContact { .. } => false,
    }
}

const ORIGINAL_UNTOUCHED: &str = "The original was not changed.";
/// Where to go from a failed exact cut. The setting is named as the window
/// labels it.
fn normal_cut_offer() -> String {
    format!(
        "To split this video the normal way, turn off \"{EXACT_CUT_SETTING}\" in Settings \
         and split again."
    )
}

impl fmt::Display for ExactCutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(what) => write!(formatter, "{what}"),
            Self::EncoderMissing => {
                write!(formatter, "this ffmpeg has no {ENCODER} encoder")
            }
            Self::TimeFilterMissing => {
                write!(
                    formatter,
                    "this ffmpeg has no {TIME_FILTER} bitstream filter"
                )
            }
            Self::Plan(problem) => write!(formatter, "{problem}"),
            Self::Ffmpeg { step, source } => write!(formatter, "{}: {source}", step.in_words()),
            Self::Disk { doing, source } => write!(formatter, "could not {doing}: {source}"),
            Self::NotProven(problem) => {
                write!(
                    formatter,
                    "the exact cut could not be proven good: {problem}"
                )
            }
        }
    }
}

impl std::error::Error for ExactCutError {}

/// The scene files of an exact split, and what their check measured.
#[derive(Debug)]
pub struct ExactSplit {
    pub scene_files: Vec<PathBuf>,
    pub proof: ExactCutProof,
}

/// Splits `input` on the first picture at or after each cut time and returns
/// the scene files in order, checked against the original.
///
/// Every scene is written and checked, the dropped ones too, since the
/// check counts the pictures and the sound of the whole video. Only the
/// kept scenes then get a name, numbered without gaps as `names` say.
///
/// Like a split on keyframes, this replaces the scene files of an earlier
/// split and writes into a folder of its own first. Unlike it, the scenes
/// are checked before they get their names. A failure of any kind is an
/// [`ExactCutError`], leaves the scene files of an earlier split as they
/// were, and never touches the original.
pub fn split_on_exact_frames(
    input: &Path,
    scenes: &ScenesToWrite,
    out_dir: &Path,
    names: impl Into<SceneNames>,
    encoder: Encoder,
) -> Result<ExactSplit> {
    let cuts = scenes.cuts();
    // Everything that can be refused is refused before a file is written.
    let source = refuse_what_is_known_at_a_glance(input, encoder)?;
    let table = read_frame_table(input)?;
    let plan = plan_exact_cuts(&table, cuts).map_err(ExactCutError::Plan)?;

    let mut proof = None;
    // The check has seen every scene by the time one is dropped, so nothing
    // more needs to be noted about a dropped scene.
    let nothing_to_note = |_dropped: &Path| Ok(());
    let outcomes = split::write_then_move_into_place(
        input,
        scenes,
        out_dir,
        names,
        |unfinished| {
            write_scenes(input, &source, &plan, unfinished)?;
            let written = exact_verify::WrittenScenes {
                files: &unfinished.files,
                scratch_folder: &unfinished.folder,
            };
            let proven = exact_verify::verify_exact_split(input, written, &plan, cuts.as_slice())
                .map_err(ExactCutError::NotProven)?;
            proof = Some(proven);
            Ok(())
        },
        nothing_to_note,
    )?;
    let proof = proof.expect("the scenes are only put in place once they are proven");
    Ok(ExactSplit {
        scene_files: split::written_files(&outcomes),
        proof,
    })
}

/// The plan an exact split of `input` at `cuts` would follow, for tests that
/// check scene files against it themselves.
#[cfg(test)]
pub fn plan_of(input: &Path, cuts: &crate::scenes::CutTimes) -> Result<ExactCutPlan> {
    let table = read_frame_table(input)?;
    Ok(plan_exact_cuts(&table, cuts).map_err(ExactCutError::Plan)?)
}

/// Looks at `input` the way an exact split does before it writes anything,
/// for telling the person before the split what the split will do: whether
/// the video can be cut on exact pictures at all, and if so its pictures,
/// from which the cut can be planned for any splits.
///
/// What shows at a glance is asked first, so a video that is refused there
/// is known in a moment. Listing the pictures reads the index of an MP4 or
/// MOV file, which takes a fraction of a second for minutes of video, and
/// reads through a Matroska file from end to end. Never call this where a
/// window is drawn.
///
/// A video or an ffmpeg that could not be asked is reported as one that
/// can be tried. The split then finds the same trouble and says what it is.
pub fn look_at_exact_cut(input: &Path) -> ExactCutLook {
    let pictures = refuse_what_is_known_at_a_glance(input, Encoder::LIBX264)
        .and_then(|_can_be_tried| read_frame_table(input));
    match pictures {
        Ok(pictures) => ExactCutLook::PicturesListed(Arc::new(pictures)),
        Err(refusal) => match refusal.calls_for_a_normal_cut() {
            Some(why_not_exact) => ExactCutLook::CannotBeCut(why_not_exact),
            None => ExactCutLook::CanBeTried,
        },
    }
}

/// Refuses a video whose file name, ffmpeg or way of storing pictures rules
/// out an exact cut. These take a moment to find out, where listing every
/// picture takes as long as reading through the file.
fn refuse_what_is_known_at_a_glance(
    input: &Path,
    encoder: Encoder,
) -> Result<SourceVideo, ExactCutError> {
    refuse_other_containers(input)?;
    look_for_encoder(encoder)?;
    look_for_time_filter()?;
    read_source_video(input)
}

fn refuse_other_containers(input: &Path) -> Result<(), ExactCutError> {
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if SUPPORTED_EXTENSIONS.contains(&extension.as_str()) {
        return Ok(());
    }
    Err(ExactCutError::Unsupported(Unsupported::Container {
        extension,
    }))
}

/// Fails with [`ExactCutError::EncoderMissing`] when the ffmpeg in use
/// cannot encode H.264 the way an exact cut needs.
fn look_for_encoder(encoder: Encoder) -> Result<(), ExactCutError> {
    let listing = tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-encoders"])
        .run_for_text(QUICK_ANSWER_LIMIT)
        .map_err(|source| ExactCutError::Ffmpeg {
            step: Step::LookForEncoder,
            source,
        })?;
    if lists_encoder(&listing, encoder.0) {
        Ok(())
    } else {
        Err(ExactCutError::EncoderMissing)
    }
}

/// Fails with [`ExactCutError::TimeFilterMissing`] when the ffmpeg in use is
/// from before version 4.4, which brought the filter that sets the decode
/// times of encoded pictures. Without this the first encoding would fail on
/// an option it does not know, after the video was read through.
fn look_for_time_filter() -> Result<(), ExactCutError> {
    let listing = tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-bsfs"])
        .run_for_text(QUICK_ANSWER_LIMIT)
        .map_err(|source| ExactCutError::Ffmpeg {
            step: Step::LookForTimeFilter,
            source,
        })?;
    if lists_bitstream_filter(&listing, TIME_FILTER) {
        Ok(())
    } else {
        Err(ExactCutError::TimeFilterMissing)
    }
}

/// Whether ffmpeg's list of bitstream filters names `filter`. The list has
/// a heading and then one name to a line.
fn lists_bitstream_filter(listing: &str, filter: &str) -> bool {
    listing.lines().any(|line| line.trim() == filter)
}

/// Whether ffmpeg's list of encoders names `encoder`. A line of it reads
/// " V....D libx264   libx264 H.264 / AVC ...": flags, name, description.
fn lists_encoder(listing: &str, encoder: &str) -> bool {
    listing
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(encoder))
}

/// How to set up the encoder so that what it writes can be joined to the
/// original's own pictures and plays as one stream.
#[derive(Debug, Clone, PartialEq)]
struct EncoderSettings {
    profile: &'static str,
    /// As ffmpeg takes it, "4.0". `None` when the original does not say.
    level: Option<String>,
    pixel_format: String,
    /// The colour properties the original states, as ffmpeg options.
    colour: Vec<(&'static str, String)>,
}

/// What an exact split needs to know about the original besides its
/// pictures.
#[derive(Debug, Clone, PartialEq)]
struct SourceVideo {
    encoder: EncoderSettings,
    /// The steps per second of the video stream's clock.
    ticks_per_second: u32,
    sound_streams: usize,
    /// How far the pictures are to be turned for showing, when the file
    /// says so. A phone held upright stores what it films lying on its side
    /// and marks it to be turned.
    turn: Option<Turn>,
}

fn read_source_video(input: &Path) -> Result<SourceVideo, ExactCutError> {
    let read_failed = |source| ExactCutError::Ffmpeg {
        step: Step::ReadFormat,
        source,
    };
    let properties = tools::command(Program::Ffprobe)
        .args(["-v", "error", "-select_streams", "v:0"])
        .args([
            "-show_entries",
            "stream=codec_name,profile,level,pix_fmt,field_order,time_base,\
             color_range,color_space,color_transfer,color_primaries",
        ])
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(input)
        .run_for_text(QUICK_ANSWER_LIMIT)
        .map_err(read_failed)?;
    let sound_streams = tools::command(Program::Ffprobe)
        .args(["-v", "error", "-select_streams", "a"])
        .args(["-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(input)
        .run_for_text(QUICK_ANSWER_LIMIT)
        .map_err(read_failed)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let turn = exact_verify::read_shown_as(input)
        .map_err(read_failed)?
        .turn();
    source_video_from(&properties, sound_streams, turn).map_err(ExactCutError::Unsupported)
}

/// Reads ffprobe's "name=value" lines about the video stream and decides
/// whether an exact cut can match it.
fn source_video_from(
    properties: &str,
    sound_streams: usize,
    turn: Option<Turn>,
) -> Result<SourceVideo, Unsupported> {
    let properties: BTreeMap<&str, &str> = properties
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.trim(), value.trim()))
        .collect();
    let property = |name: &str| properties.get(name).copied().unwrap_or_default();

    let codec = property("codec_name");
    if codec != SUPPORTED_CODEC {
        return Err(Unsupported::Codec {
            name: codec.to_owned(),
        });
    }
    let pixel_format = property("pix_fmt");
    if !SUPPORTED_PIXEL_FORMATS.contains(&pixel_format) {
        return Err(Unsupported::PixelFormat {
            name: pixel_format.to_owned(),
        });
    }
    let field_order = property("field_order");
    if !field_order.is_empty() && !PROGRESSIVE_FIELD_ORDERS.contains(&field_order) {
        return Err(Unsupported::Interlaced {
            field_order: field_order.to_owned(),
        });
    }
    let profile = match property("profile").to_ascii_lowercase().as_str() {
        "baseline" | "constrained baseline" => "baseline",
        "main" => "main",
        "high" | "constrained high" | "progressive high" => "high",
        _ => {
            return Err(Unsupported::Profile {
                name: property("profile").to_owned(),
            })
        }
    };
    let time_base = property("time_base");
    let ticks_per_second = time_base
        .strip_prefix("1/")
        .and_then(|ticks| ticks.parse::<u32>().ok())
        .filter(|&ticks| ticks > 0)
        .ok_or_else(|| Unsupported::Clock {
            time_base: time_base.to_owned(),
        })?;

    let level = property("level")
        .parse::<u32>()
        .ok()
        .filter(|level| KNOWN_LEVELS.contains(level))
        .map(|level| format!("{}.{}", level / 10, level % 10));
    let colour = [
        ("-color_range", "color_range"),
        ("-colorspace", "color_space"),
        ("-color_trc", "color_transfer"),
        ("-color_primaries", "color_primaries"),
    ]
    .into_iter()
    .map(|(option, name)| (option, property(name)))
    .filter(|(_, value)| is_stated(value))
    .map(|(option, value)| (option, value.to_owned()))
    .collect();

    Ok(SourceVideo {
        encoder: EncoderSettings {
            profile,
            level,
            pixel_format: pixel_format.to_owned(),
            colour,
        },
        ticks_per_second,
        sound_streams,
        turn,
    })
}

/// Whether ffprobe gave a real value for a colour property.
fn is_stated(value: &str) -> bool {
    !matches!(value, "" | "unknown" | "reserved" | "N/A")
}

/// Lists every picture of the video with the time it is shown and decoded.
/// Like the packet table of a scan this only reads the file's index.
fn read_frame_table(input: &Path) -> Result<FrameTable, ExactCutError> {
    let list_failed = |source| ExactCutError::Ffmpeg {
        step: Step::ListPictures,
        source,
    };
    let container_start = probe::read_container_start(input).map_err(|error| match error
        .downcast::<FfmpegError>()
    {
        Ok(source) => list_failed(source),
        Err(other) => list_failed(FfmpegError::OutputNotUnderstood {
            program: Program::Ffprobe,
            detail: format!("{other:#}"),
        }),
    })?;

    let mut stored = Vec::new();
    let mut never_shown = 0;
    tools::command(Program::Ffprobe)
        .args(["-v", "error"])
        .args(KEYFRAMES_AS_THE_CONTAINER_MARKS_THEM)
        .args(["-select_streams", "v:0"])
        .args(["-show_entries", "packet=pts,pts_time,dts,dts_time,flags"])
        .args(["-of", "csv=p=0"])
        .arg(input)
        .stream_lines(STEADY_OUTPUT_LIMIT, |line| {
            if line.trim().is_empty() {
                return Ok(());
            }
            let listed = parse_stored_frame(line, container_start)
                .map_err(|problem| NotUnderstood(format!("packet line {line:?}, {problem}")))?;
            match listed.showing {
                Showing::Shown => stored.push(listed.frame),
                Showing::NeverShown => never_shown += 1,
            }
            Ok(())
        })
        .map_err(list_failed)?;
    // The plan counts pictures off against what a player shows. A stored
    // picture that players leave out would put every count after it one off.
    if never_shown > 0 {
        return Err(ExactCutError::Unsupported(
            Unsupported::PicturesNeverShown { count: never_shown },
        ));
    }
    FrameTable::from_stored_order(stored).map_err(ExactCutError::Plan)
}

/// Whether a stored picture is shown when the video plays.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Showing {
    Shown,
    /// The file marks the picture as one to decode and then leave out. A
    /// video trimmed without encoding keeps the pictures before its new
    /// start that way, since the ones after them are built on them.
    NeverShown,
}

/// One line of ffprobe's list of stored pictures.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ListedFrame {
    frame: StoredFrame,
    showing: Showing,
}

/// The letter ffprobe puts among a packet's flags when players leave its
/// picture out.
const NEVER_SHOWN_FLAG: char = 'D';

/// ffprobe prints these fields in the order pts, pts_time, dts, dts_time,
/// flags, whatever order they were asked for in. Times are moved onto the
/// clock `ffmpeg -ss` uses, which counts from the container's start.
fn parse_stored_frame(line: &str, container_start: f64) -> Result<ListedFrame, &'static str> {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let [shown_ticks, shown_at, decoded_ticks, decoded_at, flags, ..] = fields[..] else {
        return Err("expected at least five fields");
    };
    let shown_ticks: i64 = shown_ticks.parse().map_err(|_| "pts is not a number")?;
    let shown_at: f64 = shown_at.parse().map_err(|_| "pts_time is not a number")?;
    // Matroska files do not store decode times, and ffprobe cannot work
    // them out for the first pictures.
    let decoded_ticks: Option<i64> = decoded_ticks.parse().ok();
    let decoded_at: Option<f64> = decoded_at.parse().ok();

    let showing = if flags.contains(NEVER_SHOWN_FLAG) {
        Showing::NeverShown
    } else {
        Showing::Shown
    };
    Ok(ListedFrame {
        frame: StoredFrame {
            shown_at: shown_at - container_start,
            decoded_at: decoded_at.map(|decoded_at| decoded_at - container_start),
            reorder_ticks: decoded_ticks.map(|decoded_ticks| shown_ticks - decoded_ticks),
            is_keyframe: flags.starts_with('K'),
        },
        showing,
    })
}

/// Writes every scene into the unfinished folder, one after another, since
/// each encoding already uses every processor.
fn write_scenes(
    input: &Path,
    source: &SourceVideo,
    plan: &ExactCutPlan,
    unfinished: &UnfinishedScenes,
) -> Result<(), ExactCutError> {
    let parts_dir = unfinished.folder.join(PARTS_FOLDER);
    for (scene, scene_file) in plan.scenes().iter().zip(&unfinished.files) {
        std::fs::create_dir_all(&parts_dir).map_err(|source| ExactCutError::Disk {
            doing: format!("create {}", parts_dir.display()),
            source,
        })?;
        write_scene(input, source, scene, &parts_dir, scene_file)?;
        // Removed after each scene, so the parts of only one scene take up
        // room at a time.
        std::fs::remove_dir_all(&parts_dir).map_err(|source| ExactCutError::Disk {
            doing: format!("remove {}", parts_dir.display()),
            source,
        })?;
        if !scene_file.is_file() {
            return Err(ExactCutError::Disk {
                doing: format!("find {} after joining it", scene_file.display()),
                source: io::Error::from(io::ErrorKind::NotFound),
            });
        }
    }
    Ok(())
}

fn write_scene(
    input: &Path,
    source: &SourceVideo,
    scene: &ScenePlan,
    parts_dir: &Path,
    scene_file: &Path,
) -> Result<(), ExactCutError> {
    let mut parts_list = String::from(PARTS_LIST_HEADER);
    for (position, part) in scene.parts.iter().enumerate() {
        let part_name = format!("part_{}.{PART_FORMAT}", position + 1);
        let part_file = parts_dir.join(&part_name);
        let (step, arguments) = match part.kind {
            PartKind::Copied { start } => (
                Step::CopyPart,
                copy_arguments(input, part, start, source.ticks_per_second, &part_file),
            ),
            PartKind::EncodedAgain {
                seek,
                reorder_ticks,
            } => (
                Step::EncodePart,
                encode_arguments(
                    input,
                    part,
                    seek,
                    reorder_ticks,
                    tools::picture_times_option(),
                    source,
                    &part_file,
                ),
            ),
        };
        run_ffmpeg(step, arguments)?;
        parts_list.push_str(&parts_list_entry(&part_name, part.seconds));
    }
    let parts_list_file = parts_dir.join(PARTS_LIST_FILE);
    std::fs::write(&parts_list_file, parts_list).map_err(|source| ExactCutError::Disk {
        doing: format!("write {}", parts_list_file.display()),
        source,
    })?;

    let sound_file = (source.sound_streams > 0).then(|| parts_dir.join(SOUND_FILE));
    if let Some(sound_file) = &sound_file {
        run_ffmpeg(Step::CopySound, sound_arguments(input, scene, sound_file))?;
    }
    run_ffmpeg(
        Step::JoinScene,
        join_arguments(
            input,
            scene,
            source,
            tools::turn_option(),
            &parts_list_file,
            sound_file.as_deref(),
            scene_file,
        ),
    )
}

/// Runs ffmpeg to its end. It is asked to report progress on its standard
/// output, which is what the stall limit watches.
fn run_ffmpeg(step: Step, arguments: Vec<OsString>) -> Result<(), ExactCutError> {
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-progress", "pipe:1"])
        .args(arguments)
        .stream_lines(EXACT_CUT_STALL_LIMIT, |_progress_report| Ok(()))
        .map_err(|source| ExactCutError::Ffmpeg { step, source })
}

/// Collects the arguments of one ffmpeg run.
#[derive(Default)]
struct Arguments(Vec<OsString>);

impl Arguments {
    fn with(mut self, arguments: &[&str]) -> Self {
        self.0.extend(arguments.iter().map(OsString::from));
        self
    }

    fn option(mut self, name: &str, value: impl fmt::Display) -> Self {
        self.0.push(OsString::from(name));
        self.0.push(OsString::from(value.to_string()));
        self
    }

    fn path(mut self, name: Option<&str>, path: &Path) -> Self {
        self.0.extend(name.map(OsString::from));
        self.0.push(path.as_os_str().to_os_string());
        self
    }

    fn done(self) -> Vec<OsString> {
        self.0
    }
}

/// Parts carry no titles or chapter marks of their own. The joined scene
/// takes those of the original.
const PART_WITHOUT_TAGS: [&str; 4] = ["-map_metadata", "-1", "-map_chapters", "-1"];

/// Decodes from the keyframe before the part, drops the pictures before its
/// first, and encodes exactly as many pictures as the part has.
fn encode_arguments(
    input: &Path,
    part: &ScenePart,
    seek: Option<FfmpegTime>,
    reorder_ticks: i64,
    picture_times: PictureTimesOption,
    source: &SourceVideo,
    part_file: &Path,
) -> Vec<OsString> {
    let encoder = &source.encoder;
    let mut arguments = Arguments::default();
    if let Some(seek) = seek {
        arguments = arguments.option("-ss", seek);
    }
    arguments = arguments
        // A video marked as turned on its side keeps its pictures as they
        // are stored. The mark itself is checked with the result.
        .with(&["-noautorotate"])
        .path(Some("-i"), input)
        .with(&["-map", "0:v:0"])
        .option("-frames:v", part.frames.len())
        .with(&["-c:v", ENCODER, "-preset", ENCODER_PRESET])
        .option("-crf", ENCODER_QUALITY)
        // Pictures that are stored in the order they are shown, so the last
        // one encoded is the last one before the join.
        .with(&["-bf", "0"])
        .option("-profile:v", encoder.profile)
        .option("-pix_fmt", &encoder.pixel_format);
    if let Some(level) = &encoder.level {
        arguments = arguments.option("-level", level);
    }
    for (option, value) in &encoder.colour {
        arguments = arguments.option(option, value);
    }
    arguments = arguments
        // Every picture keeps the exact time the original gives it.
        .with(&picture_times.arguments());
    if reorder_ticks != 0 {
        arguments = arguments.option("-bsf:v", format!("setts=dts=PTS-{reorder_ticks}"));
    }
    arguments
        .option("-video_track_timescale", source.ticks_per_second)
        .with(&PART_WITHOUT_TAGS)
        .option("-f", PART_FORMAT)
        .path(None, part_file)
        .done()
}

/// Copies as many pictures as the part has, from its keyframe on.
fn copy_arguments(
    input: &Path,
    part: &ScenePart,
    start: CopyStart,
    ticks_per_second: u32,
    part_file: &Path,
) -> Vec<OsString> {
    let (jump_to, drop_before) = match start {
        CopyStart::VideoStart => (None, None),
        CopyStart::FirstKeyframeDecodedAfter(time) => jump_and_drop(time),
    };
    let mut arguments = Arguments::default();
    if let Some(jump_to) = jump_to {
        arguments = arguments.option("-ss", jump_to);
    }
    arguments = arguments.path(Some("-i"), input);
    if let Some(drop_before) = drop_before {
        arguments = arguments.option("-ss", drop_before);
    }
    arguments
        .with(&["-map", "0:v:0"])
        .option("-frames:v", part.frames.len())
        .with(&["-c:v", "copy"])
        .option("-video_track_timescale", ticks_per_second)
        .with(&PART_WITHOUT_TAGS)
        .option("-f", PART_FORMAT)
        .path(None, part_file)
        .done()
}

/// Copies every packet of sound that starts while the scene is shown.
fn sound_arguments(input: &Path, scene: &ScenePlan, sound_file: &Path) -> Vec<OsString> {
    let (jump_to, drop_before) = match scene.sound_from {
        None => (None, None),
        Some(from) => jump_and_drop(from),
    };
    let mut arguments = Arguments::default();
    if let Some(jump_to) = jump_to {
        arguments = arguments.option("-ss", jump_to);
    }
    arguments = arguments.path(Some("-i"), input);
    if let Some(drop_before) = drop_before {
        arguments = arguments.option("-ss", drop_before);
    }
    if let Some(until) = scene.sound_until {
        let from = scene.sound_from.unwrap_or(FfmpegTime::ZERO);
        arguments = arguments.option("-t", until.since(from));
    }
    arguments
        .with(&["-map", "0:a", "-c", "copy"])
        .option("-output_ts_offset", SOUND_PAD)
        .option("-f", SOUND_FORMAT)
        .path(None, sound_file)
        .done()
}

/// How to have ffmpeg start a copy at `time`: the place to jump to before
/// reading, and the time, counted from there, before which packets are
/// dropped. The two add up to `time` exactly.
fn jump_and_drop(time: FfmpegTime) -> (Option<FfmpegTime>, Option<FfmpegTime>) {
    let preroll = SEEK_PREROLL.earlier_of(time);
    let jump_to = time.since(preroll);
    (jump_to.is_positive().then_some(jump_to), Some(preroll))
}

const PARTS_LIST_HEADER: &str = "ffconcat version 1.0\n";

/// One part in the list ffmpeg joins. The length is given, so the next part
/// starts exactly where this one's pictures end.
fn parts_list_entry(part_name: &str, seconds: f64) -> String {
    format!("file '{part_name}'\nduration {seconds:.6}\n")
}

/// Joins the parts and the sound into the scene file, without changing a
/// packet of either.
fn join_arguments(
    input: &Path,
    scene: &ScenePlan,
    source: &SourceVideo,
    turn_option: TurnOption,
    parts_list_file: &Path,
    sound_file: Option<&Path>,
    scene_file: &Path,
) -> Vec<OsString> {
    // Times are taken as given, so the sound keeps its place against the
    // pictures. The first scene starts where the video does. Any later scene
    // starts at zero with its first picture.
    let mut arguments = Arguments::default().with(&["-copyts"]);
    let is_first_scene = scene.sound_from.is_none();
    if is_first_scene {
        arguments = arguments.option("-itsoffset", FfmpegTime::from_seconds(scene.starts_at));
    }
    // Where the ffmpeg in use would lose the turn of the pictures on the
    // way through the parts, the parts are read as turned like the original.
    for argument in turn_option.arguments(source.turn.as_ref()) {
        arguments = arguments.with(&[&argument]);
    }
    arguments = arguments
        .with(&["-f", "concat", "-safe", "0"])
        .path(Some("-i"), parts_list_file);
    if let Some(sound_file) = sound_file {
        arguments = arguments
            .option("-itsoffset", FfmpegTime::ZERO.since(SOUND_PAD))
            .option("-f", SOUND_FORMAT)
            .path(Some("-i"), sound_file);
    }
    // The original is read for its titles and languages only.
    let (original, sound_map): (usize, &[&str]) = match sound_file {
        Some(_) => (2, &["-map", "1:a"]),
        None => (1, &[]),
    };
    arguments = arguments
        .path(Some("-i"), input)
        .with(&["-map", "0:v:0"])
        .with(sound_map)
        .with(&["-c", "copy"])
        .option("-map_metadata", original)
        .option("-map_metadata:s:v:0", format!("{original}:s:v:0"))
        .with(&["-map_chapters", "-1"]);
    for stream in 0..source.sound_streams {
        arguments = arguments.option(
            &format!("-map_metadata:s:a:{stream}"),
            format!("{original}:s:a:{stream}"),
        );
    }
    arguments.path(None, scene_file).done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenes::CutTimes;
    use crate::timecode::TimeSpan;

    const H264_PROPERTIES: &str = "codec_name=h264\n\
        profile=High\n\
        pix_fmt=yuv420p\n\
        level=40\n\
        color_range=tv\n\
        color_space=bt709\n\
        color_transfer=unknown\n\
        color_primaries=bt709\n\
        field_order=progressive\n\
        time_base=1/12800\n";

    fn properties_with(name: &str, value: &str) -> String {
        H264_PROPERTIES
            .lines()
            .map(|line| match line.split_once('=') {
                Some((this, _)) if this == name => format!("{name}={value}\n"),
                _ => format!("{line}\n"),
            })
            .collect()
    }

    fn unsupported(name: &str, value: &str) -> Unsupported {
        source_video_from(&properties_with(name, value), 1, None).unwrap_err()
    }

    fn joined(arguments: &[OsString]) -> String {
        arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The plan for a video of 225 pictures, 25 a second, with a keyframe
    /// every 50, cut on picture 73.
    fn plan() -> ExactCutPlan {
        let stored = (0..225)
            .map(|frame| StoredFrame {
                shown_at: frame as f64 * 0.04,
                decoded_at: Some(frame as f64 * 0.04 - 0.08),
                reorder_ticks: Some(1024),
                is_keyframe: frame % 50 == 0,
            })
            .collect();
        let table = FrameTable::from_stored_order(stored).unwrap();
        let whole_video = TimeSpan {
            start: 0.0,
            end: 9.0,
        };
        let cuts = CutTimes::inside(whole_video, vec![73.0 * 0.04]).unwrap();
        plan_exact_cuts(&table, &cuts).unwrap()
    }

    fn source() -> SourceVideo {
        source_video_from(H264_PROPERTIES, 2, None).unwrap()
    }

    #[test]
    fn the_encoder_is_set_up_like_the_original() {
        let source = source();
        assert_eq!(
            source.encoder,
            EncoderSettings {
                profile: "high",
                level: Some("4.0".to_owned()),
                pixel_format: "yuv420p".to_owned(),
                colour: vec![
                    ("-color_range", "tv".to_owned()),
                    ("-colorspace", "bt709".to_owned()),
                    ("-color_primaries", "bt709".to_owned()),
                ],
            }
        );
        assert_eq!(source.ticks_per_second, 12800);
        assert_eq!(source.sound_streams, 2);
        assert_eq!(source.turn, None);
    }

    #[test]
    fn a_level_the_original_does_not_state_is_left_to_the_encoder() {
        let source = source_video_from(&properties_with("level", "-99"), 0, None).unwrap();
        assert_eq!(source.encoder.level, None);
    }

    #[test]
    fn each_profile_is_named_as_the_encoder_names_it() {
        for (stated, expected) in [
            ("Constrained Baseline", "baseline"),
            ("Main", "main"),
            ("High", "high"),
        ] {
            let source = source_video_from(&properties_with("profile", stated), 0, None).unwrap();
            assert_eq!(source.encoder.profile, expected);
        }
    }

    #[test]
    fn other_kinds_of_video_are_refused_by_name() {
        assert_eq!(
            unsupported("codec_name", "hevc"),
            Unsupported::Codec {
                name: "hevc".to_owned()
            }
        );
        assert_eq!(
            unsupported("pix_fmt", "yuv420p10le"),
            Unsupported::PixelFormat {
                name: "yuv420p10le".to_owned()
            }
        );
        assert_eq!(
            unsupported("field_order", "tt"),
            Unsupported::Interlaced {
                field_order: "tt".to_owned()
            }
        );
        assert_eq!(
            unsupported("profile", "High 4:4:4 Predictive"),
            Unsupported::Profile {
                name: "High 4:4:4 Predictive".to_owned()
            }
        );
        assert_eq!(
            unsupported("time_base", "1001/30000"),
            Unsupported::Clock {
                time_base: "1001/30000".to_owned()
            }
        );
    }

    #[test]
    fn only_some_containers_are_cut_exactly() {
        assert!(refuse_other_containers(Path::new("film.MP4")).is_ok());
        assert!(refuse_other_containers(Path::new("film.mkv")).is_ok());
        let refused = refuse_other_containers(Path::new("film.avi")).unwrap_err();
        assert!(
            matches!(
                &refused,
                ExactCutError::Unsupported(Unsupported::Container { extension }) if extension == "avi"
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn the_encoder_is_found_by_its_name_and_not_by_a_description() {
        let listing = "Encoders:\n ------\n \
            V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)\n \
            V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)\n";
        assert!(lists_encoder(listing, "libx264"));
        assert!(!lists_encoder(listing, "libx265"));
        assert!(!lists_encoder(
            " V....D h264_mf   H264 via MediaFoundation, not libx264\n",
            "libx264"
        ));
    }

    #[test]
    fn a_stored_picture_is_read_with_both_its_times() {
        let listed = parse_stored_frame("51200,4.000000,50176,3.920000,K_", 0.0).unwrap();
        assert_eq!(
            listed,
            ListedFrame {
                frame: StoredFrame {
                    shown_at: 4.0,
                    decoded_at: Some(3.92),
                    reorder_ticks: Some(1024),
                    is_keyframe: true,
                },
                showing: Showing::Shown,
            }
        );
    }

    #[test]
    fn a_picture_that_players_leave_out_is_read_as_never_shown() {
        // As ffprobe lists the pictures before the new start of a video
        // that was trimmed without encoding, in old and new versions.
        for line in [
            "-1024,-0.080000,-2048,-0.160000,_D",
            "-1024,-0.080000,-2048,-0.160000,KD_",
        ] {
            let listed = parse_stored_frame(line, 0.0).unwrap();
            assert_eq!(listed.showing, Showing::NeverShown, "{line}");
        }
        let shown = parse_stored_frame("0,0.000000,-1024,-0.080000,K__", 0.0).unwrap();
        assert_eq!(shown.showing, Showing::Shown);
    }

    #[test]
    fn the_time_filter_is_found_by_its_whole_name() {
        let listing = "Bitstream filters:\naac_adtstoasc\nsetts\ntrace_headers\n";
        assert!(lists_bitstream_filter(listing, "setts"));
        assert!(!lists_bitstream_filter(
            "Bitstream filters:\nsetts_of_another_kind\n",
            "setts"
        ));
    }

    #[test]
    fn a_picture_without_a_decode_time_is_still_read() {
        let frame = parse_stored_frame("120,1.620000,N/A,N/A,___", 1.5)
            .unwrap()
            .frame;
        assert!((frame.shown_at - 0.12).abs() < 1e-9);
        assert_eq!(frame.decoded_at, None);
        assert_eq!(frame.reorder_ticks, None);
        assert!(!frame.is_keyframe);
    }

    #[test]
    fn a_picture_without_a_time_is_refused() {
        assert!(parse_stored_frame("N/A,N/A,0,0.000000,K_", 0.0).is_err());
        assert!(parse_stored_frame("0,0.000000", 0.0).is_err());
    }

    #[test]
    fn a_part_is_encoded_from_just_before_its_first_picture() {
        let plan = plan();
        // Pictures 73 to 99 of the second scene.
        let part = &plan.scenes()[1].parts[0];
        let PartKind::EncodedAgain {
            seek,
            reorder_ticks,
        } = part.kind
        else {
            panic!("{part:?}");
        };
        let arguments = joined(&encode_arguments(
            Path::new("film.mp4"),
            part,
            seek,
            reorder_ticks,
            PictureTimesOption::Vsync,
            &source(),
            Path::new("part_1.mp4"),
        ));

        assert_eq!(
            arguments,
            "-ss 2.900000 -noautorotate -i film.mp4 -map 0:v:0 -frames:v 27 \
             -c:v libx264 -preset medium -crf 16 -bf 0 -profile:v high -pix_fmt yuv420p \
             -level 4.0 -color_range tv -colorspace bt709 -color_primaries bt709 \
             -vsync passthrough -enc_time_base -1 -bsf:v setts=dts=PTS-1024 \
             -video_track_timescale 12800 -map_metadata -1 -map_chapters -1 -f mp4 part_1.mp4"
        );
    }

    #[test]
    fn a_part_is_copied_from_its_keyframe_for_as_many_pictures_as_it_has() {
        let plan = plan();
        let from_the_start = &plan.scenes()[0].parts[0];
        let later = &plan.scenes()[1].parts[1];
        let arguments_of = |part: &ScenePart| {
            let PartKind::Copied { start } = part.kind else {
                panic!("{part:?}");
            };
            joined(&copy_arguments(
                Path::new("film.mp4"),
                part,
                start,
                12800,
                Path::new("part.mp4"),
            ))
        };

        assert_eq!(
            arguments_of(from_the_start),
            "-i film.mp4 -map 0:v:0 -frames:v 50 -c:v copy -video_track_timescale 12800 \
             -map_metadata -1 -map_chapters -1 -f mp4 part.mp4"
        );
        // The keyframe is decoded at 3.92 s: jump to 2.90, drop up to 3.90.
        assert_eq!(
            arguments_of(later),
            "-ss 2.900000 -i film.mp4 -ss 1.000000 -map 0:v:0 -frames:v 125 -c:v copy \
             -video_track_timescale 12800 -map_metadata -1 -map_chapters -1 -f mp4 part.mp4"
        );
    }

    #[test]
    fn the_sound_of_two_scenes_meets_at_the_same_instant() {
        let plan = plan();
        let first = joined(&sound_arguments(
            Path::new("film.mp4"),
            &plan.scenes()[0],
            Path::new("sound.nut"),
        ));
        let second = joined(&sound_arguments(
            Path::new("film.mp4"),
            &plan.scenes()[1],
            Path::new("sound.nut"),
        ));

        // The first ends 2.92 s in. The second starts at 1.92 + 1.00.
        assert_eq!(
            first,
            "-i film.mp4 -t 2.920000 -map 0:a -c copy -output_ts_offset 1.000000 -f nut sound.nut"
        );
        assert_eq!(
            second,
            "-ss 1.920000 -i film.mp4 -ss 1.000000 -map 0:a -c copy \
             -output_ts_offset 1.000000 -f nut sound.nut"
        );
    }

    #[test]
    fn a_start_close_to_the_beginning_is_reached_without_a_jump() {
        let half_a_second = FfmpegTime::from_seconds(0.5);
        assert_eq!(jump_and_drop(half_a_second), (None, Some(half_a_second)));
        assert_eq!(
            jump_and_drop(FfmpegTime::from_seconds(7.25)),
            (Some(FfmpegTime::from_seconds(6.25)), Some(SEEK_PREROLL))
        );
    }

    #[test]
    fn the_parts_and_the_sound_are_joined_without_encoding() {
        let plan = plan();
        let second_scene = joined(&join_arguments(
            Path::new("film.mp4"),
            &plan.scenes()[1],
            &source(),
            TurnOption::LeftToTheParts,
            Path::new("parts.txt"),
            Some(Path::new("sound.nut")),
            Path::new("002.mp4"),
        ));
        assert_eq!(
            second_scene,
            "-copyts -f concat -safe 0 -i parts.txt -itsoffset -1.000000 -f nut -i sound.nut \
             -i film.mp4 -map 0:v:0 -map 1:a -c copy -map_metadata 2 \
             -map_metadata:s:v:0 2:s:v:0 -map_chapters -1 \
             -map_metadata:s:a:0 2:s:a:0 -map_metadata:s:a:1 2:s:a:1 002.mp4"
        );
    }

    #[test]
    fn a_video_without_sound_is_joined_from_its_parts_alone() {
        let plan = plan();
        let without_sound = SourceVideo {
            sound_streams: 0,
            ..source()
        };
        let first_scene = joined(&join_arguments(
            Path::new("film.mp4"),
            &plan.scenes()[0],
            &without_sound,
            TurnOption::LeftToTheParts,
            Path::new("parts.txt"),
            None,
            Path::new("001.mp4"),
        ));
        assert_eq!(
            first_scene,
            "-copyts -itsoffset 0.000000 -f concat -safe 0 -i parts.txt -i film.mp4 \
             -map 0:v:0 -c copy -map_metadata 1 -map_metadata:s:v:0 1:s:v:0 \
             -map_chapters -1 001.mp4"
        );
    }

    #[test]
    fn where_ffmpeg_would_lose_the_turn_it_is_named_for_the_parts_at_the_join() {
        let plan = plan();
        let turned = SourceVideo {
            sound_streams: 0,
            turn: Turn::stated_as("-90"),
            ..source()
        };
        let join_with = |turn_option, source: &SourceVideo| {
            joined(&join_arguments(
                Path::new("film.mp4"),
                &plan.scenes()[1],
                source,
                turn_option,
                Path::new("parts.txt"),
                None,
                Path::new("002.mp4"),
            ))
        };

        assert!(
            join_with(TurnOption::NamedAtTheJoin, &turned)
                .starts_with("-copyts -display_rotation -90 -f concat -safe 0 -i parts.txt "),
            "{}",
            join_with(TurnOption::NamedAtTheJoin, &turned)
        );
        // An ffmpeg that keeps the turn is not told, and neither is any
        // ffmpeg about a video that states none.
        assert!(!join_with(TurnOption::LeftToTheParts, &turned).contains("-display_rotation"));
        let upright = SourceVideo {
            turn: None,
            ..turned
        };
        assert!(!join_with(TurnOption::NamedAtTheJoin, &upright).contains("-display_rotation"));
    }

    #[test]
    fn a_part_is_listed_with_its_length() {
        assert_eq!(
            parts_list_entry("part_2.mp4", 1.08),
            "file 'part_2.mp4'\nduration 1.080000\n"
        );
    }

    #[test]
    fn every_failure_says_the_original_is_untouched_and_offers_the_normal_cut() {
        let failures = [
            ExactCutError::Unsupported(Unsupported::Codec {
                name: "hevc".to_owned(),
            }),
            ExactCutError::EncoderMissing,
            ExactCutError::Plan(PlanError::NoPictures),
            ExactCutError::Disk {
                doing: "create parts".to_owned(),
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            },
        ];
        for failure in failures {
            let words = failure.in_plain_words();
            assert!(words.contains("The original was not changed."), "{words}");
            assert!(
                words.contains("turn off \"Cut on the exact picture (slower)\" in Settings"),
                "{words}"
            );
        }
    }

    fn ffmpeg_found_fault() -> FfmpegError {
        FfmpegError::OutputNotUnderstood {
            program: Program::Ffmpeg,
            detail: "it reported problems while it ran".to_owned(),
        }
    }

    fn ffmpeg_went_silent() -> FfmpegError {
        FfmpegError::TimedOut {
            program: Program::Ffmpeg,
            limit: EXACT_CUT_STALL_LIMIT,
        }
    }

    #[test]
    fn what_is_known_before_writing_calls_for_a_normal_cut_and_says_why() {
        let not_h264 = Unsupported::Codec {
            name: "hevc".to_owned(),
        };
        assert_eq!(
            ExactCutError::Unsupported(not_h264.clone()).calls_for_a_normal_cut(),
            Some(WhyNotExact::KindOfVideo(not_h264))
        );
        assert_eq!(
            ExactCutError::EncoderMissing.calls_for_a_normal_cut(),
            Some(WhyNotExact::EncoderMissing)
        );
        assert_eq!(
            ExactCutError::TimeFilterMissing.calls_for_a_normal_cut(),
            Some(WhyNotExact::FfmpegTooOld)
        );
        assert_eq!(
            ExactCutError::Plan(PlanError::NoPictures).calls_for_a_normal_cut(),
            Some(WhyNotExact::Pictures(PlanError::NoPictures))
        );
    }

    #[test]
    fn an_exact_cut_that_was_not_proven_calls_for_a_normal_cut_as_a_failed_try() {
        let not_proven = ExactCutError::NotProven(VerifyError::NotProven(
            exact_verify::NotProven::SceneCount {
                planned: 2,
                written: 1,
            },
        ));
        assert_eq!(
            not_proven.calls_for_a_normal_cut().unwrap(),
            WhyNotExact::DidNotPassTheCheck {
                details: "the exact cut could not be proven good: 2 scenes were planned but 1 \
                          were written"
                    .to_owned()
            }
        );

        let scene_does_not_decode = ExactCutError::NotProven(VerifyError::DoesNotDecode {
            file: PathBuf::from("002.mp4"),
            source: ffmpeg_found_fault(),
        });
        assert!(matches!(
            scene_does_not_decode.calls_for_a_normal_cut(),
            Some(WhyNotExact::DidNotPassTheCheck { .. })
        ));
    }

    #[test]
    fn a_part_ffmpeg_refused_to_write_calls_for_a_normal_cut_as_a_failed_try() {
        for step in [
            Step::EncodePart,
            Step::CopyPart,
            Step::CopySound,
            Step::JoinScene,
        ] {
            let failure = ExactCutError::Ffmpeg {
                step,
                source: ffmpeg_found_fault(),
            };
            assert!(
                matches!(
                    failure.calls_for_a_normal_cut(),
                    Some(WhyNotExact::CouldNotBeWritten { .. })
                ),
                "{step:?}"
            );
        }
    }

    #[test]
    fn trouble_a_normal_cut_would_meet_too_stays_a_failure() {
        let failures = [
            // The drive stopped answering while a part was written.
            ExactCutError::Ffmpeg {
                step: Step::EncodePart,
                source: ffmpeg_went_silent(),
            },
            // The video could not be read at all.
            ExactCutError::Ffmpeg {
                step: Step::ReadFormat,
                source: ffmpeg_found_fault(),
            },
            ExactCutError::Ffmpeg {
                step: Step::ListPictures,
                source: ffmpeg_found_fault(),
            },
            ExactCutError::Disk {
                doing: "create parts".to_owned(),
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            },
            ExactCutError::NotProven(VerifyError::DoesNotDecode {
                file: PathBuf::from("film.mp4"),
                source: ffmpeg_went_silent(),
            }),
        ];
        for failure in failures {
            assert_eq!(failure.calls_for_a_normal_cut(), None, "{failure}");
        }
    }

    #[test]
    fn a_missing_encoder_is_named_in_the_details() {
        assert_eq!(
            ExactCutError::EncoderMissing.to_string(),
            "this ffmpeg has no libx264 encoder"
        );
        assert!(ExactCutError::EncoderMissing
            .in_plain_words()
            .starts_with("Exact cuts need an encoder that this copy of ffmpeg does not have."));
    }
}
