//! A small video player for checking where scenes really begin and end.
//!
//! Playback is two ffmpeg processes, one decoding pictures and one decoding
//! sound, both started at the wanted position and speed. Seeking, pausing
//! and changing speed all work the same way: stop both and start again from
//! the new position. That keeps the player simple, at the cost of a short
//! pause on every jump.

use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use eframe::egui;
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};

use crate::timecode::format_timestamp;
use crate::tools;

const FRAME_WIDTH: usize = 640;
const FRAME_HEIGHT: usize = 360;
const FRAME_BYTES: usize = FRAME_WIDTH * FRAME_HEIGHT * 3;
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
/// On a touchpad, which scrolls smoothly, this much scrolling steps one frame.
const SCROLL_POINTS_PER_FRAME: f32 = 50.0;
/// A split lands "exactly here" when the keyframe is closer than this.
const SAME_MOMENT_SECONDS: f64 = 0.0005;

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
        ctx: &egui::Context,
    ) -> Result<Self> {
        let state = Arc::new(PlaybackState::default());

        let mut picture_process = spawn_picture_decoder(video, position, speed)?;
        let frames = picture_process
            .stdout
            .take()
            .context("ffmpeg has no picture pipe")?;
        let picture_state = Arc::clone(&state);
        let ctx = ctx.clone();
        std::thread::spawn(move || show_frames_on_time(frames, &picture_state, &ctx));

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

fn spawn_picture_decoder(video: &Path, position: f64, speed: PlaybackSpeed) -> Result<Child> {
    let filter = format!(
        "setpts=PTS/{},fps={FRAMES_PER_SECOND},\
         scale={FRAME_WIDTH}:{FRAME_HEIGHT}:force_original_aspect_ratio=decrease,\
         pad={FRAME_WIDTH}:{FRAME_HEIGHT}:(ow-iw)/2:(oh-ih)/2",
        speed.factor()
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
fn show_frames_on_time(mut frames: ChildStdout, state: &PlaybackState, ctx: &egui::Context) {
    let mut frame = vec![0u8; FRAME_BYTES];
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

/// Decodes the one picture shown at `position`, at the player's size.
fn grab_still(video: &Path, position: f64) -> Result<Vec<u8>> {
    let filter = format!(
        "scale={FRAME_WIDTH}:{FRAME_HEIGHT}:force_original_aspect_ratio=decrease,\
         pad={FRAME_WIDTH}:{FRAME_HEIGHT}:(ow-iw)/2:(oh-ih)/2"
    );
    let output = tools::ffmpeg_command()
        .args(["-nostdin", "-v", "error"])
        .args(["-ss", &format!("{position:.6}")])
        .arg("-i")
        .arg(video)
        .args(["-map", "0:v:0", "-frames:v", "1", "-vf", &filter])
        .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .output()
        .context("could not start ffmpeg, is it on PATH?")?;
    if !output.status.success() || output.stdout.len() != FRAME_BYTES {
        bail!(
            "ffmpeg returned {} bytes instead of a picture at {position:.3}s: {}",
            output.stdout.len(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
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
/// that is from the frame they picked.
fn landing_description(position: f64, keyframe: f64) -> String {
    let gap = keyframe - position;
    let distance = if gap.abs() < SAME_MOMENT_SECONDS {
        "exactly here".to_owned()
    } else if gap > 0.0 {
        format!("{gap:.2} s later")
    } else {
        format!("{:.2} s earlier", -gap)
    };
    format!(
        "A split lands on the keyframe at {}, {distance}.",
        format_timestamp(keyframe)
    )
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

pub struct Player {
    video: Arc<Path>,
    video_length: f64,
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
    /// True while the "turn the sound on?" question is on screen.
    asking_to_turn_sound_on: bool,
    /// What the last split did, as told by the review screen.
    split_feedback: String,
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
}

/// One paused picture, grabbed after stepping to a frame.
struct Still {
    request: u64,
    picture: Result<Vec<u8>>,
}

impl Player {
    pub fn play_from(
        video: Arc<Path>,
        video_length: f64,
        frame_seconds: f64,
        position: f64,
        ctx: &egui::Context,
    ) -> Self {
        let (stills_sender, stills) = mpsc::channel();
        let mut player = Self {
            video,
            video_length,
            speed: PlaybackSpeed::Normal,
            // Sound starts off, so opening the player never blares.
            sound: Sound::Muted,
            state: PlayState::Paused { position },
            picture: None,
            scrubbing_to: None,
            split_requested: false,
            close_requested: false,
            asking_to_turn_sound_on: false,
            split_feedback: String::new(),
            frame_seconds,
            stills_sender,
            stills,
            still_request: 0,
            still_being_grabbed: false,
            still_wanted_next: None,
            scroll_points_left_over: 0.0,
        };
        player.play_at(position, ctx);
        player
    }

    pub fn position(&self) -> f64 {
        let position = match &self.state {
            PlayState::Playing(playback) => playback.position(),
            PlayState::Paused { position } => *position,
            PlayState::Failed(_) => 0.0,
        };
        position.clamp(0.0, self.video_length)
    }

    /// Jumps to `position`, playing from there. Also used to restart
    /// playback after a speed change.
    pub fn play_at(&mut self, position: f64, ctx: &egui::Context) {
        let position = position.clamp(0.0, self.video_length);
        // A still that is on its way must not replace the moving picture.
        self.still_request += 1;
        self.still_wanted_next = None;
        // The old decoders are stopped before new ones start, so two sound
        // tracks never play over each other.
        self.state = PlayState::Paused { position };
        self.state = match Playback::start(&self.video, position, self.speed, self.sound, ctx) {
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
        let position = position.clamp(0.0, self.video_length);
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
        let video = Arc::clone(&self.video);
        let stills_sender = self.stills_sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picture = grab_still(&video, position);
            // The send only fails once the player is closed.
            if stills_sender.send(Still { request, picture }).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn pause(&mut self) {
        self.state = PlayState::Paused {
            position: self.position(),
        };
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

    fn ask_before_turning_sound_on(&mut self, ctx: &egui::Context) {
        if !self.asking_to_turn_sound_on {
            return;
        }
        egui::Window::new("Turn the sound on?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
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

    /// Draws the player window and handles its buttons and keys.
    ///
    /// `split_would_land_at` is the keyframe a split at the current position
    /// would be cut on, if a split is possible there.
    pub fn show(&mut self, ctx: &egui::Context, split_would_land_at: Option<f64>) -> PlayerEvent {
        self.take_newest_frame(ctx);
        self.take_newest_still(ctx);
        self.pause_at_the_end();
        self.handle_keys(ctx);

        let mut open = true;
        egui::Window::new("Player")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let pointer_over_picture = self.show_picture(ui);
                if pointer_over_picture {
                    self.step_frames_with_the_wheel(ui.ctx());
                }
                self.show_position_slider(ui);
                self.show_controls(ui, split_would_land_at);
            });
        self.ask_before_turning_sound_on(ctx);
        if !open || self.close_requested {
            return PlayerEvent::Closed;
        }
        if std::mem::take(&mut self.split_requested) {
            return PlayerEvent::SplitRequested(self.position());
        }
        PlayerEvent::StillOpen
    }

    /// Shows the outcome of a split in the player window.
    pub fn show_split_feedback(&mut self, feedback: String) {
        self.split_feedback = feedback;
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
        if let Some(frame) = newest_frame {
            self.set_picture(ctx, &frame);
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
                Ok(frame) => self.set_picture(ctx, &frame),
                Err(error) => eprintln!("Could not show the frame: {error:#}"),
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

    fn set_picture(&mut self, ctx: &egui::Context, frame: &[u8]) {
        let image = egui::ColorImage::from_rgb([FRAME_WIDTH, FRAME_HEIGHT], frame);
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

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Keys typed into a text box belong to the text box.
        if ctx.wants_keyboard_input() {
            return;
        }
        let (back, forward, pause, split, escape) = ctx.input(|input| {
            // With Ctrl held, A belongs to "select all" on the review screen.
            let plain_key = |key| input.key_pressed(key) && !input.modifiers.command;
            (
                input.key_pressed(egui::Key::ArrowLeft) || plain_key(egui::Key::A),
                input.key_pressed(egui::Key::ArrowRight) || plain_key(egui::Key::D),
                input.key_pressed(egui::Key::Space),
                input.key_pressed(egui::Key::S),
                input.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            // Escape backs out of the innermost thing that is open.
            if self.asking_to_turn_sound_on {
                self.asking_to_turn_sound_on = false;
            } else {
                self.close_requested = true;
            }
        }
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

    /// Draws the picture and returns whether the pointer is over it.
    fn show_picture(&self, ui: &mut egui::Ui) -> bool {
        let size = egui::vec2(FRAME_WIDTH as f32, FRAME_HEIGHT as f32);
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
                ui.painter().rect_filled(area, 0.0, egui::Color32::BLACK);
                response
            }
        };
        response.contains_pointer()
    }

    fn show_position_slider(&mut self, ui: &mut egui::Ui) {
        let mut position = self.scrubbing_to.unwrap_or_else(|| self.position());
        ui.spacing_mut().slider_width = FRAME_WIDTH as f32;
        let slider =
            ui.add(egui::Slider::new(&mut position, 0.0..=self.video_length).show_value(false));
        if slider.dragged() {
            // Hold the handle where the person has it. Restarting playback
            // on every pixel of the drag would stutter.
            self.scrubbing_to = Some(position);
        } else if slider.drag_stopped() || slider.changed() {
            self.scrubbing_to = None;
            self.seek(position, ui.ctx());
        }
    }

    fn show_controls(&mut self, ui: &mut egui::Ui, split_would_land_at: Option<f64>) {
        ui.horizontal(|ui| {
            if ui.button(format!("-{SKIP_SECONDS} s")).clicked() {
                self.skip(-SKIP_SECONDS, ui.ctx());
            }
            let pause_label = match self.state {
                PlayState::Playing(_) => "Pause",
                PlayState::Paused { .. } | PlayState::Failed(_) => "Play",
            };
            if ui.button(pause_label).clicked() {
                self.toggle_pause(ui.ctx());
            }
            if ui.button(format!("+{SKIP_SECONDS} s")).clicked() {
                self.skip(SKIP_SECONDS, ui.ctx());
            }

            ui.separator();
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

            ui.separator();
            ui.label(format!(
                "{} / {}",
                format_timestamp(self.position()),
                format_timestamp(self.video_length)
            ));
        });
        ui.horizontal(|ui| {
            let split_button = ui.button("Split scene here (S)").on_hover_text(
                "Cuts the scene playing now in two, on the keyframe nearest to this moment.",
            );
            if split_button.clicked() {
                self.split_requested = true;
            }
            ui.label(&self.split_feedback);
        });
        if let Some(keyframe) = split_would_land_at {
            ui.label(landing_description(self.position(), keyframe));
        }
        ui.weak("Left or A goes back 10 seconds, Right or D goes forward, Space pauses, S splits, Esc closes.");
        ui.weak("Turn the mouse wheel over the picture to move one frame at a time.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_videos::TestVideo;

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

    /// Plays a generated video for two seconds with the sound off.
    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_real_video_plays_at_double_speed() {
        const START: f64 = 1.0;
        const WATCHED: Duration = Duration::from_secs(2);

        let video = TestVideo::two_scenes_with_a_black_screen("playback");
        let playback = Playback::start(
            &video.path,
            START,
            PlaybackSpeed::Double,
            Sound::Muted,
            &egui::Context::default(),
        )
        .unwrap();
        std::thread::sleep(WATCHED);

        // Two seconds of watching at double speed covers four seconds of
        // video, less the moment ffmpeg takes to start, which is long on a
        // busy machine.
        let covered = playback.position() - START;
        assert!((1.0..=4.1).contains(&covered), "covered {covered} seconds");

        let newest_frame = playback.state.newest_frame.lock().unwrap().take();
        assert_eq!(newest_frame.map(|frame| frame.len()), Some(FRAME_BYTES));
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
        assert_eq!(
            landing_description(601.63, 602.0),
            "A split lands on the keyframe at 00:10:02.000, 0.37 s later."
        );
        assert_eq!(
            landing_description(602.5, 602.0),
            "A split lands on the keyframe at 00:10:02.000, 0.50 s earlier."
        );
        assert_eq!(
            landing_description(602.0, 602.0),
            "A split lands on the keyframe at 00:10:02.000, exactly here."
        );
    }

    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_real_frame_can_be_grabbed_for_stepping() {
        let video = TestVideo::two_scenes_with_a_black_screen("still");
        let one_frame_later = 1.0 + crate::test_videos::FRAME_SECONDS;

        let first = grab_still(&video.path, 1.0).unwrap();
        let next = grab_still(&video.path, one_frame_later).unwrap();
        assert_eq!(first.len(), FRAME_BYTES);
        assert_ne!(first, next, "stepping one frame should change the picture");
    }

    #[test]
    fn a_tenth_of_a_second_of_stereo_sound_is_one_chunk() {
        assert_eq!(AUDIO_CHUNK_BYTES, 19_200);
    }
}
