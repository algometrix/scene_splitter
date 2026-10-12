//! The window: a queue of videos on the left, the selected video on the
//! right. Videos are scanned in the background, one after another, so the
//! next one is ready by the time the current one has been reviewed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use eframe::egui;

use crate::cache::{self, CacheUse, DetectionSource, SourcedDetection};
use crate::detect::{ScanDepth, ScanMode};
use crate::export::{self, ExportReport, OriginalPolicy, TRASH_NAME};
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::library;
use crate::player::{Player, PlayerEvent};
use crate::review::{counted, file_name, Review, ReviewRequest};
use crate::settings::{self, Settings};
use crate::split::{self, SceneLocation};
use crate::theme;
use crate::tools::{self, Availability, DownloadProgress};

const WINDOW_TITLE: &str = "Scene splitter";
const WINDOW_SIZE: [f32; 2] = [1320.0, 900.0];
const QUEUE_PANEL_WIDTH: f32 = 290.0;
const MESSAGE_CARD_WIDTH: f32 = 560.0;
const JOB_CARD_ROUNDING: f32 = 10.0;
const STATUS_DOT_SIZE: f32 = 8.0;
const DROP_PROMPT: &str = "Drop videos or a folder here";
const PERCENT: f64 = 100.0;
const BYTES_PER_MEGABYTE: u64 = 1024 * 1024;
const SECONDS_PER_MINUTE: f64 = 60.0;
/// The ranges the settings section offers.
const SHORTEST_BLACK_SECONDS: std::ops::RangeInclusive<f64> = 0.05..=10.0;
const BLACK_SHARE_PERCENT: std::ops::RangeInclusive<f64> = 50.0..=100.0;
const SHORTEST_SCENE_MINUTES: std::ops::RangeInclusive<f64> = 0.5..=60.0;
const DROP_EXPLANATION: &str =
    "Scene Splitter finds the black screens between scenes and cuts the video there. \
     You can also paste a path on the left.";

/// Opens the window and blocks until it is closed. With `first_path` that
/// video, or every video in that folder, is queued at once, to be scanned
/// as deeply as `first_scan` says.
pub fn run(settings: Settings, first_path: Option<PathBuf>, first_scan: ScanDepth) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |creation| {
            theme::apply(&creation.egui_ctx);
            let mut app = SceneSplitterApp::new(settings);
            app.get_tools_if_missing(&creation.egui_ctx);
            if let Some(path) = first_path {
                app.add_path(path);
                app.scan_queued_videos_as(first_scan);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow!("could not open the window: {error}"))
}

/// One video in the queue. Jobs are never removed, so a job's position in
/// the queue identifies it for as long as the window is open.
struct Job {
    video: Arc<Path>,
    stage: JobStage,
    /// Whether the next scan of this video may reuse a stored one.
    cache_use: CacheUse,
    scan_kind: ScanKind,
}

/// Which scan the person chose for a video.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ScanKind {
    /// The normal, fast scan.
    BlackScreens,
    /// The slow scan that also finds scene changes without a black screen.
    HardCuts(HardCutSettings),
}

impl ScanKind {
    fn depth(self) -> ScanDepth {
        match self {
            Self::BlackScreens => ScanDepth::BlackScreens,
            Self::HardCuts(_) => ScanDepth::WithHardCuts,
        }
    }
}

enum JobStage {
    Queued,
    Scanning,
    Reviewing(Box<Review>),
    Exporting,
    Finished(ExportReport),
    Skipped,
    Failed(String),
}

impl JobStage {
    fn status(&self) -> String {
        match self {
            Self::Queued => "Queued".to_owned(),
            Self::Scanning => "Scanning".to_owned(),
            Self::Reviewing(review) => format!("Ready, {}", counted(review.scene_count(), "scene")),
            Self::Exporting => "Writing and verifying".to_owned(),
            Self::Finished(report) if report.is_clean() => "Done, verified".to_owned(),
            Self::Finished(_) => "Done, needs a look".to_owned(),
            Self::Skipped => "Skipped".to_owned(),
            Self::Failed(_) => "Failed".to_owned(),
        }
    }

    fn status_color(&self) -> egui::Color32 {
        match self {
            Self::Queued | Self::Skipped => theme::MUTED_TEXT,
            Self::Scanning | Self::Exporting => theme::ACCENT,
            Self::Reviewing(_) => theme::PRIMARY,
            Self::Finished(report) if report.is_clean() => theme::SUCCESS,
            Self::Finished(_) | Self::Failed(_) => theme::DANGER,
        }
    }

    /// Whether the person still has something to do or wait for here.
    fn is_open(&self) -> bool {
        matches!(self, Self::Queued | Self::Scanning | Self::Reviewing(_))
    }
}

/// The result of background work on one job.
struct JobUpdate {
    job: usize,
    event: JobEvent,
}

enum JobEvent {
    Scanned(Result<SourcedDetection>),
    Exported(Result<ExportReport>),
}

struct SceneSplitterApp {
    settings: Settings,
    /// The settings as last written to disk, to notice when they change.
    saved_settings: Settings,
    /// Why the settings could not be saved, when that happened.
    settings_problem: String,
    setup: Setup,
    jobs: Vec<Job>,
    /// The job shown on the right.
    current: Option<usize>,
    updates_sender: Sender<JobUpdate>,
    updates: Receiver<JobUpdate>,
    pasted_path: String,
    add_feedback: String,
    player: Option<ActivePlayer>,
}

/// Whether the programs that do the video work are in place.
enum Setup {
    Ready,
    Downloading {
        updates: Receiver<SetupUpdate>,
        progress: DownloadProgress,
    },
    Failed(String),
}

enum SetupUpdate {
    Progress(DownloadProgress),
    Finished(Result<(), String>),
}

/// The player, and the job whose video it is playing.
struct ActivePlayer {
    job: usize,
    player: Player,
}

impl eframe::App for SceneSplitterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.follow_setup();
        self.apply_job_updates();
        self.add_dropped_paths(ctx);
        self.start_next_scan(ctx);

        egui::SidePanel::left("queue")
            .frame(theme::panel(theme::SIDEBAR))
            .default_width(QUEUE_PANEL_WIDTH)
            .show(ctx, |ui| self.show_queue(ui));
        self.close_player_unless_its_video_is_in_review();
        self.show_current_job(ctx);
        self.show_player(ctx);
        self.save_settings_once_changed(ctx);
    }
}

impl SceneSplitterApp {
    fn new(settings: Settings) -> Self {
        let (updates_sender, updates) = mpsc::channel();
        Self {
            settings,
            saved_settings: settings,
            settings_problem: String::new(),
            setup: Setup::Ready,
            jobs: Vec::new(),
            current: None,
            updates_sender,
            updates,
            pasted_path: String::new(),
            add_feedback: String::new(),
            player: None,
        }
    }

    /// On a machine without ffmpeg, starts downloading it in the background.
    fn get_tools_if_missing(&mut self, ctx: &egui::Context) {
        if tools::find_installed() == Availability::Ready {
            self.setup = Setup::Ready;
            return;
        }
        let (updates_sender, updates) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // Sends only fail once the window is gone, when nobody is
            // waiting for the download any more.
            let outcome = tools::download(|progress| {
                if updates_sender.send(SetupUpdate::Progress(progress)).is_ok() {
                    ctx.request_repaint();
                }
            });
            let finished = SetupUpdate::Finished(outcome.map_err(|error| format!("{error:#}")));
            if updates_sender.send(finished).is_ok() {
                ctx.request_repaint();
            }
        });
        self.setup = Setup::Downloading {
            updates,
            progress: DownloadProgress {
                received_bytes: 0,
                total_bytes: 0,
            },
        };
    }

    fn follow_setup(&mut self) {
        let Setup::Downloading { updates, progress } = &mut self.setup else {
            return;
        };
        let mut finished = None;
        for update in updates.try_iter() {
            match update {
                SetupUpdate::Progress(latest) => *progress = latest,
                SetupUpdate::Finished(outcome) => finished = Some(outcome),
            }
        }
        match finished {
            Some(Ok(())) => self.setup = Setup::Ready,
            Some(Err(reason)) => self.setup = Setup::Failed(reason),
            None => {}
        }
    }

    /// Settings are saved as soon as the person lets go of a control, not
    /// on every step of dragging a number.
    fn save_settings_once_changed(&mut self, ctx: &egui::Context) {
        let still_adjusting = ctx.input(|input| input.pointer.any_down());
        if self.settings == self.saved_settings || still_adjusting {
            return;
        }
        // Remembered as saved even on failure, so a folder that cannot be
        // written is reported once and not retried on every frame.
        self.saved_settings = self.settings;
        self.settings_problem = match self.settings.save() {
            Ok(()) => String::new(),
            Err(error) => format!("Could not save the settings: {error:#}"),
        };
    }

    fn add_dropped_paths(&mut self, ctx: &egui::Context) {
        let dropped_paths: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        for path in dropped_paths {
            self.add_path(path);
        }
    }

    /// Queues a video, or every video directly inside a folder.
    fn add_path(&mut self, path: PathBuf) {
        self.add_feedback = match library::videos_at(&path) {
            Ok(videos) if videos.is_empty() => format!("No videos found in {}.", path.display()),
            Ok(videos) => {
                let added = videos
                    .into_iter()
                    .filter(|video| self.queue_video(video))
                    .count();
                format!("Added {}.", counted(added, "video"))
            }
            Err(error) => format!("{error:#}"),
        };
    }

    /// Chooses the scan for every video still waiting in the queue.
    fn scan_queued_videos_as(&mut self, depth: ScanDepth) {
        let scan_kind = match depth {
            ScanDepth::BlackScreens => ScanKind::BlackScreens,
            ScanDepth::WithHardCuts => ScanKind::HardCuts(self.settings.hard_cuts),
        };
        for job in &mut self.jobs {
            if matches!(job.stage, JobStage::Queued) {
                job.scan_kind = scan_kind;
            }
        }
    }

    /// Returns false for a video that is already in the queue.
    fn queue_video(&mut self, video: &Path) -> bool {
        if self.jobs.iter().any(|job| &*job.video == video) {
            return false;
        }
        self.jobs.push(Job {
            video: Arc::from(video),
            stage: JobStage::Queued,
            cache_use: CacheUse::ReuseStoredScan,
            scan_kind: ScanKind::BlackScreens,
        });
        if self.current.is_none() {
            self.current = Some(self.jobs.len() - 1);
        }
        true
    }

    /// Scans one video at a time. A scan already uses every core, so running
    /// several at once would only make each of them slower.
    fn start_next_scan(&mut self, ctx: &egui::Context) {
        // Scanning needs ffmpeg, so videos wait in the queue until it is here.
        if !matches!(self.setup, Setup::Ready) {
            return;
        }
        let scan_in_flight = self
            .jobs
            .iter()
            .any(|job| matches!(job.stage, JobStage::Scanning));
        if scan_in_flight {
            return;
        }
        let Some(job_index) = self
            .jobs
            .iter()
            .position(|job| matches!(job.stage, JobStage::Queued))
        else {
            return;
        };

        let job = &mut self.jobs[job_index];
        job.stage = JobStage::Scanning;
        let video = Arc::clone(&job.video);
        let cache_use = job.cache_use;
        let depth = job.scan_kind.depth();
        let settings = self.settings.scan;
        self.run_in_background(ctx, job_index, move || {
            JobEvent::Scanned(cache::detect_with_cache(
                &video, &settings, cache_use, depth,
            ))
        });
    }

    /// The player belongs to the video under review. Switching to another
    /// video, or sending this one off to be split, closes it.
    fn close_player_unless_its_video_is_in_review(&mut self) {
        let still_in_review = self.player.as_ref().is_some_and(|active| {
            self.current == Some(active.job)
                && matches!(self.jobs[active.job].stage, JobStage::Reviewing(_))
        });
        if !still_in_review {
            self.player = None;
        }
    }

    fn show_player(&mut self, ctx: &egui::Context) {
        let Some(active) = &mut self.player else {
            return;
        };
        let split_would_land_at = match &self.jobs[active.job].stage {
            JobStage::Reviewing(review) => review.split_time_near(active.player.position()),
            _ => None,
        };
        match active.player.show(ctx, split_would_land_at) {
            PlayerEvent::StillOpen => {}
            PlayerEvent::Closed => self.player = None,
            PlayerEvent::SplitRequested(time) => {
                // The player only stays open while its video is in review.
                if let JobStage::Reviewing(review) = &mut self.jobs[active.job].stage {
                    active.player.show_split_feedback(review.split_near(time));
                }
            }
        }
    }

    fn start_export(&mut self, ctx: &egui::Context, job_index: usize) {
        // The player keeps the video file open, which would stop the
        // original from being recycled after the split.
        self.player = None;

        let job = &mut self.jobs[job_index];
        let JobStage::Reviewing(review) = &job.stage else {
            return;
        };
        let video = Arc::clone(&job.video);
        let cuts = review.accepted_cuts();
        let policy = self.settings.export.original_policy;
        let out_dir = match split::scene_dir(&video, self.settings.export.scene_location) {
            Ok(out_dir) => out_dir,
            Err(error) => {
                job.stage = JobStage::Failed(format!("{error:#}"));
                return;
            }
        };
        job.stage = JobStage::Exporting;

        self.run_in_background(ctx, job_index, move || {
            JobEvent::Exported(export::export_scenes(&video, &cuts, &out_dir, policy))
        });
        self.move_on_from(job_index);
    }

    fn run_in_background(
        &self,
        ctx: &egui::Context,
        job: usize,
        work: impl FnOnce() -> JobEvent + Send + 'static,
    ) {
        let updates_sender = self.updates_sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let event = work();
            // The send only fails once the window is gone, when nobody is
            // waiting for the result any more.
            if updates_sender.send(JobUpdate { job, event }).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Shows the next video that still needs attention, so the person can
    /// keep reviewing while the last one is written.
    fn move_on_from(&mut self, finished_job: usize) {
        let later_jobs = finished_job + 1..self.jobs.len();
        let earlier_jobs = 0..finished_job;
        let next_open_job = later_jobs
            .chain(earlier_jobs)
            .find(|&job| self.jobs[job].stage.is_open());
        if let Some(job) = next_open_job {
            self.current = Some(job);
        }
    }

    fn apply_job_updates(&mut self) {
        let updates: Vec<JobUpdate> = self.updates.try_iter().collect();
        for update in updates {
            let job = &mut self.jobs[update.job];
            job.stage = match update.event {
                JobEvent::Scanned(Ok(sourced)) => {
                    let detection = sourced.detection;
                    let cuts = match job.scan_kind {
                        ScanKind::BlackScreens => detection.cuts.clone(),
                        ScanKind::HardCuts(settings) => detection.cuts_with_hard_cuts(&settings),
                    };
                    let source_note = match sourced.source {
                        DetectionSource::FreshScan => "",
                        DetectionSource::StoredScan => "Loaded from an earlier scan.",
                    };
                    let scan_note = match job.scan_kind {
                        ScanKind::BlackScreens => source_note.to_owned(),
                        ScanKind::HardCuts(_) => format!(
                            "{} added. {source_note}",
                            counted(cuts.len() - detection.cuts.len(), "hard cut")
                        ),
                    };
                    JobStage::Reviewing(Box::new(Review::new(
                        Arc::clone(&job.video),
                        detection,
                        cuts,
                        &scan_note,
                    )))
                }
                JobEvent::Scanned(Err(error)) => {
                    JobStage::Failed(format!("Could not scan this video:\n{error:#}"))
                }
                JobEvent::Exported(Ok(report)) => JobStage::Finished(report),
                JobEvent::Exported(Err(error)) => {
                    JobStage::Failed(format!("Could not split this video:\n{error:#}"))
                }
            };
        }
    }

    /// Plays the video under review from `time`, in the player that is
    /// already open if there is one.
    fn play_from(&mut self, ctx: &egui::Context, job_index: usize, time: f64) {
        if let Some(active) = &mut self.player {
            active.player.play_at(time, ctx);
            return;
        }
        let job = &self.jobs[job_index];
        let JobStage::Reviewing(review) = &job.stage else {
            return;
        };
        self.player = Some(ActivePlayer {
            job: job_index,
            player: Player::play_from(
                Arc::clone(&job.video),
                review.video_length(),
                review.frame_seconds(),
                time,
                ctx,
            ),
        });
    }

    fn show_queue(&mut self, ui: &mut egui::Ui) {
        ui.label(theme::semibold("Scene Splitter", 20.0).color(theme::PRIMARY));
        ui.label(theme::caption(
            "Drop videos or a folder anywhere, or paste a path.",
        ));
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.pasted_path)
                .hint_text(r"C:\Videos or C:\Videos\film.mp4")
                .desired_width(f32::INFINITY),
        );
        let pressed_enter =
            field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if ui.button("Add").clicked() || pressed_enter {
            if let Some(path) = path_from_pasted_text(&self.pasted_path) {
                self.add_path(path);
                self.pasted_path.clear();
            }
        }
        ui.label(theme::caption(&self.add_feedback));
        ui.add_space(4.0);
        egui::CollapsingHeader::new(theme::semibold("Settings", 14.0)).show(ui, |ui| {
            show_settings(ui, &mut self.settings, &self.settings_problem)
        });
        ui.label(theme::semibold("Videos", 14.0));

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (job_index, job) in self.jobs.iter().enumerate() {
                    let is_current = self.current == Some(job_index);
                    if show_job_card(ui, job_index, job, is_current) {
                        self.current = Some(job_index);
                    }
                }
            });
    }

    fn show_current_job(&mut self, ctx: &egui::Context) {
        if !matches!(self.setup, Setup::Ready) {
            if show_setup(ctx, &self.setup) {
                self.get_tools_if_missing(ctx);
            }
            return;
        }
        let Some(job_index) = self.current else {
            show_message(ctx, DROP_PROMPT, DROP_EXPLANATION);
            return;
        };
        let job = &mut self.jobs[job_index];
        let name = file_name(&job.video);

        let request = match &mut job.stage {
            JobStage::Reviewing(review) => {
                let playhead = self
                    .player
                    .as_ref()
                    .filter(|active| active.job == job_index)
                    .map(|active| active.player.position());
                review.show(
                    ctx,
                    &mut self.settings.export,
                    &mut self.settings.hard_cuts,
                    playhead,
                )
            }
            JobStage::Queued => {
                show_message(ctx, &name, "Waiting for its turn to be scanned.");
                None
            }
            JobStage::Scanning => {
                let what = match job.scan_kind {
                    ScanKind::BlackScreens => format!("Scanning {name}"),
                    ScanKind::HardCuts(_) => {
                        format!("Looking at every frame of {name} for hard cuts")
                    }
                };
                show_busy(ctx, &what);
                None
            }
            JobStage::Exporting => {
                show_busy(ctx, &format!("Writing and verifying the scenes of {name}"));
                None
            }
            JobStage::Finished(report) => {
                if show_finished(ctx, &name, report) {
                    open_in_file_manager(&report.out_dir);
                }
                None
            }
            JobStage::Skipped => {
                show_message(ctx, &name, "Skipped, nothing was written.");
                None
            }
            JobStage::Failed(reason) => {
                show_message(ctx, &name, reason);
                None
            }
        };

        match request {
            Some(ReviewRequest::Export) => self.start_export(ctx, job_index),
            Some(ReviewRequest::PlayFrom(time)) => self.play_from(ctx, job_index, time),
            Some(ReviewRequest::ScanAgain) => {
                let job = &mut self.jobs[job_index];
                job.cache_use = CacheUse::ScanFresh;
                job.scan_kind = ScanKind::BlackScreens;
                job.stage = JobStage::Queued;
            }
            Some(ReviewRequest::FindHardCuts) => {
                let job = &mut self.jobs[job_index];
                // The slow part of an earlier hard-cut scan is stored, so
                // only changed settings are applied when it can be reused.
                job.cache_use = CacheUse::ReuseStoredScan;
                job.scan_kind = ScanKind::HardCuts(self.settings.hard_cuts);
                job.stage = JobStage::Queued;
            }
            Some(ReviewRequest::Skip) => {
                self.jobs[job_index].stage = JobStage::Skipped;
                self.move_on_from(job_index);
            }
            None => {}
        }
    }
}

/// Windows' "Copy as path" wraps the path in double quotes, so those are
/// dropped. Returns `None` when nothing was pasted.
fn path_from_pasted_text(text: &str) -> Option<PathBuf> {
    let path = text.trim().trim_matches('"').trim();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

/// The settings section. Whatever changes here is saved and used from then
/// on, by the window and by the command line.
fn show_settings(ui: &mut egui::Ui, settings: &mut Settings, problem: &str) {
    ui.label(theme::semibold("Black screens", 13.0));
    let criteria = &mut settings.scan.criteria;
    ui.horizontal(|ui| {
        ui.label("Shortest");
        ui.add(
            egui::DragValue::new(&mut criteria.min_seconds)
                .range(SHORTEST_BLACK_SECONDS)
                .speed(0.01)
                .fixed_decimals(2)
                .suffix(" s"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Darkest level").on_hover_text(
            "The brightest a pixel may be, from 0 to 255, and still count as black.",
        );
        ui.add(egui::DragValue::new(&mut criteria.max_dark_luma));
    });
    ui.horizontal(|ui| {
        ui.label("Share of the frame").on_hover_text(
            "How much of the picture must be black. A logo in a corner is why this is not 100%.",
        );
        let mut percent = criteria.min_dark_fraction * PERCENT;
        let share = egui::DragValue::new(&mut percent)
            .range(BLACK_SHARE_PERCENT)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" %");
        if ui.add(share).changed() {
            criteria.min_dark_fraction = percent / PERCENT;
        }
    });
    let mut every_frame = settings.scan.mode == ScanMode::EveryFrame;
    let every_frame_box = ui
        .checkbox(&mut every_frame, "Decode every frame")
        .on_hover_text("Much slower. For videos whose black screens are grainy and get missed.");
    if every_frame_box.changed() {
        settings.scan.mode = if every_frame {
            ScanMode::EveryFrame
        } else {
            ScanMode::Suspicious
        };
    }

    ui.add_space(4.0);
    ui.label(theme::semibold("Hard cuts", 13.0));
    show_hard_cut_settings(ui, &mut settings.hard_cuts);

    ui.add_space(4.0);
    ui.label(theme::semibold("Scene files", 13.0));
    let mut recycle = settings.export.original_policy == OriginalPolicy::RecycleOnceVerified;
    if ui
        .checkbox(
            &mut recycle,
            format!("Move the original to the {TRASH_NAME} once verified"),
        )
        .changed()
    {
        settings.export.original_policy = if recycle {
            OriginalPolicy::RecycleOnceVerified
        } else {
            OriginalPolicy::Keep
        };
    }
    let mut new_folder = settings.export.scene_location == SceneLocation::NewFolder;
    if ui
        .checkbox(&mut new_folder, "Put scenes in a new folder")
        .changed()
    {
        settings.export.scene_location = if new_folder {
            SceneLocation::NewFolder
        } else {
            SceneLocation::BesideVideo
        };
    }

    ui.add_space(4.0);
    ui.label(theme::caption(
        "Saved as you change them. Detection settings apply to videos scanned from now on.",
    ));
    if let Some(path) = settings::settings_path() {
        ui.label(theme::caption(path.display().to_string()));
    }
    if !problem.is_empty() {
        ui.colored_label(theme::DANGER, problem);
    }
    if ui.small_button("Back to defaults").clicked() {
        *settings = Settings::default();
    }
}

fn show_hard_cut_settings(ui: &mut egui::Ui, hard_cuts: &mut HardCutSettings) {
    ui.horizontal(|ui| {
        ui.label("Sensitivity");
        egui::ComboBox::from_id_salt("settings_hard_cut_sensitivity")
            .selected_text(hard_cuts.sensitivity.label())
            .show_ui(ui, |ui| {
                for sensitivity in Sensitivity::ALL {
                    ui.selectable_value(
                        &mut hard_cuts.sensitivity,
                        sensitivity,
                        sensitivity.label(),
                    );
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Shortest scene");
        let mut minutes = hard_cuts.min_scene_seconds / SECONDS_PER_MINUTE;
        let shortest_scene = egui::DragValue::new(&mut minutes)
            .range(SHORTEST_SCENE_MINUTES)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" min");
        if ui.add(shortest_scene).changed() {
            hard_cuts.min_scene_seconds = minutes * SECONDS_PER_MINUTE;
        }
    });
}

/// One video in the sidebar. Returns true when it was clicked.
fn show_job_card(ui: &mut egui::Ui, job_index: usize, job: &Job, is_current: bool) -> bool {
    let (fill, border) = if is_current {
        (theme::PRIMARY_TINT, theme::PRIMARY)
    } else {
        (theme::CARD, theme::CARD_BORDER)
    };
    let card = egui::Frame::none()
        .fill(fill)
        .rounding(JOB_CARD_ROUNDING)
        .stroke(egui::Stroke::new(1.0_f32, border))
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(theme::semibold(file_name(&job.video), 13.5)).truncate());
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(
                    egui::vec2(STATUS_DOT_SIZE, STATUS_DOT_SIZE),
                    egui::Sense::hover(),
                );
                ui.painter().circle_filled(
                    dot.center(),
                    STATUS_DOT_SIZE / 2.0,
                    job.stage.status_color(),
                );
                ui.label(theme::caption(job.stage.status()));
            });
        })
        .response;
    ui.interact(
        card.rect,
        ui.id().with(("job", job_index)),
        egui::Sense::click(),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .clicked()
}

/// The first-run screen. Returns true when the person asked to try the
/// download again.
fn show_setup(ctx: &egui::Context, setup: &Setup) -> bool {
    let mut retry = false;
    show_centered_card(ctx, |ui| match setup {
        Setup::Ready => {}
        Setup::Downloading { progress, .. } => {
            ui.label(theme::semibold("Getting ffmpeg", 18.0));
            ui.label(theme::muted(format!(
                "Scene Splitter uses ffmpeg {} for all video work. It is downloaded once, about {} MB. \
                 You can add videos meanwhile.",
                tools::FFMPEG_VERSION,
                megabytes(progress.total_bytes.max(progress.received_bytes))
            )));
            ui.add(egui::ProgressBar::new(progress.fraction()).show_percentage());
        }
        Setup::Failed(reason) => {
            ui.label(theme::semibold("ffmpeg could not be downloaded", 18.0).color(theme::DANGER));
            ui.label(theme::muted(reason));
            ui.label(theme::muted(
                "Check the internet connection and try again, or install ffmpeg yourself and restart.",
            ));
            retry = ui.add(theme::primary_button("Try again")).clicked();
        }
    });
    retry
}

fn megabytes(bytes: u64) -> u64 {
    bytes / BYTES_PER_MEGABYTE
}

/// A card in the middle of the main area, for everything that is not the
/// review screen.
fn show_centered_card(ctx: &egui::Context, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default()
        .frame(theme::panel(theme::BACKGROUND))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 4.0);
                theme::card().show(ui, |ui| {
                    ui.set_width(MESSAGE_CARD_WIDTH);
                    ui.vertical_centered(add_contents);
                });
            });
        });
}

fn show_message(ctx: &egui::Context, title: &str, text: &str) {
    show_centered_card(ctx, |ui| {
        ui.label(theme::semibold(title, 18.0));
        ui.label(theme::muted(text));
    });
}

/// The result of a split. Returns true when the person asked to see the
/// folder the scenes were written to.
fn show_finished(ctx: &egui::Context, name: &str, report: &ExportReport) -> bool {
    let mut open_folder = false;
    show_centered_card(ctx, |ui| {
        let (verdict, verdict_color) = if report.is_clean() {
            ("Done", theme::SUCCESS)
        } else {
            ("Done, but check the note below", theme::DANGER)
        };
        ui.label(theme::semibold(verdict, 22.0).color(verdict_color));
        ui.label(theme::semibold(name, 15.0));
        ui.label(theme::muted(report.summary()));
        open_folder = ui.add(theme::primary_button("Open folder")).clicked();
    });
    open_folder
}

/// Shows a folder in the system's file manager. Failing to is not worth
/// interrupting the person for, the folder's path is on screen anyway.
fn open_in_file_manager(folder: &Path) {
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(error) = Command::new(opener).arg(folder).spawn() {
        eprintln!("Could not open {}: {error}", folder.display());
    }
}

fn show_busy(ctx: &egui::Context, text: &str) {
    show_centered_card(ctx, |ui| {
        ui.spinner();
        ui.label(theme::semibold(text, 16.0));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_path_loses_its_surrounding_quotes() {
        assert_eq!(
            path_from_pasted_text(r#" "E:\Files\My Video.mp4" "#),
            Some(PathBuf::from(r"E:\Files\My Video.mp4"))
        );
    }

    #[test]
    fn a_path_without_quotes_is_kept_as_it_is() {
        assert_eq!(
            path_from_pasted_text(r"E:\Files\video.mp4"),
            Some(PathBuf::from(r"E:\Files\video.mp4"))
        );
    }

    #[test]
    fn an_empty_box_is_not_a_path() {
        assert_eq!(path_from_pasted_text("  "), None);
        assert_eq!(path_from_pasted_text("\"\""), None);
    }
}
