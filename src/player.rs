//! A small video player for checking where scenes really begin and end.
//!
//! Playback is two ffmpeg processes, one decoding pictures and one decoding
//! sound, both started at the wanted position and speed. Seeking, pausing
//! and changing speed all work the same way: stop both and start again from
//! the new position. That keeps the player simple, at the cost of a short
//! pause on every jump.

use std::io::Read;
use std::ops::RangeInclusive;
use std::path::Path;
use std::process::{Child, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use eframe::egui;
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};

use crate::exact_cut::CutPrecision;
use crate::icons::Icon;
use crate::keyboard::{self, KeyboardOwner};
use crate::plan::SceneIndex;
use crate::review_session::SplitFeedback;
use crate::theme;
use crate::thumbnails::{self, DecoderThreads, PictureSize};
use crate::timecode::{format_clock, format_clock_precise, format_timestamp, TimeSpan};
use crate::tools;

/// How wide the picture is decoded, in pixels. The narrowest is enough for
/// a screen of ordinary sharpness. On a sharper screen the picture is
/// decoded larger, up to the widest: beyond that, decoding and showing
/// thirty pictures a second costs more than the sharpness is worth.
const FRAME_WIDTHS: RangeInclusive<usize> = 640..=1280;
/// Pictures shown per second of watching, whatever the playback speed.
const FRAMES_PER_SECOND: f64 = 30.0;

const AUDIO_SAMPLE_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u16 = 2;
const BYTES_PER_SAMPLE: usize = 2;
/// Sound is handed to the sound card in tenths of a second.
const AUDIO_CHUNK_BYTES: usize =
    AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS as usize * BYTES_PER_SAMPLE / 10;
/// Queued sound keeps playing after a stop is asked for, so little is queued.
const MAX_QUEUED_AUDIO_CHUNKS: usize = 3;
const AUDIO_POLL_INTERVAL: Duration = Duration::from_millis(15);

pub const SKIP_SECONDS: f64 = 10.0;
/// The picture is drawn as large as the window allows, between these widths.
const MIN_PICTURE_WIDTH: f32 = 320.0;
const MAX_PICTURE_WIDTH: f32 = 640.0;
const PICTURE_ASPECT: f32 = 9.0 / 16.0;
/// Room the player's title, slider and controls need around the picture.
const PLAYER_CHROME_WIDTH: f32 = 48.0;
const PLAYER_CHROME_HEIGHT: f32 = 340.0;
/// The bar under the picture that shows the scenes and the place in the
/// video. It is the only place the scenes can be seen in a small window,
/// where the player covers the timeline of the review screen.
const SCENE_BAR_HEIGHT: f32 = 26.0;
const SCENE_BAR_BLOCK_HEIGHT: f32 = 14.0;
const SCENE_BAR_CURRENT_BLOCK_HEIGHT: f32 = 22.0;
const SCENE_BAR_GAP: f32 = 2.0;
const SCENE_BAR_ROUNDING: f32 = 4.0;
const SCENE_BAR_HANDLE_RADIUS: f32 = 7.0;
/// How strongly the scenes that are not playing are faded on the bar.
const OTHER_SCENE_OPACITY: f32 = 0.45;
/// The frame around the picture, in the colour of the scene on screen.
const PICTURE_FRAME_WIDTH: f32 = 2.0;
/// How long the picture says which scene has just begun.
const SCENE_NOTICE_SECONDS: f64 = 2.5;
const SCENE_NOTICE_MARGIN: f32 = 12.0;
/// Around the words of a note drawn on the embedded picture.
const NOTE_PADDING: egui::Vec2 = egui::vec2(10.0, 6.0);
/// How long a note about a split stays on the embedded picture.
const NOTE_ON_PICTURE_SECONDS: f64 = 4.0;
/// How far from the corner of the window the player first appears.
const PLAYER_CORNER_GAP: f32 = 16.0;
/// On a touchpad, which scrolls smoothly, this much scrolling steps one frame.
const SCROLL_POINTS_PER_FRAME: f32 = 50.0;
/// A split lands "exactly here" when the keyframe is closer than this.
const SAME_MOMENT_SECONDS: f64 = 0.0005;
/// The split button says where its split lands: near the picture on show
/// while the video is copied, on it while cuts are exact.
const SPLIT_NEAR_LABEL: &str = "Split near here";
const SPLIT_NEAR_HINT: &str =
    "Cuts the scene playing now in two, at the closest point where the video can be cut.";
const SPLIT_EXACTLY_LABEL: &str = "Split here";
/// Shown on the split button, which the key S also presses.
const SPLIT_EXACTLY_HINT: &str = "Cuts the scene playing now in two, on this picture.";
/// Room for the two one-picture buttons side by side.
const PICTURE_STEP_BUTTONS_WIDTH: f32 = 185.0;
/// Room for the dividing line and the three speeds after it.
const SPEED_CHOICES_WIDTH: f32 = 150.0;
const ONE_PICTURE_BACK_LABEL: &str = "-1 picture";
const ONE_PICTURE_FORWARD_LABEL: &str = "+1 picture";
const PLAYER_HINT: &str =
    "Esc closes the player. Pause, then step one picture at a time to find the exact one.";
/// Between the embedded picture and the row of buttons under it.
const TRANSPORT_TOP_GAP: f32 = 8.0;
/// The row of buttons under the embedded picture.
const TRANSPORT_HEIGHT: f32 = 52.0;
/// The row of buttons under the embedded picture, with the gap above it.
const EMBEDDED_CONTROLS_HEIGHT: f32 = TRANSPORT_TOP_GAP + TRANSPORT_HEIGHT;
const TRANSPORT_GAP: f32 = 8.0;
/// The row is never narrower than its buttons with sound beside them.
const TRANSPORT_LEAST_WIDTH: f32 = 300.0;
/// A cut this close to where the video is counts as the place itself, so
/// "previous cut" moves to the one before.
const CUT_NEARBY_SECONDS: f64 = 0.25;
const PLAY_LABEL: &str = "Play";
const PAUSE_LABEL: &str = "Pause";
const PLAY_HINT: &str = "Play or pause (Space)";
const PREVIOUS_CUT_LABEL: &str = "Previous cut";
const NEXT_CUT_LABEL: &str = "Next cut";
const ONE_PICTURE_BACK_HINT: &str =
    "One picture back. The mouse wheel over the picture does the same.";
const ONE_PICTURE_FORWARD_HINT: &str = "One picture forward";
const MUTE_LABEL: &str = "Mute";
const SOUND_ON_LABEL: &str = "Turn the sound on";
const STILL_PROBLEM_NOTE: &str =
    "This picture could not be shown. Try a moment a little earlier or later.";

/// The buttons of the embedded player's row, in their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportAction {
    PreviousCut,
    PictureBack,
    PlayOrPause,
    PictureForward,
    NextCut,
}

const TIME_FONT_SIZE: f32 = 13.0;

/// What the row under the embedded picture holds in the room it has.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CompactRow {
    time: TimeShown,
    cut_steps: CutSteps,
    /// The room kept on each side of the buttons, for the time on the left
    /// and sound on the right, so play stays in the middle.
    side_room: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TimeShown {
    /// Where the video is and how long it is.
    Whole,
    /// Where the video is.
    Short,
    Hidden,
}

/// Whether the buttons to the previous and next cut are in the row.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CutSteps {
    Shown,
    LeftOut,
}

/// The widths `compact_row` chooses between.
#[derive(Debug, Clone, Copy)]
struct CompactRowWidths {
    all_buttons: f32,
    without_cut_steps: f32,
    whole_time: f32,
    short_time: f32,
}

/// Short of room, the steps to the cuts go first: the timeline shows the
/// cuts, and the time has no other place under the picture. Then the time
/// gets shorter, and last it goes.
fn compact_row(room: f32, widths: CompactRowWidths) -> CompactRow {
    let side_room_for = |time: f32| time + TRANSPORT_GAP;
    let fits = |buttons: f32, time: f32| buttons + 2.0 * side_room_for(time) <= room;
    let (cut_steps, buttons) = if fits(widths.all_buttons, widths.whole_time) {
        (CutSteps::Shown, widths.all_buttons)
    } else {
        (CutSteps::LeftOut, widths.without_cut_steps)
    };
    let (time, side_room) = if fits(buttons, widths.whole_time) {
        (TimeShown::Whole, side_room_for(widths.whole_time))
    } else if fits(buttons, widths.short_time) {
        (TimeShown::Short, side_room_for(widths.short_time))
    } else {
        (TimeShown::Hidden, theme::CONTROL_HEIGHT + TRANSPORT_GAP)
    };
    CompactRow {
        time,
        cut_steps,
        side_room,
    }
}

/// `words` with every digit made a zero, the width the time takes at most
/// as its digits change.
fn widest_digits(words: &str) -> String {
    words
        .chars()
        .map(|letter| if letter.is_ascii_digit() { '0' } else { letter })
        .collect()
}

/// Below this the embedded picture is not shrunk further, and its area scrolls.
const MIN_EMBEDDED_PICTURE_WIDTH: f32 = 160.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackSpeed {
    Normal,
    Double,
    Quadruple,
}

impl PlaybackSpeed {
    const ALL: [Self; 3] = [Self::Normal, Self::Double, Self::Quadruple];

    fn factor(self) -> f64 {
        match self {
            Self::Normal => 1.0,
            Self::Double => 2.0,
            Self::Quadruple => 4.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Normal => "1x",
            Self::Double => "2x",
            Self::Quadruple => "4x",
        }
    }

    /// The sound filter for this speed. ffmpeg's tempo filter goes up to 2x
    /// per use, so 4x chains two of them.
    fn audio_filter(self) -> &'static str {
        match self {
            Self::Normal => "anull",
            Self::Double => "atempo=2.0",
            Self::Quadruple => "atempo=2.0,atempo=2.0",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sound {
    On,
    Muted,
}

/// What the picture and sound threads share with the window.
#[derive(Default)]
struct PlaybackState {
    stop_requested: AtomicBool,
    frames_shown: AtomicU64,
    reached_the_end: AtomicBool,
    /// The newest decoded picture, taken by the window when it draws.
    newest_frame: Mutex<Option<Vec<u8>>>,
}

/// One run of playback from a fixed position at a fixed speed.
struct Playback {
    started_at: f64,
    speed: PlaybackSpeed,
    /// The size its pictures are decoded at.
    frame_size: PictureSize,
    state: Arc<PlaybackState>,
    picture_process: Child,
    /// Absent when the sound decoder could not be started.
    sound_process: Option<Child>,
}

impl Playback {
    fn start(
        video: &Path,
        position: f64,
        speed: PlaybackSpeed,
        sound: Sound,
        frame_size: PictureSize,
        ctx: &egui::Context,
    ) -> Result<Self> {
        let state = Arc::new(PlaybackState::default());

        let mut picture_process = spawn_picture_decoder(video, position, speed, frame_size)?;
        let frames = picture_process
            .stdout
            .take()
            .context("ffmpeg has no picture pipe")?;
        let picture_state = Arc::clone(&state);
        let ctx = ctx.clone();
        std::thread::spawn(move || show_frames_on_time(frames, frame_size, &picture_state, &ctx));

        // A video without sound, or a machine without a sound card, still
        // plays its pictures.
        let mut sound_process = match sound {
            Sound::On => spawn_sound_decoder(video, position, speed).ok(),
            Sound::Muted => None,
        };
        if let Some(samples) = sound_process
            .as_mut()
            .and_then(|process| process.stdout.take())
        {
            let sound_state = Arc::clone(&state);
            std::thread::spawn(move || play_sound(samples, &sound_state));
        }

        Ok(Self {
            started_at: position,
            speed,
            frame_size,
            state,
            picture_process,
            sound_process,
        })
    }

    fn position(&self) -> f64 {
        let frames_shown = self.state.frames_shown.load(Ordering::Relaxed);
        position_after(self.started_at, frames_shown, self.speed)
    }
}

impl Drop for Playback {
    /// Stops both decoders and waits for them to exit, so the video file is
    /// closed by the time this returns. The original can then be recycled.
    fn drop(&mut self) {
        self.state.stop_requested.store(true, Ordering::Relaxed);
        for process in std::iter::once(&mut self.picture_process).chain(&mut self.sound_process) {
            // Killing a process that already exited fails, which is fine.
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

/// Where playback is in the video after showing some frames.
fn position_after(started_at: f64, frames_shown: u64, speed: PlaybackSpeed) -> f64 {
    started_at + frames_shown as f64 / FRAMES_PER_SECOND * speed.factor()
}

fn spawn_picture_decoder(
    video: &Path,
    position: f64,
    speed: PlaybackSpeed,
    frame_size: PictureSize,
) -> Result<Child> {
    let filter = format!(
        "setpts=PTS/{},fps={FRAMES_PER_SECOND},{}",
        speed.factor(),
        frame_size.letterbox_filter()
    );
    tools::ffmpeg_command()
        .args(["-nostdin", "-v", "error"])
        .args(["-ss", &format!("{position:.3}")])
        .arg("-i")
        .arg(video)
        .args(["-map", "0:v:0", "-an", "-vf", &filter])
        .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start ffmpeg, is it on PATH?")
}

fn spawn_sound_decoder(video: &Path, position: f64, speed: PlaybackSpeed) -> Result<Child> {
    tools::ffmpeg_command()
        .args(["-nostdin", "-v", "error"])
        .args(["-ss", &format!("{position:.3}")])
        .arg("-i")
        .arg(video)
        .args(["-map", "0:a:0", "-vn", "-af", speed.audio_filter()])
        .args(["-ac", &AUDIO_CHANNELS.to_string()])
        .args(["-ar", &AUDIO_SAMPLE_RATE.to_string()])
        .args(["-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start ffmpeg, is it on PATH?")
}

/// Hands each decoded picture to the window at its moment. ffmpeg decodes
/// faster than real time and simply waits whenever this thread is not
/// reading.
fn show_frames_on_time(
    mut frames: ChildStdout,
    frame_size: PictureSize,
    state: &PlaybackState,
    ctx: &egui::Context,
) {
    let mut frame = vec![0u8; frame_size.rgb_bytes()];
    let mut first_frame_at = None;
    let mut frames_shown = 0u64;
    // The read fails when the video ends or when the decoder is stopped.
    while frames.read_exact(&mut frame).is_ok() {
        if state.stop_requested.load(Ordering::Relaxed) {
            return;
        }
        let first_frame_at = *first_frame_at.get_or_insert_with(Instant::now);
        let due = first_frame_at + Duration::from_secs_f64(frames_shown as f64 / FRAMES_PER_SECOND);
        if let Some(early_by) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(early_by);
        }

        *state
            .newest_frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(frame.clone());
        frames_shown += 1;
        state.frames_shown.store(frames_shown, Ordering::Relaxed);
        ctx.request_repaint();
    }
    if !state.stop_requested.load(Ordering::Relaxed) {
        state.reached_the_end.store(true, Ordering::Relaxed);
        ctx.request_repaint();
    }
}

/// Feeds decoded sound to the sound card. The sound device has to be opened
/// and kept on this thread, so it lives and dies with the playback.
fn play_sound(mut samples: ChildStdout, state: &PlaybackState) {
    let Ok((_output_stream, output)) = OutputStream::try_default() else {
        eprintln!("No sound device found, playing without sound.");
        return;
    };
    let Ok(sink) = Sink::try_new(&output) else {
        eprintln!("The sound device could not be opened, playing without sound.");
        return;
    };

    let stopped = || state.stop_requested.load(Ordering::Relaxed);
    let mut chunk = vec![0u8; AUDIO_CHUNK_BYTES];
    while !stopped() && samples.read_exact(&mut chunk).is_ok() {
        let decoded: Vec<i16> = chunk
            .as_chunks::<BYTES_PER_SAMPLE>()
            .0
            .iter()
            .map(|sample| i16::from_le_bytes(*sample))
            .collect();
        sink.append(SamplesBuffer::new(
            AUDIO_CHANNELS,
            AUDIO_SAMPLE_RATE,
            decoded,
        ));
        while sink.len() > MAX_QUEUED_AUDIO_CHUNKS && !stopped() {
            std::thread::sleep(AUDIO_POLL_INTERVAL);
        }
    }
    // At the end of the video, let the last queued sound play out.
    while !sink.empty() && !stopped() {
        std::thread::sleep(AUDIO_POLL_INTERVAL);
    }
}

/// Decodes the one picture shown at `position`, at `frame_size`. Only one
/// is grabbed at a time, so ffmpeg may use every core for it.
fn grab_still(video: &Path, position: f64, frame_size: PictureSize) -> Result<Vec<u8>> {
    thumbnails::grab_frame(video, position, frame_size, DecoderThreads::Automatic)
        .with_context(|| format!("could not get the picture at {position:.3}s"))
}

/// The size to decode the player's picture at on a screen that puts
/// `pixels_per_point` pixels in every point. It goes by the largest the
/// picture is ever drawn and not by its size right now, so resizing the
/// window never restarts the decoder.
fn frame_size_for(pixels_per_point: f32) -> PictureSize {
    thumbnails::pixel_size_for(MAX_PICTURE_WIDTH, pixels_per_point, FRAME_WIDTHS)
}

/// How many frames one turn of the wheel moves. Scrolling down, a negative
/// turn, moves forward. A mouse wheel reports whole notches. A touchpad
/// reports many small movements, which are added up in `points_left_over`.
fn frames_for_wheel_turn(unit: egui::MouseWheelUnit, turn: f32, points_left_over: &mut f32) -> i32 {
    match unit {
        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
            if turn < 0.0 {
                1
            } else if turn > 0.0 {
                -1
            } else {
                0
            }
        }
        egui::MouseWheelUnit::Point => {
            *points_left_over += turn;
            let whole_frames = (*points_left_over / SCROLL_POINTS_PER_FRAME).trunc();
            *points_left_over -= whole_frames * SCROLL_POINTS_PER_FRAME;
            -(whole_frames as i32)
        }
    }
}

/// Says where a split at `position` would really be cut. Splits without
/// re-encoding can only cut on a keyframe, so the person should see how far
/// that is from the picture they picked. The word "keyframe" stays out of
/// it, since nobody needs to know it to use the answer.
fn landing_description(position: f64, split_point: f64) -> String {
    let gap = split_point - position;
    if gap.abs() < SAME_MOMENT_SECONDS {
        return "A split would be exactly at this picture.".to_owned();
    }
    let direction = if gap > 0.0 { "after" } else { "before" };
    format!(
        "A split would be {:.2} s {direction} this picture, at {}. A video can only be split at certain points.",
        gap.abs(),
        format_timestamp(split_point)
    )
}

/// What the player's split button would do where the video stands now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitHere {
    /// Whether a split lands on the very picture or on the nearest point
    /// where the video can be cut. The button says which.
    pub precision: CutPrecision,
    /// The time a split at the current position would be cut at, if a
    /// split is possible there.
    pub would_land_at: Option<f64>,
}

impl SplitHere {
    fn button_label(self) -> &'static str {
        match self.precision {
            CutPrecision::NearestKeyframe => SPLIT_NEAR_LABEL,
            CutPrecision::ExactFrame => SPLIT_EXACTLY_LABEL,
        }
    }

    fn button_hint(self) -> &'static str {
        match self.precision {
            CutPrecision::NearestKeyframe => SPLIT_NEAR_HINT,
            CutPrecision::ExactFrame => SPLIT_EXACTLY_HINT,
        }
    }
}

enum PlayState {
    Playing(Playback),
    Paused {
        position: f64,
    },
    /// ffmpeg could not be started.
    Failed(String),
}

/// What the person did with the player window during one frame.
#[derive(Debug, PartialEq)]
pub enum PlayerEvent {
    StillOpen,
    Closed,
    /// The person asked to split the scene at this time of the video.
    SplitRequested(f64),
}

/// What a screen around the player asks of it, from controls the player
/// does not draw itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerAsk {
    /// Go to this place, playing on if it was playing.
    GoTo(f64),
    /// The playhead is being dragged here. Playing on from every place the
    /// drag passes would stutter, so the player holds the place, and shows
    /// its picture when paused.
    ScrubTo(f64),
    /// The drag of the playhead is over: go where it ended, playing on if
    /// it was playing.
    ScrubEnded,
    /// Split near the place on screen, as the S key does.
    SplitHere,
}

pub struct Player {
    video: Arc<Path>,
    video_end: f64,
    speed: PlaybackSpeed,
    sound: Sound,
    state: PlayState,
    picture: Option<egui::TextureHandle>,
    /// Where the person is dragging the position slider to, until released.
    scrubbing_to: Option<f64>,
    /// Set by the split button or key, and reported once by `show`.
    split_requested: bool,
    /// Set by the Escape key, and reported by `show`.
    close_requested: bool,
    /// Where the player was last drawn, which decides what its hint says.
    place: PlayerPlace,
    /// True while the "turn the sound on?" question is on screen.
    asking_to_turn_sound_on: bool,
    /// What the last split asked for here did, once one was.
    split_note: Option<SplitNote>,
    /// Why the picture at the place the person stepped to could not be
    /// shown, until a picture can be shown again.
    still_problem: Option<String>,
    /// How long one frame of the video lasts, the size of one wheel step.
    frame_seconds: f64,
    stills_sender: Sender<Still>,
    stills: Receiver<Still>,
    /// Counts requests for a still picture, so that only the answer to the
    /// newest request is shown when the wheel is turned quickly.
    still_request: u64,
    /// True while a still picture is being decoded.
    still_being_grabbed: bool,
    /// The place to grab a still at once the one in progress is done.
    still_wanted_next: Option<f64>,
    /// Smooth scrolling that has not yet added up to a whole frame.
    scroll_points_left_over: f32,
    /// How large the picture is drawn in this window, in points.
    picture_size: egui::Vec2,
    /// How large the picture is decoded, in pixels, for this screen.
    frame_size: PictureSize,
    /// The scene the position was in when the player was last drawn.
    scene_on_screen: Option<SceneIndex>,
    /// The scene that has just begun, said over the picture until the time
    /// given (in the clock of the window).
    scene_notice: Option<SceneNotice>,
}

/// "Scene 3" written over the picture for a moment after the video moves
/// into another scene.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SceneNotice {
    scene: SceneIndex,
    shown_until: f64,
}

/// One paused picture, grabbed after stepping to a frame.
struct Still {
    request: u64,
    size: PictureSize,
    picture: Result<Vec<u8>>,
}

impl Player {
    pub fn play_from(
        video: Arc<Path>,
        video_end: f64,
        frame_seconds: f64,
        position: f64,
        ctx: &egui::Context,
    ) -> Self {
        let mut player = Self::paused_at(video, video_end, frame_seconds, position, ctx);
        player.play_at(position, ctx);
        player
    }

    /// A player paused at `position`, showing the picture there once it is
    /// grabbed.
    pub fn showing_still_at(
        video: Arc<Path>,
        video_end: f64,
        frame_seconds: f64,
        position: f64,
        ctx: &egui::Context,
    ) -> Self {
        let mut player = Self::paused_at(video, video_end, frame_seconds, position, ctx);
        player.pause_at(position, ctx);
        player
    }

    /// A player that shows nothing yet and has started no decoder.
    pub(crate) fn paused_at(
        video: Arc<Path>,
        video_end: f64,
        frame_seconds: f64,
        position: f64,
        ctx: &egui::Context,
    ) -> Self {
        let (stills_sender, stills) = mpsc::channel();
        Self {
            video,
            video_end,
            speed: PlaybackSpeed::Normal,
            // Sound starts off, so opening the player never blares.
            sound: Sound::Muted,
            state: PlayState::Paused { position },
            picture: None,
            scrubbing_to: None,
            split_requested: false,
            close_requested: false,
            place: PlayerPlace::Window,
            asking_to_turn_sound_on: false,
            split_note: None,
            still_problem: None,
            frame_seconds,
            stills_sender,
            stills,
            still_request: 0,
            still_being_grabbed: false,
            still_wanted_next: None,
            scroll_points_left_over: 0.0,
            picture_size: picture_size_for(ctx.screen_rect().size()),
            frame_size: frame_size_for(ctx.pixels_per_point()),
            scene_on_screen: None,
            scene_notice: None,
        }
    }

    /// Carries out what the screen around the player asked of it.
    pub fn act_on(&mut self, ask: PlayerAsk, ctx: &egui::Context) {
        match ask {
            PlayerAsk::GoTo(position) => self.seek(position, ctx),
            PlayerAsk::ScrubTo(position) => self.scrub_to(position, ctx),
            PlayerAsk::ScrubEnded => self.end_scrub(ctx),
            PlayerAsk::SplitHere => self.split_requested = true,
        }
    }

    pub fn position(&self) -> f64 {
        let position = match &self.state {
            PlayState::Playing(playback) => playback.position(),
            PlayState::Paused { position } => *position,
            PlayState::Failed(_) => 0.0,
        };
        position.clamp(0.0, self.video_end)
    }

    /// Jumps to `position`, playing from there. Also used to restart
    /// playback after a speed change.
    pub fn play_at(&mut self, position: f64, ctx: &egui::Context) {
        let position = position.clamp(0.0, self.video_end);
        // A still that is on its way must not replace the moving picture.
        self.still_request += 1;
        self.still_wanted_next = None;
        // The old decoders are stopped before new ones start, so two sound
        // tracks never play over each other.
        self.state = PlayState::Paused { position };
        let started = Playback::start(
            &self.video,
            position,
            self.speed,
            self.sound,
            self.frame_size,
            ctx,
        );
        self.state = match started {
            Ok(playback) => PlayState::Playing(playback),
            Err(error) => PlayState::Failed(format!("{error:#}")),
        };
    }

    /// Pauses and moves by whole frames, for finding the exact frame where
    /// a scene changes.
    fn step_frames(&mut self, frames: i32, ctx: &egui::Context) {
        let stepped = self.position() + f64::from(frames) * self.frame_seconds;
        self.pause_at(stepped, ctx);
    }

    /// Moves to `position` without changing whether the video is playing.
    /// A paused video stays paused and shows the picture at the new place.
    fn seek(&mut self, position: f64, ctx: &egui::Context) {
        match self.state {
            PlayState::Playing(_) => self.play_at(position, ctx),
            PlayState::Paused { .. } | PlayState::Failed(_) => self.pause_at(position, ctx),
        }
    }

    /// Stops at `position` and shows the picture there.
    fn pause_at(&mut self, position: f64, ctx: &egui::Context) {
        let position = position.clamp(0.0, self.video_end);
        self.state = PlayState::Paused { position };

        // One picture is grabbed at a time. Turning the wheel quickly would
        // otherwise start a decoder for every notch, and only the picture at
        // the place the person ends up matters.
        if self.still_being_grabbed {
            self.still_wanted_next = Some(position);
        } else {
            self.grab_still_at(position, ctx);
        }
    }

    fn grab_still_at(&mut self, position: f64, ctx: &egui::Context) {
        self.still_being_grabbed = true;
        self.still_request += 1;
        let request = self.still_request;
        let size = self.frame_size;
        let video = Arc::clone(&self.video);
        let stills_sender = self.stills_sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let still = Still {
                request,
                size,
                picture: grab_still(&video, position, size),
            };
            // The send only fails once the player is closed.
            if stills_sender.send(still).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn pause(&mut self) {
        self.state = PlayState::Paused {
            position: self.position(),
        };
    }

    /// Holds the playhead where it is dragged. A drag held still asks for
    /// the same place every frame, which starts nothing new.
    fn scrub_to(&mut self, position: f64, ctx: &egui::Context) {
        if self.scrubbing_to == Some(position) {
            return;
        }
        self.scrubbing_to = Some(position);
        if matches!(self.state, PlayState::Paused { .. }) {
            // Paused, the picture follows the drag. Stills are grabbed one
            // at a time, so a fast drag starts few.
            self.pause_at(position, ctx);
        }
    }

    fn end_scrub(&mut self, ctx: &egui::Context) {
        let Some(position) = self.scrubbing_to.take() else {
            return;
        };
        if matches!(self.state, PlayState::Playing(_)) {
            self.play_at(position, ctx);
        }
    }

    /// Where the playhead is drawn: where it is dragged to, during a drag.
    pub fn playhead(&self) -> f64 {
        self.scrubbing_to.unwrap_or_else(|| self.position())
    }

    fn toggle_pause(&mut self, ctx: &egui::Context) {
        match self.state {
            PlayState::Playing(_) => self.pause(),
            PlayState::Paused { position } => self.play_at(position, ctx),
            PlayState::Failed(_) => {}
        }
    }

    fn skip(&mut self, seconds: f64, ctx: &egui::Context) {
        self.seek(self.position() + seconds, ctx);
    }

    /// Asks in front of the player, in the middle of it, with everything
    /// else dimmed and out of reach until the person has answered. A click
    /// on the dimmed part, like Escape, means no.
    fn ask_before_turning_sound_on(&mut self, ctx: &egui::Context, player_window: egui::Rect) {
        if !self.asking_to_turn_sound_on {
            return;
        }
        // The player is a floating window, so the dimming has to go on a
        // layer above floating windows to cover it.
        let clicked_outside =
            theme::show_backdrop(ctx, "sound_question_backdrop", egui::Order::Foreground);
        let question = egui::Window::new("Turn the sound on?")
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(player_window.center())
            .constrain_to(ctx.screen_rect())
            .show(ctx, |ui| {
                ui.label("The video's sound will play through your speakers.");
                ui.horizontal(|ui| {
                    if ui.button("Turn sound on").clicked() {
                        self.asking_to_turn_sound_on = false;
                        self.set_sound(Sound::On, ui.ctx());
                    }
                    if ui.button("Keep it muted").clicked() {
                        self.asking_to_turn_sound_on = false;
                    }
                });
            });
        // The dimming shares its layer order with the question. A click on
        // it would otherwise bring it to the front, over the question.
        if let Some(question) = question {
            ctx.move_to_top(question.response.layer_id);
        }
        if clicked_outside {
            self.asking_to_turn_sound_on = false;
        }
    }

    fn set_sound(&mut self, sound: Sound, ctx: &egui::Context) {
        self.sound = sound;
        if matches!(self.state, PlayState::Playing(_)) {
            self.play_at(self.position(), ctx);
        }
    }

    fn set_speed(&mut self, speed: PlaybackSpeed, ctx: &egui::Context) {
        if self.speed == speed {
            return;
        }
        self.speed = speed;
        if matches!(self.state, PlayState::Playing(_)) {
            self.play_at(self.position(), ctx);
        }
    }

    /// Draws the player window and handles its buttons. Its keys are taken
    /// by `take_keys`, which the app calls earlier in the frame.
    ///
    /// `split_here` says what a split at the current position would do.
    /// `scenes` are the scenes of the video as the review has them now, in
    /// order. The player shows which one is on screen.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        scenes: &[TimeSpan],
        split_here: SplitHere,
    ) -> PlayerEvent {
        self.place = PlayerPlace::Window;
        self.keep_up_with_the_video(ctx);

        // The window can be resized under the player, and the player must
        // still fit inside it with its controls in reach.
        let screen = ctx.screen_rect();
        self.picture_size = picture_size_for(screen.size());

        let mut open = true;
        let player_window = egui::Window::new("Player")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            // Starts in the bottom right corner, where it covers the end of
            // the scene list and leaves the timeline and first scenes clear.
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .default_pos(screen.right_bottom() - egui::vec2(PLAYER_CORNER_GAP, PLAYER_CORNER_GAP))
            .constrain_to(screen)
            .show(ctx, |ui| self.show_inside(ui, scenes, split_here))
            .map_or(screen, |shown| shown.response.rect);
        self.ask_before_turning_sound_on(ctx, player_window);
        if !open || self.close_requested {
            return PlayerEvent::Closed;
        }
        self.split_event()
    }

    /// Draws the player into `ui`, as part of a screen, with the picture
    /// as large as the room allows. It cannot be closed there: Escape is
    /// left for whatever else the screen does with it.
    pub fn show_embedded(
        &mut self,
        ui: &mut egui::Ui,
        scenes: &[TimeSpan],
        split_here: SplitHere,
    ) -> PlayerEvent {
        self.place = PlayerPlace::Embedded;
        self.close_requested = false;
        self.keep_up_with_the_video(ui.ctx());
        let area = ui
            .scope(|ui| {
                // Every gap is counted in EMBEDDED_CONTROLS_HEIGHT, so egui
                // must add none, or the row is pushed out of its card.
                ui.spacing_mut().item_spacing.y = 0.0;
                // The picture and its controls sit in the middle of the
                // room, so whatever room is left over is shared above and
                // below.
                let room = ui.available_size();
                self.picture_size = picture_size_within(room);
                let left_over = room.y - self.picture_size.y - EMBEDDED_CONTROLS_HEIGHT;
                ui.add_space((left_over / 2.0).max(0.0));
                ui.vertical_centered(|ui| self.show_inside(ui, scenes, split_here))
                    .response
                    .rect
            })
            .inner;
        self.ask_before_turning_sound_on(&ui.ctx().clone(), area);
        self.split_event()
    }

    /// Takes the newest decoded picture and still, and keeps decoding at the
    /// sharpness the screen needs. Called once a frame, before drawing.
    fn keep_up_with_the_video(&mut self, ctx: &egui::Context) {
        self.take_newest_frame(ctx);
        self.take_newest_still(ctx);
        self.pause_at_the_end();
        self.decode_for_the_sharpness_of_the_screen(ctx);
    }

    /// The picture, the scene bar and the controls.
    fn show_inside(&mut self, ui: &mut egui::Ui, scenes: &[TimeSpan], split_here: SplitHere) {
        let scene = scene_at(scenes, self.position());
        self.notice_a_new_scene(scene, ui.ctx());
        let mut picture = self.show_picture(ui, scene);
        if let Some(problem) = &self.still_problem {
            // The note on the picture is short; the reason is a hover away.
            picture = picture.on_hover_text(problem);
        }
        if picture.contains_pointer() {
            self.step_frames_with_the_wheel(ui.ctx());
        }
        match self.place {
            PlayerPlace::Window => self.show_scene_bar(ui, scenes, scene),
            // The timeline under it shows the scenes and moves the playhead.
            PlayerPlace::Embedded => {
                let now = ui.input(|input| input.time);
                if let Some(left) = self.expire_split_note(now) {
                    ui.ctx().request_repaint_after(left);
                }
                self.show_note_on_picture(ui, picture.rect);
                ui.add_space(TRANSPORT_TOP_GAP);
            }
        }
        self.show_controls(ui, scenes, scene, split_here);
    }

    fn split_event(&mut self) -> PlayerEvent {
        if std::mem::take(&mut self.split_requested) {
            return PlayerEvent::SplitRequested(self.position());
        }
        PlayerEvent::StillOpen
    }

    /// Whether the question about sound is open in front of the player.
    /// Until it is answered, the keys are not the review screen's either.
    pub fn is_asking_about_sound(&self) -> bool {
        self.asking_to_turn_sound_on
    }

    /// Shows the outcome of a split in the player.
    pub fn show_split_feedback(&mut self, feedback: SplitFeedback) {
        self.split_note = Some(SplitNote {
            feedback,
            shown_until: None,
        });
    }

    /// Takes the note about a split off the embedded picture once it has
    /// been there long enough. Returns how long it has left, for a repaint
    /// to take it off then.
    fn expire_split_note(&mut self, now: f64) -> Option<Duration> {
        let note = self.split_note.as_mut()?;
        let until = *note
            .shown_until
            .get_or_insert(now + NOTE_ON_PICTURE_SECONDS);
        if now >= until {
            self.split_note = None;
            return None;
        }
        Some(Duration::from_secs_f64(until - now))
    }

    fn take_newest_frame(&mut self, ctx: &egui::Context) {
        let PlayState::Playing(playback) = &self.state else {
            return;
        };
        let newest_frame = playback
            .state
            .newest_frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let size = playback.frame_size;
        if let Some(frame) = newest_frame {
            self.set_picture(ctx, size, &frame);
        }
    }

    /// Moving the window to a sharper or coarser screen, or zooming, changes
    /// how many pixels the picture covers. The picture is then decoded
    /// afresh at the size that suits, from where the video is.
    fn decode_for_the_sharpness_of_the_screen(&mut self, ctx: &egui::Context) {
        let wanted = frame_size_for(ctx.pixels_per_point());
        if wanted == self.frame_size {
            return;
        }
        self.frame_size = wanted;
        if !matches!(self.state, PlayState::Failed(_)) {
            self.seek(self.position(), ctx);
        }
    }

    fn take_newest_still(&mut self, ctx: &egui::Context) {
        let stills: Vec<Still> = self.stills.try_iter().collect();
        for still in stills {
            self.still_being_grabbed = false;
            if still.request != self.still_request {
                continue;
            }
            match still.picture {
                Ok(frame) => self.set_picture(ctx, still.size, &frame),
                Err(error) => self.still_problem = Some(format!("{error:#}")),
            }
        }
        if !self.still_being_grabbed {
            if let Some(position) = self.still_wanted_next.take() {
                self.grab_still_at(position, ctx);
            }
        }
    }

    /// One wheel notch is one frame. Scrolling down moves forward in time.
    fn step_frames_with_the_wheel(&mut self, ctx: &egui::Context) {
        let wheel_turns: Vec<(egui::MouseWheelUnit, f32)> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::MouseWheel { unit, delta, .. } => Some((*unit, delta.y)),
                    _ => None,
                })
                .collect()
        });
        let frames: i32 = wheel_turns
            .into_iter()
            .map(|(unit, delta)| {
                frames_for_wheel_turn(unit, delta, &mut self.scroll_points_left_over)
            })
            .sum();
        if frames != 0 {
            self.step_frames(frames, ctx);
        }
    }

    fn set_picture(&mut self, ctx: &egui::Context, size: PictureSize, frame: &[u8]) {
        // The picture on screen is that of the place the video is at again.
        self.still_problem = None;
        let image = egui::ColorImage::from_rgb([size.width, size.height], frame);
        match &mut self.picture {
            Some(picture) => picture.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.picture =
                    Some(ctx.load_texture("player_picture", image, egui::TextureOptions::LINEAR));
            }
        }
    }

    fn pause_at_the_end(&mut self) {
        let ended = matches!(&self.state, PlayState::Playing(playback)
            if playback.state.reached_the_end.load(Ordering::Relaxed));
        if ended {
            self.pause();
        }
    }

    /// Acts on the keys pressed in this frame that are the player's.
    ///
    /// The app calls this before it draws the review screen, which is behind
    /// the player. Escape closes the innermost thing that is open, so the
    /// player has to take it before the review screen can read it as "clear
    /// the selection".
    pub fn take_keys(&mut self, ctx: &egui::Context, keyboard: KeyboardOwner) {
        if !keyboard.lets_screens_use_keys(ctx) {
            return;
        }
        if self.asking_to_turn_sound_on {
            // The question is answered before anything else is done with
            // the player. Escape says no to it, and the player stays open.
            if keyboard::take_escape(ctx) {
                self.asking_to_turn_sound_on = false;
            }
            return;
        }
        // Part of the review screen, the player cannot be closed, so Escape
        // is left for the review, which clears its selection with it.
        if self.place == PlayerPlace::Window && keyboard::take_escape(ctx) {
            self.close_requested = true;
        }
        // The review screen behind the player reads the arrow keys, Space
        // and S too, as "another picture", "play this scene" and "the scene
        // below". They are taken here, so one press does one thing. With Alt
        // held an arrow key moves a split in the review, and is left for it.
        let back_arrow = keyboard::take_plain_key(ctx, egui::Key::ArrowLeft);
        let forward_arrow = keyboard::take_plain_key(ctx, egui::Key::ArrowRight);
        let pause = keyboard::take_plain_key(ctx, egui::Key::Space);
        let split = keyboard::take_plain_key(ctx, keyboard::SPLIT_SHORTCUT.logical_key);
        let (back_letter, forward_letter) = ctx.input(|input| {
            // With Ctrl held a letter is a shortcut of the review screen,
            // such as "select all", and not the player's.
            let plain_key = |key| input.key_pressed(key) && !input.modifiers.command;
            (plain_key(egui::Key::A), plain_key(egui::Key::D))
        });
        let back = back_arrow || back_letter;
        let forward = forward_arrow || forward_letter;
        if split {
            self.split_requested = true;
        }
        if back {
            self.skip(-SKIP_SECONDS, ctx);
        }
        if forward {
            self.skip(SKIP_SECONDS, ctx);
        }
        if pause {
            self.toggle_pause(ctx);
        }
    }

    /// Remembers that the video has moved into another scene, so that the
    /// picture can say so for a moment. Opening the player is not a move.
    fn notice_a_new_scene(&mut self, scene: Option<SceneIndex>, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        if let (Some(before), Some(scene)) = (self.scene_on_screen, scene) {
            if before != scene {
                self.scene_notice = Some(SceneNotice {
                    scene,
                    shown_until: now + SCENE_NOTICE_SECONDS,
                });
            }
        }
        self.scene_on_screen = scene;

        match self.scene_notice {
            Some(notice) if notice.shown_until > now => {
                // A paused player draws nothing by itself, and the notice
                // still has to go away.
                ctx.request_repaint_after(Duration::from_secs_f64(notice.shown_until - now));
            }
            _ => self.scene_notice = None,
        }
    }

    /// Draws the picture.
    fn show_picture(&self, ui: &mut egui::Ui, scene: Option<SceneIndex>) -> egui::Response {
        let palette = theme::palette(ui.ctx());
        let size = self.picture_size;
        let response = match (&self.state, &self.picture) {
            (PlayState::Failed(reason), _) => {
                let (area, response) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().text(
                    area.center(),
                    egui::Align2::CENTER_CENTER,
                    reason,
                    egui::FontId::default(),
                    ui.visuals().error_fg_color,
                );
                response
            }
            (_, Some(picture)) => ui.image((picture.id(), size)),
            (_, None) => {
                let (area, response) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter()
                    .rect_filled(area, 0.0, palette.picture_backdrop);
                response
            }
        };
        if let Some(scene) = scene {
            // The frame changes colour the moment the scene does, which is
            // seen without reading anything.
            let frame =
                egui::Stroke::new(PICTURE_FRAME_WIDTH, palette.scene_color(scene.position()));
            ui.painter()
                .rect_stroke(response.rect.shrink(PICTURE_FRAME_WIDTH / 2.0), 0.0, frame);
        }
        if let Some(notice) = self.scene_notice {
            show_scene_notice(ui, response.rect, notice.scene);
        }
        response
    }

    /// What a split just did, and a picture that could not be shown, in a
    /// dark band along the bottom of the embedded picture. The note about a
    /// split goes after a few seconds; the timeline shows the split itself.
    fn show_note_on_picture(&self, ui: &egui::Ui, picture: egui::Rect) {
        let mut lines = Vec::new();
        if let Some(note) = &self.split_note {
            let color = theme::picture_note_color(note.feedback.tone());
            lines.push((note.feedback.in_words(), color));
        }
        if self.still_problem.is_some() {
            lines.push((STILL_PROBLEM_NOTE.to_owned(), theme::PICTURE_NOTE_PROBLEM));
        }
        if lines.is_empty() {
            return;
        }
        let painter = ui.painter();
        let wrap_width = picture.width() - 2.0 * NOTE_PADDING.x;
        let font = egui::FontId::proportional(theme::PICTURE_NOTE_FONT_SIZE);
        let texts: Vec<_> = lines
            .into_iter()
            .map(|(words, color)| painter.layout(words, font.clone(), color, wrap_width))
            .collect();
        let text_height: f32 = texts.iter().map(|text| text.size().y).sum();
        let band = egui::Rect::from_min_max(
            egui::pos2(
                picture.left(),
                picture.bottom() - text_height - 2.0 * NOTE_PADDING.y,
            ),
            picture.right_bottom(),
        );
        painter.rect_filled(band, 0.0, theme::PICTURE_NOTE_FILL);
        let mut at = band.min + NOTE_PADDING;
        for text in texts {
            let height = text.size().y;
            painter.galley(at, text, theme::PICTURE_NOTE_TEXT);
            at.y += height;
        }
    }

    /// The scenes of the video as coloured blocks, with the place in the
    /// video marked. Clicking or dragging on it moves there.
    fn show_scene_bar(
        &mut self,
        ui: &mut egui::Ui,
        scenes: &[TimeSpan],
        scene: Option<SceneIndex>,
    ) {
        let palette = theme::palette(ui.ctx());
        let size = egui::vec2(self.picture_size.x, SCENE_BAR_HEIGHT);
        let (bar, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        // The handle is a circle, and half of it must not hang over the end.
        let track = bar.shrink2(egui::vec2(SCENE_BAR_HANDLE_RADIUS, 0.0));
        let video_end = self.video_end;
        let x_at = |time: f64| track.left() + track.width() * share_of_video(time, video_end);

        let painter = ui.painter();
        if scenes.is_empty() {
            let block = egui::Rect::from_center_size(
                track.center(),
                egui::vec2(track.width(), SCENE_BAR_BLOCK_HEIGHT),
            );
            painter.rect_filled(block, SCENE_BAR_ROUNDING, palette.muted_text);
        }
        for (position, span) in scenes.iter().enumerate() {
            let is_on_screen = scene == Some(SceneIndex::new(position));
            let height = if is_on_screen {
                SCENE_BAR_CURRENT_BLOCK_HEIGHT
            } else {
                SCENE_BAR_BLOCK_HEIGHT
            };
            let left = x_at(span.start);
            let right = (x_at(span.end) - SCENE_BAR_GAP).max(left + 1.0);
            let block = egui::Rect::from_min_max(
                egui::pos2(left, track.center().y - height / 2.0),
                egui::pos2(right, track.center().y + height / 2.0),
            );
            let colour = palette.scene_color(position);
            let fill = if is_on_screen {
                colour
            } else {
                colour.gamma_multiply(OTHER_SCENE_OPACITY)
            };
            painter.rect_filled(block, SCENE_BAR_ROUNDING, fill);
        }

        let position = self.scrubbing_to.unwrap_or_else(|| self.position());
        let handle = egui::pos2(x_at(position), track.center().y);
        painter.circle(
            handle,
            SCENE_BAR_HANDLE_RADIUS,
            palette.card,
            egui::Stroke::new(2.0_f32, palette.text),
        );

        let pointed_time = response
            .interact_pointer_pos()
            .or(response.hover_pos())
            .map(|pointer| {
                let share = (pointer.x - track.left()) / track.width().max(1.0);
                f64::from(share.clamp(0.0, 1.0)) * video_end
            });
        if let (true, Some(time)) = (response.dragged(), pointed_time) {
            self.scrub_to(time, ui.ctx());
        } else if response.drag_stopped() {
            self.end_scrub(ui.ctx());
        } else if let (true, Some(time)) = (response.clicked(), pointed_time) {
            self.seek(time, ui.ctx());
        }
        if let Some(time) = pointed_time {
            let place = match scene_at(scenes, time) {
                Some(pointed_scene) => {
                    format!("Scene {}, {}", pointed_scene.number(), format_clock(time))
                }
                None => format_clock(time),
            };
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text_at_pointer(place);
        }
    }

    /// The row under the picture of the player that is part of the review
    /// screen, as wide as the picture: where the video is on the left, the
    /// ways to move in the middle with play at their centre, sound on the
    /// right. The split is on the timeline, with the cuts it makes.
    fn show_compact_controls(&mut self, ui: &mut egui::Ui, scenes: &[TimeSpan]) {
        let palette = theme::palette(ui.ctx());
        let paused = !matches!(self.state, PlayState::Playing(_));
        let position = self.position();
        let time_in_words = |words: String| {
            ui.painter()
                .layout_no_wrap(words, theme::semibold_font(TIME_FONT_SIZE), palette.text)
        };
        let whole_time = |at: f64| {
            format!(
                "{} / {}",
                format_clock_precise(at),
                format_clock(self.video_end)
            )
        };
        // The length of the video is on the ruler of the timeline, so short
        // of room only where the video is stays.
        let short_time = format_clock_precise;
        // Measured with the last moment of the video in zeros, so the row
        // does not change as the digits do while the video plays.
        let widest = |words: String| time_in_words(widest_digits(&words)).size().x;
        let (play_icon, play_label) = if paused {
            (Icon::Play, PLAY_LABEL)
        } else {
            (Icon::Pause, PAUSE_LABEL)
        };
        let round = |icon, label| {
            theme::ToolButton::new(icon, label, theme::ButtonKind::Quiet)
                .size(theme::ToolButtonSize::Small)
        };
        let previous_cut = cut_before(scenes, position);
        let next_cut = cut_after(scenes, position);
        let play = theme::ToolButton::new(play_icon, play_label, theme::ButtonKind::Primary)
            .size(theme::ToolButtonSize::Large);
        let all_buttons = [
            (
                round(Icon::SkipBack, PREVIOUS_CUT_LABEL),
                PREVIOUS_CUT_LABEL,
                previous_cut.is_some(),
                TransportAction::PreviousCut,
            ),
            (
                round(Icon::StepBack, ONE_PICTURE_BACK_HINT),
                ONE_PICTURE_BACK_HINT,
                true,
                TransportAction::PictureBack,
            ),
            (play, PLAY_HINT, true, TransportAction::PlayOrPause),
            (
                round(Icon::StepForward, ONE_PICTURE_FORWARD_HINT),
                ONE_PICTURE_FORWARD_HINT,
                true,
                TransportAction::PictureForward,
            ),
            (
                round(Icon::SkipForward, NEXT_CUT_LABEL),
                NEXT_CUT_LABEL,
                next_cut.is_some(),
                TransportAction::NextCut,
            ),
        ];
        // Short of room, the steps to the cuts go first: the timeline shows
        // the cuts, and the steps of one picture have no other place.
        let width_of =
            |widths: &[f32]| widths.iter().sum::<f32>() + TRANSPORT_GAP * (widths.len() - 1) as f32;
        let mut buttons: Vec<_> = all_buttons.into_iter().collect();
        let is_cut_step = |action: &TransportAction| {
            matches!(
                action,
                TransportAction::PreviousCut | TransportAction::NextCut
            )
        };
        let all_widths: Vec<f32> = buttons.iter().map(|(button, ..)| button.width()).collect();
        let widths_without_cut_steps: Vec<f32> = buttons
            .iter()
            .filter(|(.., action)| !is_cut_step(action))
            .map(|(button, ..)| button.width())
            .collect();
        let widest_row = ui.available_width();
        let shape = compact_row(
            widest_row,
            CompactRowWidths {
                all_buttons: width_of(&all_widths),
                without_cut_steps: width_of(&widths_without_cut_steps),
                whole_time: widest(whole_time(self.video_end)),
                short_time: widest(short_time(self.video_end)),
            },
        );
        if shape.cut_steps == CutSteps::LeftOut {
            buttons.retain(|(.., action)| !is_cut_step(action));
        }
        let buttons_width = match shape.cut_steps {
            CutSteps::Shown => width_of(&all_widths),
            CutSteps::LeftOut => width_of(&widths_without_cut_steps),
        };
        let time = match shape.time {
            TimeShown::Whole => Some(time_in_words(whole_time(position))),
            TimeShown::Short => Some(time_in_words(short_time(position))),
            TimeShown::Hidden => None,
        };
        let side_room = shape.side_room;
        // As wide as the picture, or a little wider when the time would not
        // fit beside the buttons under a small picture.
        let width = (buttons_width + 2.0 * side_room)
            .max(self.picture_size.x)
            .max(TRANSPORT_LEAST_WIDTH)
            .min(widest_row);
        let (row, _) =
            ui.allocate_exact_size(egui::vec2(width, TRANSPORT_HEIGHT), egui::Sense::hover());
        let mut left = row.center().x - buttons_width / 2.0;
        let mut pressed = None;
        for (button, hint, enabled, action) in buttons {
            let side = button.width();
            let place = egui::Rect::from_center_size(
                egui::pos2(left + side / 2.0, row.center().y),
                egui::vec2(side, side),
            );
            left += side + TRANSPORT_GAP;
            let clicked = ui
                .add_enabled_ui(enabled, |ui| {
                    ui.put(place, button).on_hover_text(hint).clicked()
                })
                .inner;
            if clicked {
                pressed = Some(action);
            }
        }
        match pressed {
            Some(TransportAction::PreviousCut) => {
                if let Some(cut) = previous_cut {
                    self.seek(cut, ui.ctx());
                }
            }
            Some(TransportAction::PictureBack) => self.step_frames(-1, ui.ctx()),
            Some(TransportAction::PlayOrPause) => self.toggle_pause(ui.ctx()),
            Some(TransportAction::PictureForward) => self.step_frames(1, ui.ctx()),
            Some(TransportAction::NextCut) => {
                if let Some(cut) = next_cut {
                    self.seek(cut, ui.ctx());
                }
            }
            None => {}
        }

        // Where the video is, on the left, when it fits beside the buttons.
        if let Some(time) = time {
            let at = egui::pos2(row.left(), row.center().y - time.size().y / 2.0);
            ui.painter().galley(at, time, palette.text);
        }

        let muted = self.sound == Sound::Muted;
        let (sound_icon, sound_label) = if muted {
            (Icon::SoundOff, SOUND_ON_LABEL)
        } else {
            (Icon::SoundOn, MUTE_LABEL)
        };
        let sound = round(sound_icon, sound_label);
        let side = sound.width();
        let place = egui::Rect::from_center_size(
            egui::pos2(row.right() - side / 2.0, row.center().y),
            egui::vec2(side, side),
        );
        if ui.put(place, sound).on_hover_text(sound_label).clicked() {
            if muted {
                self.asking_to_turn_sound_on = true;
            } else {
                self.set_sound(Sound::Muted, ui.ctx());
            }
        }
    }

    fn show_controls(
        &mut self,
        ui: &mut egui::Ui,
        scenes: &[TimeSpan],
        scene: Option<SceneIndex>,
        split_here: SplitHere,
    ) {
        if self.place == PlayerPlace::Embedded {
            self.show_compact_controls(ui, scenes);
            return;
        }
        let palette = theme::palette(ui.ctx());
        ui.set_max_width(self.picture_size.x);
        let paused = !matches!(self.state, PlayState::Playing(_));
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(format!("-{SKIP_SECONDS} s"))
                .on_hover_text("Left arrow or A")
                .clicked()
            {
                self.skip(-SKIP_SECONDS, ui.ctx());
            }
            let pause_label = if paused { "Play" } else { "Pause" };
            if ui.button(pause_label).on_hover_text("Space").clicked() {
                self.toggle_pause(ui.ctx());
            }
            if ui
                .button(format!("+{SKIP_SECONDS} s"))
                .on_hover_text("Right arrow or D")
                .clicked()
            {
                self.skip(SKIP_SECONDS, ui.ctx());
            }

            // The two buttons are one pair. In the smallest window the row
            // ended between them, so the pair starts the next row when it
            // does not fit whole, and no line is drawn before it that would
            // take the room it just fits in.
            if ui.available_size_before_wrap().x < PICTURE_STEP_BUTTONS_WIDTH {
                ui.end_row();
            }
            // The same step the mouse wheel makes, for those who do not
            // think to turn it.
            if ui
                .button(ONE_PICTURE_BACK_LABEL)
                .on_hover_text("One picture back. The mouse wheel over the picture does the same.")
                .clicked()
            {
                self.step_frames(-1, ui.ctx());
            }
            if ui
                .button(ONE_PICTURE_FORWARD_LABEL)
                .on_hover_text(
                    "One picture forward. The mouse wheel over the picture does the same.",
                )
                .clicked()
            {
                self.step_frames(1, ui.ctx());
            }

            // A line drawn at the end of a row divides nothing, so the
            // speeds start the next row without one.
            if ui.available_size_before_wrap().x < SPEED_CHOICES_WIDTH {
                ui.end_row();
            } else {
                ui.separator();
            }
            for speed in PlaybackSpeed::ALL {
                if ui
                    .selectable_label(self.speed == speed, speed.label())
                    .clicked()
                {
                    self.set_speed(speed, ui.ctx());
                }
            }

            ui.separator();
            let mut muted = self.sound == Sound::Muted;
            if ui.checkbox(&mut muted, "Mute").changed() {
                if muted {
                    self.set_sound(Sound::Muted, ui.ctx());
                } else {
                    // Turning sound on is confirmed first, the box stays
                    // ticked until the person agrees.
                    self.asking_to_turn_sound_on = true;
                }
            }
        });

        ui.horizontal_wrapped(|ui| {
            let split_button = ui
                .add(theme::IconButton::new(
                    Icon::Scissors,
                    split_here.button_label(),
                    theme::ButtonKind::Primary,
                ))
                .on_hover_text(theme::hint_with_key(
                    ui.ctx(),
                    split_here.button_hint(),
                    &keyboard::SPLIT_SHORTCUT,
                ));
            if split_button.clicked() {
                self.split_requested = true;
            }
            if let Some((scene, span)) =
                scene.and_then(|scene| Some((scene, *scenes.get(scene.position())?)))
            {
                theme::pill(
                    ui,
                    theme::semibold(scene_name(scene, scenes.len()), 14.0).color(palette.on_fill),
                    palette.scene_color(scene.position()),
                )
                .on_hover_text(scene_times(span));
            }
            // The place is exact, for finding a picture. The length is only
            // for orientation, and short enough that the row does not wrap.
            ui.label(format!(
                "{} / {}",
                format_timestamp(self.position()),
                format_clock(self.video_end)
            ));
        });
        if let Some(note) = &self.split_note {
            let color = theme::answer_color(palette, note.feedback.tone(), palette.text);
            let words = egui::RichText::new(note.feedback.in_words()).color(color);
            ui.add(egui::Label::new(words).wrap());
        }
        if let Some(problem) = &self.still_problem {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(palette.danger, STILL_PROBLEM_NOTE);
                theme::copy_details_button(ui, problem);
            });
        }
        // While playing the answer would change many times a second, so it
        // is shown once the person has stopped on a picture.
        if let (true, Some(split_point)) = (paused, split_here.would_land_at) {
            ui.add(
                egui::Label::new(theme::caption(
                    ui,
                    landing_description(self.position(), split_point),
                ))
                .wrap(),
            );
        }
        ui.add(egui::Label::new(theme::caption(ui, PLAYER_HINT)).wrap());
    }
}

/// The scene that `position` of the video is in. A scene owns its first
/// moment and not its last, so the moment of a cut belongs to the scene
/// that begins there. The very end of the video is still the last scene.
fn scene_at(scenes: &[TimeSpan], position: f64) -> Option<SceneIndex> {
    let scenes_begun = scenes
        .iter()
        .take_while(|scene| scene.start <= position)
        .count();
    match scenes_begun {
        // Before the first scene only by a rounding error, if at all.
        0 if scenes.is_empty() => None,
        0 => Some(SceneIndex::new(0)),
        begun => Some(SceneIndex::new(begun - 1)),
    }
}

/// How far along the video `time` is, from 0 at the start to 1 at the end.
fn share_of_video(time: f64, video_end: f64) -> f32 {
    // A video of a single frame has no length to take a share of.
    (time / video_end.max(f64::MIN_POSITIVE)).clamp(0.0, 1.0) as f32
}

fn scene_name(scene: SceneIndex, scene_count: usize) -> String {
    format!("Scene {} of {scene_count}", scene.number())
}

fn scene_times(scene: TimeSpan) -> String {
    format!(
        "This scene runs from {} to {}.",
        format_clock(scene.start),
        format_clock(scene.end)
    )
}

/// Writes the scene that has just begun over the top of the picture, large
/// enough to be seen while watching the video and not the controls.
fn show_scene_notice(ui: &egui::Ui, picture: egui::Rect, scene: SceneIndex) {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter();
    let text = painter.layout_no_wrap(
        format!("Scene {}", scene.number()),
        egui::FontId::proportional(22.0),
        palette.on_fill,
    );
    let padding = egui::vec2(14.0, 6.0);
    let label = egui::Rect::from_min_size(
        picture.left_top() + egui::vec2(SCENE_NOTICE_MARGIN, SCENE_NOTICE_MARGIN),
        text.size() + padding * 2.0,
    );
    painter.rect_filled(label, 8.0, palette.scene_color(scene.position()));
    painter.rect_stroke(label, 8.0, egui::Stroke::new(2.0_f32, palette.on_fill));
    painter.galley(label.min + padding, text, palette.on_fill);
}

/// What a split asked for here did, and, on the embedded picture, until
/// when that is said: a few seconds from the first frame it was drawn on.
#[derive(Debug, PartialEq)]
struct SplitNote {
    feedback: SplitFeedback,
    shown_until: Option<f64>,
}

/// Where the player is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PlayerPlace {
    /// A window of its own, in front of the review screen.
    Window,
    /// Part of the review screen, which it fills.
    Embedded,
}

/// The cut before `position`, leaving out one it is at.
fn cut_before(scenes: &[TimeSpan], position: f64) -> Option<f64> {
    scenes
        .iter()
        .skip(1)
        .map(|scene| scene.start)
        .rev()
        .find(|&cut| cut < position - CUT_NEARBY_SECONDS)
}

/// The cut after `position`, leaving out one it is at.
fn cut_after(scenes: &[TimeSpan], position: f64) -> Option<f64> {
    scenes
        .iter()
        .skip(1)
        .map(|scene| scene.start)
        .find(|&cut| cut > position + CUT_NEARBY_SECONDS)
}

/// The largest picture that leaves room for the controls under it in
/// `room`, kept to the picture's shape.
fn picture_size_within(room: egui::Vec2) -> egui::Vec2 {
    let widest_that_fits_down = (room.y - EMBEDDED_CONTROLS_HEIGHT) / PICTURE_ASPECT;
    let width = room
        .x
        .min(widest_that_fits_down)
        .max(MIN_EMBEDDED_PICTURE_WIDTH)
        .floor();
    egui::vec2(width, (width * PICTURE_ASPECT).round())
}

/// How large to draw the picture so that the whole player fits a window of
/// `screen` size.
fn picture_size_for(screen: egui::Vec2) -> egui::Vec2 {
    let widest_that_fits_across = screen.x - PLAYER_CHROME_WIDTH;
    let widest_that_fits_down = (screen.y - PLAYER_CHROME_HEIGHT) / PICTURE_ASPECT;
    let width = MAX_PICTURE_WIDTH
        .min(widest_that_fits_across)
        .min(widest_that_fits_down)
        .max(MIN_PICTURE_WIDTH)
        .floor();
    egui::vec2(width, (width * PICTURE_ASPECT).round())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cuts_either_side_leave_out_the_one_the_video_is_at() {
        let scenes = [
            TimeSpan {
                start: 0.0,
                end: 10.0,
            },
            TimeSpan {
                start: 10.0,
                end: 20.0,
            },
            TimeSpan {
                start: 20.0,
                end: 30.0,
            },
        ];
        assert_eq!(cut_before(&scenes, 15.0), Some(10.0));
        assert_eq!(cut_after(&scenes, 15.0), Some(20.0));
        assert_eq!(cut_before(&scenes, 10.0), None);
        assert_eq!(cut_after(&scenes, 10.0), Some(20.0));
        assert_eq!(cut_after(&scenes, 25.0), None);
    }
    use crate::test_videos::TestVideo;

    /// The picture as it is decoded for a screen of ordinary sharpness.
    const ORDINARY_FRAME: PictureSize = PictureSize {
        width: 640,
        height: 360,
    };

    #[test]
    fn on_an_ordinary_screen_the_picture_is_decoded_as_large_as_it_is_drawn() {
        assert_eq!(frame_size_for(1.0), ORDINARY_FRAME);
    }

    #[test]
    fn on_a_sharp_screen_the_picture_is_decoded_larger() {
        assert_eq!(
            frame_size_for(1.5),
            PictureSize {
                width: 960,
                height: 540
            }
        );
        assert_eq!(
            frame_size_for(2.0),
            PictureSize {
                width: 1280,
                height: 720
            }
        );
    }

    #[test]
    fn the_picture_is_never_decoded_larger_than_the_widest_frame() {
        assert_eq!(frame_size_for(3.0).width, *FRAME_WIDTHS.end());
    }

    /// A player on a video that does not exist, paused, with no decoder
    /// started, driven with simulated keys.
    struct PausedPlayer {
        ctx: egui::Context,
        player: Player,
        keyboard: KeyboardOwner,
    }

    /// What the split button does in a test that is not about it.
    const SPLIT_NEAR_NOWHERE: SplitHere = SplitHere {
        precision: CutPrecision::NearestKeyframe,
        would_land_at: None,
    };

    #[test]
    fn the_split_button_says_whether_the_split_lands_on_the_picture_or_near_it() {
        let exactly = SplitHere {
            precision: CutPrecision::ExactFrame,
            would_land_at: Some(10.0),
        };
        assert_eq!(SPLIT_NEAR_NOWHERE.button_label(), "Split near here");
        assert!(SPLIT_NEAR_NOWHERE.button_hint().contains("closest point"));
        assert_eq!(exactly.button_label(), "Split here");
        assert!(exactly.button_hint().contains("on this picture"));
    }

    impl PausedPlayer {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let video: Arc<Path> = Arc::from(Path::new("no_such_video.mp4"));
            let player = Player::paused_at(video, 100.0, 0.04, 10.0, &ctx);
            Self {
                ctx,
                player,
                keyboard: KeyboardOwner::Screens,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> PlayerEvent {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut event = PlayerEvent::StillOpen;
            let _ = self.ctx.run(input, |ctx| {
                self.player.take_keys(ctx, self.keyboard);
                event = self.player.show(ctx, &three_scenes(), SPLIT_NEAR_NOWHERE);
            });
            event
        }

        fn press(&mut self, key: egui::Key) -> PlayerEvent {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }])
        }
    }

    #[test]
    fn escape_cancels_the_sound_question_and_leaves_the_player_open() {
        let mut paused = PausedPlayer::new();
        paused.player.asking_to_turn_sound_on = true;

        assert_eq!(paused.press(egui::Key::Escape), PlayerEvent::StillOpen);
        assert!(!paused.player.asking_to_turn_sound_on);
        assert_eq!(paused.player.sound, Sound::Muted);

        // With no question open, Escape closes the player as before.
        assert_eq!(paused.press(egui::Key::Escape), PlayerEvent::Closed);
    }

    #[test]
    fn the_player_keys_wait_while_the_sound_question_is_open() {
        let mut paused = PausedPlayer::new();
        paused.player.asking_to_turn_sound_on = true;

        assert_eq!(paused.press(egui::Key::S), PlayerEvent::StillOpen);
        assert!(paused.player.asking_to_turn_sound_on);

        paused.player.asking_to_turn_sound_on = false;
        assert_eq!(
            paused.press(egui::Key::S),
            PlayerEvent::SplitRequested(10.0)
        );
    }

    #[test]
    fn the_keys_the_player_acts_on_are_not_left_for_the_review_behind_it() {
        let mut paused = PausedPlayer::new();
        let mut left_for_the_review_screen = true;
        let _ = paused.ctx.run(
            key_press(egui::Key::ArrowRight, egui::Modifiers::NONE),
            |ctx| {
                paused.player.take_keys(ctx, KeyboardOwner::Screens);
                left_for_the_review_screen =
                    ctx.input(|input| input.key_pressed(egui::Key::ArrowRight));
            },
        );
        assert!(!left_for_the_review_screen);
        assert_eq!(paused.player.position(), 10.0 + SKIP_SECONDS);
    }

    #[test]
    fn s_splits_in_the_player_and_is_not_left_to_move_through_the_review() {
        let mut paused = PausedPlayer::new();
        let mut left_for_the_review_screen = true;
        let _ = paused
            .ctx
            .run(key_press(egui::Key::S, egui::Modifiers::NONE), |ctx| {
                paused.player.take_keys(ctx, KeyboardOwner::Screens);
                left_for_the_review_screen = ctx.input(|input| input.key_pressed(egui::Key::S));
            });
        assert!(!left_for_the_review_screen);
        assert!(paused.player.split_requested);
    }

    #[test]
    fn an_arrow_key_with_alt_held_is_the_reviews_and_does_not_skip() {
        let mut paused = PausedPlayer::new();
        let mut left_for_the_review_screen = false;
        let _ = paused.ctx.run(
            key_press(egui::Key::ArrowRight, egui::Modifiers::ALT),
            |ctx| {
                paused.player.take_keys(ctx, KeyboardOwner::Screens);
                left_for_the_review_screen =
                    ctx.input(|input| input.key_pressed(egui::Key::ArrowRight));
            },
        );
        assert!(left_for_the_review_screen);
        assert_eq!(paused.player.position(), 10.0);
    }

    fn key_press(key: egui::Key, modifiers: egui::Modifiers) -> egui::RawInput {
        egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            modifiers,
            ..Default::default()
        }
    }

    #[test]
    fn the_player_leaves_every_key_alone_behind_a_dialog() {
        let mut paused = PausedPlayer::new();
        paused.keyboard = KeyboardOwner::Dialog;

        assert_eq!(paused.press(egui::Key::S), PlayerEvent::StillOpen);
        assert_eq!(paused.press(egui::Key::Escape), PlayerEvent::StillOpen);
        assert_eq!(paused.press(egui::Key::ArrowRight), PlayerEvent::StillOpen);
        assert_eq!(paused.player.position(), 10.0);
    }

    #[test]
    fn the_escape_that_closes_the_player_is_not_left_for_what_is_behind_it() {
        let mut paused = PausedPlayer::new();
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let input = egui::RawInput {
            events: vec![escape],
            ..Default::default()
        };
        let mut left_for_the_review_screen = true;
        let _ = paused.ctx.run(input, |ctx| {
            paused.player.take_keys(ctx, KeyboardOwner::Screens);
            left_for_the_review_screen = keyboard::take_escape(ctx);
        });

        assert!(paused.player.close_requested);
        assert!(!left_for_the_review_screen);
    }

    #[test]
    fn a_picture_that_cannot_be_shown_is_said_until_one_can() {
        let mut paused = PausedPlayer::new();
        paused.player.still_request = 1;
        paused
            .player
            .stills_sender
            .send(Still {
                request: 1,
                size: ORDINARY_FRAME,
                picture: Err(anyhow::anyhow!("the decoder gave no picture")),
            })
            .unwrap();
        paused.frame(Vec::new());
        assert_eq!(
            paused.player.still_problem.as_deref(),
            Some("the decoder gave no picture")
        );

        let black_picture = vec![0u8; ORDINARY_FRAME.rgb_bytes()];
        paused
            .player
            .stills_sender
            .send(Still {
                request: 1,
                size: ORDINARY_FRAME,
                picture: Ok(black_picture),
            })
            .unwrap();
        paused.frame(Vec::new());
        assert_eq!(paused.player.still_problem, None);
    }

    #[test]
    fn the_outcome_of_a_split_is_kept_to_be_shown() {
        let mut paused = PausedPlayer::new();
        assert_eq!(paused.player.split_note, None);

        paused
            .player
            .show_split_feedback(SplitFeedback::Added(12.0));
        paused.frame(Vec::new());
        assert_eq!(
            paused.player.split_note.as_ref().map(|note| &note.feedback),
            Some(&SplitFeedback::Added(12.0))
        );
    }

    #[test]
    fn the_note_about_a_split_leaves_the_embedded_picture_after_a_while() {
        let mut paused = PausedPlayer::new();
        paused
            .player
            .show_split_feedback(SplitFeedback::Added(12.0));
        assert!(paused.player.expire_split_note(100.0).is_some());
        assert!(paused
            .player
            .expire_split_note(100.0 + NOTE_ON_PICTURE_SECONDS - 0.1)
            .is_some());
        assert_eq!(
            paused
                .player
                .expire_split_note(100.0 + NOTE_ON_PICTURE_SECONDS),
            None
        );
        assert_eq!(paused.player.split_note, None);
    }

    #[test]
    fn a_drag_held_still_grabs_no_new_picture() {
        let mut paused = PausedPlayer::new();
        let ctx = paused.ctx.clone();
        paused.player.act_on(PlayerAsk::ScrubTo(42.0), &ctx);
        let grabs = paused.player.still_request;
        paused.player.act_on(PlayerAsk::ScrubTo(42.0), &ctx);
        assert_eq!(paused.player.still_request, grabs);
        assert_eq!(paused.player.playhead(), 42.0);

        paused.player.act_on(PlayerAsk::ScrubEnded, &ctx);
        assert_eq!(paused.player.scrubbing_to, None);
        assert_eq!(paused.player.position(), 42.0);
    }

    /// The row's widths in the smallest window: five buttons, three without
    /// the steps to the cuts, and the time measured as it is drawn.
    const ROW_WIDTHS: CompactRowWidths = CompactRowWidths {
        all_buttons: 208.0,
        without_cut_steps: 128.0,
        whole_time: 95.0,
        short_time: 50.0,
    };

    #[test]
    fn a_roomy_row_has_every_button_and_the_whole_time() {
        let row = compact_row(420.0, ROW_WIDTHS);
        assert_eq!(row.time, TimeShown::Whole);
        assert_eq!(row.cut_steps, CutSteps::Shown);
        assert_eq!(row.side_room, 95.0 + TRANSPORT_GAP);
    }

    #[test]
    fn short_of_room_the_steps_to_the_cuts_go_before_the_time() {
        let row = compact_row(340.0, ROW_WIDTHS);
        assert_eq!(row.time, TimeShown::Whole);
        assert_eq!(row.cut_steps, CutSteps::LeftOut);
    }

    #[test]
    fn in_the_smallest_window_the_row_keeps_where_the_video_is() {
        let row = compact_row(290.0, ROW_WIDTHS);
        assert_eq!(row.time, TimeShown::Short);
        assert_eq!(row.cut_steps, CutSteps::LeftOut);
    }

    #[test]
    fn with_no_room_for_any_time_the_row_keeps_room_for_sound() {
        let row = compact_row(200.0, ROW_WIDTHS);
        assert_eq!(row.time, TimeShown::Hidden);
        assert_eq!(row.side_room, theme::CONTROL_HEIGHT + TRANSPORT_GAP);
    }

    #[test]
    fn the_time_is_measured_with_its_digits_as_zeros() {
        assert_eq!(widest_digits("1:23.45 / 8:53"), "0:00.00 / 0:00");
    }

    #[test]
    fn the_sound_question_is_drawn_in_front_of_the_player() {
        let mut paused = PausedPlayer::new();
        paused.player.asking_to_turn_sound_on = true;
        // Areas take their place in the first frame they are drawn.
        paused.frame(Vec::new());
        paused.frame(Vec::new());

        let ctx = &paused.ctx;
        let question =
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("Turn the sound on?"));
        let backdrop = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("sound_question_backdrop"),
        );
        let topmost = ctx.memory(|memory| memory.areas().top_layer_id(egui::Order::Foreground));
        assert_eq!(topmost, Some(question));
        // The dimming covers the whole window, so it takes every click that
        // misses the question: in an empty corner, and on the player, which
        // sits in the bottom right corner.
        assert_eq!(ctx.layer_id_at(egui::pos2(2.0, 2.0)), Some(backdrop));
        let on_the_player = egui::pos2(
            900.0 - 2.0 * PLAYER_CORNER_GAP,
            600.0 - 2.0 * PLAYER_CORNER_GAP,
        );
        assert_eq!(ctx.layer_id_at(on_the_player), Some(backdrop));
    }

    #[test]
    fn position_advances_one_second_per_thirty_frames_at_normal_speed() {
        assert_eq!(position_after(100.0, 30, PlaybackSpeed::Normal), 101.0);
    }

    #[test]
    fn faster_playback_covers_more_video_per_frame() {
        assert_eq!(position_after(100.0, 30, PlaybackSpeed::Double), 102.0);
        assert_eq!(position_after(100.0, 30, PlaybackSpeed::Quadruple), 104.0);
    }

    #[test]
    fn sound_is_sped_up_to_match_the_pictures() {
        assert_eq!(PlaybackSpeed::Normal.audio_filter(), "anull");
        assert_eq!(PlaybackSpeed::Double.audio_filter(), "atempo=2.0");
        assert_eq!(
            PlaybackSpeed::Quadruple.audio_filter(),
            "atempo=2.0,atempo=2.0"
        );
    }

    /// Plays a generated video with the sound off. The clock starts at the
    /// first picture, so a machine that is slow to start ffmpeg does not
    /// fail the test.
    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_real_video_plays_at_double_speed() {
        const START: f64 = 1.0;
        const WATCHED: Duration = Duration::from_secs(1);
        const FIRST_PICTURE_DEADLINE: Duration = Duration::from_secs(30);
        const POLL_INTERVAL: Duration = Duration::from_millis(20);

        let video = TestVideo::two_scenes_with_a_black_screen("playback");
        let playback = Playback::start(
            &video.path,
            START,
            PlaybackSpeed::Double,
            Sound::Muted,
            ORDINARY_FRAME,
            &egui::Context::default(),
        )
        .unwrap();

        let waiting_since = Instant::now();
        while playback.state.frames_shown.load(Ordering::Relaxed) == 0 {
            assert!(
                waiting_since.elapsed() < FIRST_PICTURE_DEADLINE,
                "no picture arrived"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
        let position_at_first_picture = playback.position();
        std::thread::sleep(WATCHED);

        // One second of watching at double speed covers two seconds of
        // video. The bounds are wide because sleeping is not exact.
        let covered = playback.position() - position_at_first_picture;
        assert!((1.0..=3.0).contains(&covered), "covered {covered} seconds");

        let newest_frame = playback.state.newest_frame.lock().unwrap().take();
        assert_eq!(
            newest_frame.map(|frame| frame.len()),
            Some(ORDINARY_FRAME.rgb_bytes())
        );
    }

    fn three_scenes() -> Vec<TimeSpan> {
        [(0.0, 160.0), (160.0, 288.0), (288.0, 533.0)]
            .into_iter()
            .map(|(start, end)| TimeSpan { start, end })
            .collect()
    }

    #[test]
    fn a_position_is_in_the_scene_that_holds_it() {
        let scenes = three_scenes();
        assert_eq!(scene_at(&scenes, 0.0), Some(SceneIndex::new(0)));
        assert_eq!(scene_at(&scenes, 159.9), Some(SceneIndex::new(0)));
        assert_eq!(scene_at(&scenes, 200.0), Some(SceneIndex::new(1)));
        assert_eq!(scene_at(&scenes, 400.0), Some(SceneIndex::new(2)));
    }

    #[test]
    fn the_moment_of_a_cut_belongs_to_the_scene_that_begins_there() {
        assert_eq!(scene_at(&three_scenes(), 160.0), Some(SceneIndex::new(1)));
    }

    #[test]
    fn the_end_of_the_video_is_still_the_last_scene() {
        let scenes = three_scenes();
        assert_eq!(scene_at(&scenes, 533.0), Some(SceneIndex::new(2)));
        assert_eq!(scene_at(&scenes, 540.0), Some(SceneIndex::new(2)));
    }

    #[test]
    fn a_position_before_the_first_scene_counts_as_the_first() {
        let scenes = [TimeSpan {
            start: 0.04,
            end: 10.0,
        }];
        assert_eq!(scene_at(&scenes, 0.0), Some(SceneIndex::new(0)));
    }

    #[test]
    fn a_video_with_no_scenes_has_no_scene_on_screen() {
        assert_eq!(scene_at(&[], 5.0), None);
    }

    #[test]
    fn the_share_of_the_video_stays_between_nothing_and_all() {
        assert_eq!(share_of_video(50.0, 200.0), 0.25);
        assert_eq!(share_of_video(-1.0, 200.0), 0.0);
        assert_eq!(share_of_video(300.0, 200.0), 1.0);
        assert_eq!(share_of_video(0.0, 0.0), 0.0);
    }

    #[test]
    fn the_scene_is_named_with_how_many_there_are() {
        assert_eq!(scene_name(SceneIndex::new(1), 4), "Scene 2 of 4");
    }

    #[test]
    fn the_picture_is_full_size_in_a_large_window() {
        let size = picture_size_for(egui::vec2(1320.0, 900.0));
        assert_eq!(size, egui::vec2(640.0, 360.0));
    }

    #[test]
    fn the_picture_shrinks_so_the_player_fits_a_small_window() {
        let window = egui::vec2(800.0, 560.0);
        let size = picture_size_for(window);
        assert!(size.x < MAX_PICTURE_WIDTH);
        assert!(size.y + PLAYER_CHROME_HEIGHT <= window.y + 1.0, "{size:?}");
        assert!(size.x + PLAYER_CHROME_WIDTH <= window.x);
    }

    #[test]
    fn the_picture_never_gets_too_small_to_see() {
        let size = picture_size_for(egui::vec2(200.0, 200.0));
        assert_eq!(size.x, MIN_PICTURE_WIDTH);
    }

    #[test]
    fn one_wheel_notch_is_one_frame_and_down_is_forward() {
        let mut left_over = 0.0;
        let line = egui::MouseWheelUnit::Line;
        assert_eq!(frames_for_wheel_turn(line, -1.0, &mut left_over), 1);
        assert_eq!(frames_for_wheel_turn(line, 1.0, &mut left_over), -1);
        assert_eq!(frames_for_wheel_turn(line, 0.0, &mut left_over), 0);
    }

    #[test]
    fn touchpad_scrolling_adds_up_to_whole_frames() {
        let mut left_over = 0.0;
        let point = egui::MouseWheelUnit::Point;
        assert_eq!(frames_for_wheel_turn(point, -30.0, &mut left_over), 0);
        assert_eq!(frames_for_wheel_turn(point, -30.0, &mut left_over), 1);
        assert_eq!(left_over, -10.0);
        assert_eq!(frames_for_wheel_turn(point, 110.0, &mut left_over), -2);
    }

    #[test]
    fn the_landing_note_says_how_far_the_keyframe_is() {
        let later = landing_description(601.63, 602.0);
        assert!(later.contains("0.37 s after this picture"), "{later}");
        assert!(later.contains("00:10:02.000"), "{later}");

        let earlier = landing_description(602.5, 602.0);
        assert!(earlier.contains("0.50 s before this picture"), "{earlier}");

        assert_eq!(
            landing_description(602.0, 602.0),
            "A split would be exactly at this picture."
        );
    }

    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_real_frame_can_be_grabbed_for_stepping() {
        let video = TestVideo::two_scenes_with_a_black_screen("still");
        let one_frame_later = 1.0 + crate::test_videos::FRAME_SECONDS;

        let first = grab_still(&video.path, 1.0, ORDINARY_FRAME).unwrap();
        let next = grab_still(&video.path, one_frame_later, ORDINARY_FRAME).unwrap();
        assert_eq!(first.len(), ORDINARY_FRAME.rgb_bytes());
        assert_ne!(first, next, "stepping one frame should change the picture");
    }

    #[test]
    fn a_tenth_of_a_second_of_stereo_sound_is_one_chunk() {
        assert_eq!(AUDIO_CHUNK_BYTES, 19_200);
    }
}
