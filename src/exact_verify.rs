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
//! - the pictures of each scene are shown at the same pace as in the
//!   original, also across the joins,
//! - the scenes, one after another, hold every packet of the original's
//!   sound, unchanged and in order, each in the scene in which it starts,
//! - the sound of each scene is in step with its pictures as in the original.
//!
//! What it cannot prove is how the pictures that were encoded again look.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::exact_cut::ExactCutPlan;
use crate::ffmpeg::{FfmpegError, NotUnderstood, Program, STEADY_OUTPUT_LIMIT};
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

/// What reading a video from start to end gave: every picture decoded, and
/// every packet of every sound stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReadBack {
    pub pictures: Vec<DecodedPicture>,
    pub sound: BTreeMap<u32, Vec<SoundPacket>>,
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
    /// The file could not be decoded from start to end.
    DoesNotDecode {
        file: PathBuf,
        source: FfmpegError,
    },
    NotProven(NotProven),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DoesNotDecode { file, source } => {
                write!(
                    formatter,
                    "{} does not decode to its end: {source}",
                    file.display()
                )
            }
            Self::NotProven(what) => write!(formatter, "{what}"),
        }
    }
}

impl std::error::Error for VerifyError {}

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
    scene_files: &[PathBuf],
    plan: &ExactCutPlan,
    cut_times: &[f64],
) -> Result<ExactCutProof, VerifyError> {
    let read = |file: &Path, decode_errors| {
        read_back(file, decode_errors).map_err(|source| VerifyError::DoesNotDecode {
            file: file.to_path_buf(),
            source,
        })
    };
    let original_read = read(original, DecodeErrors::ReadOn)?;
    let scenes_read = scene_files
        .par_iter()
        .map(|scene| read(scene, DecodeErrors::Fail))
        .collect::<Result<Vec<_>, _>>()?;
    prove(plan, cut_times, &original_read, &scenes_read).map_err(VerifyError::NotProven)
}

/// Has ffmpeg decode every picture and list a checksum of it, and list a
/// checksum of every packet of sound without decoding it.
fn read_back(video: &Path, decode_errors: DecodeErrors) -> Result<ReadBack, FfmpegError> {
    let mut ffmpeg = tools::command(Program::Ffmpeg).args(["-nostdin", "-v", "error"]);
    if decode_errors == DecodeErrors::Fail {
        // ffmpeg is told to stop at the first error. It still carries on
        // after some pictures it could not decode properly and only writes
        // about them, so anything it writes counts as a failure too.
        ffmpeg = ffmpeg.arg("-xerror").failing_on_complaints();
    }
    let mut listing = ReadBackListing::default();
    ffmpeg
        .arg("-i")
        .arg(video)
        .args(["-map", "0:v:0", "-map", "0:a?", "-c:a", "copy"])
        // Every decoded picture is listed once, with the time the file gives
        // it. Without these ffmpeg drops or repeats pictures to even out
        // their times, and rounds the times to whole pictures.
        .args(tools::picture_times_option().arguments())
        .args(["-enc_time_base", "-1"])
        .args(["-f", "framecrc", "-"])
        .stream_lines(STEADY_OUTPUT_LIMIT, |line| {
            listing
                .take_line(line)
                .map_err(|problem| NotUnderstood(format!("checksum line {line:?}, {problem}")))
        })?;
    Ok(listing.read)
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
    let cut_landings = prove_cut_landings(plan, cut_times, original)?;
    let largest_sound_shift_seconds = prove_sound(plan, original, scenes)?;
    Ok(ExactCutProof {
        pictures: original.pictures.len(),
        pictures_encoded_again: plan.frames_encoded_again(),
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

            let scene_start = scene.pictures[0].shown_at;
            let original_start = original.pictures[scene_plan.frames.first()].shown_at;
            let ends = [0, in_scene.len() - 1];
            for place in ends {
                let seconds_off = (in_scene[place].starts_at - scene_start)
                    - (in_original[place].starts_at - original_start);
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
        ReadBack {
            pictures,
            sound: BTreeMap::from([(SOUND_STREAM, packets)]),
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
                ReadBack {
                    pictures,
                    sound: BTreeMap::from([(SOUND_STREAM, packets)]),
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
