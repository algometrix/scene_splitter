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
use crate::settings::{LoadedSettings, Settings};
use crate::split::{self, SceneLocation};
use crate::theme;
use crate::tools::{self, Availability, DownloadProgress};

const WINDOW_TITLE: &str = "Scene Splitter";
const WINDOW_SIZE: [f32; 2] = [1320.0, 900.0];
/// Below this the review screen cannot show its buttons and a scene card.
const MIN_WINDOW_SIZE: [f32; 2] = [900.0, 600.0];
const QUEUE_PANEL_WIDTH: f32 = 270.0;
const QUEUE_PANEL_WIDTHS: std::ops::RangeInclusive<f32> = 230.0..=380.0;
/// How dark the page goes behind a dialog that has to be answered first.
const BACKDROP: egui::Color32 = egui::Color32::from_rgba_premultiplied(14, 16, 25, 110);
const MESSAGE_CARD_WIDTH: f32 = 560.0;
/// Room kept on both sides of a card or dialog, for its margin and shadow.
const CARD_SIDE_ROOM: f32 = 48.0;
const DETAILS_MAX_HEIGHT: f32 = 140.0;
/// The width of the numbered pill and its gap on the welcome card.
const STEP_NUMBER_ROOM: f32 = 48.0;
const JOB_CARD_ROUNDING: f32 = 10.0;
const STATUS_DOT_SIZE: f32 = 8.0;
const ADD_DIALOG_WIDTH: f32 = 440.0;
/// Room left beside the path box for its Add button.
const ADD_BUTTON_SPACE: f32 = 70.0;
const PATH_HINT: &str = if cfg!(windows) {
    r"C:\Videos or C:\Videos\film.mp4"
} else {
    "/videos or /videos/film.mp4"
};
const DROP_PROMPT: &str = "or drop videos or a folder anywhere in this window";
/// The steps of using the app, shown on the first screen.
const WELCOME_STEPS: [(&str, &str); 3] = [
    (
        "Add videos",
        "Each one is scanned for scene changes. That takes a few seconds for most videos.",
    ),
    (
        "Review the scenes",
        "Every scene is shown with previews. Merge the ones that belong together, or add splits.",
    ),
    (
        "Split",
        "One file per scene is written next to the video and checked against the original. \
         Then the original is moved to the Recycle Bin or Trash. You can turn that off.",
    ),
];
const PERCENT: f64 = 100.0;
const BYTES_PER_MEGABYTE: u64 = 1024 * 1024;
const SECONDS_PER_MINUTE: f64 = 60.0;
/// The ranges the settings section offers.
const SHORTEST_BLACK_SECONDS: std::ops::RangeInclusive<f64> = 0.05..=10.0;
const BLACK_SHARE_PERCENT: std::ops::RangeInclusive<f64> = 50.0..=100.0;
const SHORTEST_SCENE_MINUTES: std::ops::RangeInclusive<f64> = 0.5..=60.0;
const DROP_EXPLANATION: &str =
    "Scene Splitter finds the black screens between scenes and cuts the video there, \
     without re-encoding it.";

/// Opens the window and blocks until it is closed. With `first_path` that
/// video, or every video in that folder, is queued at once, to be scanned
/// as deeply as `first_scan` says.
pub fn run(
    loaded: LoadedSettings,
    first_path: Option<PathBuf>,
    first_scan: ScanDepth,
) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |creation| {
            theme::apply(&creation.egui_ctx);
            let mut app = SceneSplitterApp::new(loaded);
            app.get_tools_if_missing(&creation.egui_ctx);
            if let Some(path) = first_path {
                if let Err(problem) = app.add_path(path) {
                    app.add_feedback = problem;
                }
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
    /// Left as it is. The review is kept, in case that was a slip.
    Skipped(Box<Review>),
    Failed(String),
}

/// What the person asked for on a screen that is not the review.
enum JobAction {
    ReviewAfterAll,
    TryAgain,
    OpenFolder,
    AddMoreVideos,
}

impl JobStage {
    fn status(&self) -> String {
        match self {
            Self::Queued => "Waiting".to_owned(),
            Self::Scanning => "Scanning".to_owned(),
            Self::Reviewing(review) => {
                format!(
                    "Ready to review, {}",
                    counted(review.scene_count(), "scene")
                )
            }
            Self::Exporting => "Writing and checking".to_owned(),
            Self::Finished(report) if report.is_clean() => "Done, checked".to_owned(),
            Self::Finished(_) => "Done, check the result".to_owned(),
            Self::Skipped(_) => "Skipped".to_owned(),
            Self::Failed(_) => "Failed".to_owned(),
        }
    }

    fn status_color(&self) -> egui::Color32 {
        match self {
            Self::Queued | Self::Skipped(_) => theme::MUTED_TEXT,
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
    /// The background work stopped without a result.
    Crashed,
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
    add_dialog: AddDialog,
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

/// The dialog for adding videos, opened from the sidebar or with Ctrl+O.
#[derive(Default)]
struct AddDialog {
    open: bool,
    pasted_path: String,
    /// Set when the dialog opens, so the path box takes the keyboard once.
    focus_path_box: bool,
    /// Why the last thing chosen could not be added.
    problem: String,
}

/// What happened to a video that was asked to be queued.
enum Queued {
    Added,
    /// It is in the list already, as this job.
    AlreadyListed(usize),
}

fn counted_already_listed(video_count: usize) -> String {
    if video_count == 1 {
        "That video is already in the list.".to_owned()
    } else {
        "Those videos are already in the list.".to_owned()
    }
}

/// What the person chose in the add dialog during one frame.
enum AddDialogChoice {
    Paths(Vec<PathBuf>),
    Close,
}

/// The player, and the job whose video it is playing.
struct ActivePlayer {
    job: usize,
    player: Player,
}

impl eframe::App for SceneSplitterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.open_add_dialog_on_its_shortcut(ctx);
        self.follow_setup();
        self.apply_job_updates();
        self.add_dropped_paths(ctx);
        self.start_next_scan(ctx);

        egui::SidePanel::left("queue")
            .frame(theme::panel(theme::SIDEBAR))
            .default_width(QUEUE_PANEL_WIDTH)
            .width_range(QUEUE_PANEL_WIDTHS)
            .show(ctx, |ui| {
                // One scroll area for the whole panel, so opening the
                // settings can never push the video list out of reach.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.show_queue(ui));
            });
        self.close_player_unless_its_video_is_in_review();
        self.show_current_job(ctx);
        self.show_add_dialog(ctx);
        self.show_player(ctx);
        self.save_settings_once_changed(ctx);
    }
}

impl SceneSplitterApp {
    fn new(loaded: LoadedSettings) -> Self {
        let (updates_sender, updates) = mpsc::channel();
        Self {
            settings: loaded.settings,
            saved_settings: loaded.settings,
            settings_problem: loaded.problem.unwrap_or_default(),
            setup: Setup::Ready,
            jobs: Vec::new(),
            current: None,
            updates_sender,
            updates,
            add_dialog: AddDialog::default(),
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
                total_bytes: tools::download_bytes(),
            },
        };
    }

    fn follow_setup(&mut self) {
        let Setup::Downloading { updates, progress } = &mut self.setup else {
            return;
        };
        let mut finished = None;
        loop {
            match updates.try_recv() {
                Ok(SetupUpdate::Progress(latest)) => *progress = latest,
                Ok(SetupUpdate::Finished(outcome)) => finished = Some(outcome),
                Err(mpsc::TryRecvError::Empty) => break,
                // The download thread is gone without saying how it ended.
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished.get_or_insert(Err("The download stopped unexpectedly.".to_owned()));
                    break;
                }
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
            if let Err(problem) = self.add_path(path) {
                self.add_feedback = problem;
            }
        }
    }

    fn open_add_dialog(&mut self) {
        self.add_dialog.open = true;
        self.add_dialog.focus_path_box = true;
    }

    fn open_add_dialog_on_its_shortcut(&mut self, ctx: &egui::Context) {
        let pressed = ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::O));
        if pressed {
            self.open_add_dialog();
        }
    }

    fn show_add_dialog(&mut self, ctx: &egui::Context) {
        if !self.add_dialog.open {
            return;
        }
        match show_add_dialog(ctx, &mut self.add_dialog) {
            Some(AddDialogChoice::Paths(paths)) => {
                // The dialog stays open on a mistake, with the reason in it,
                // so a mistyped path can be corrected in place.
                let problems: Vec<String> = paths
                    .into_iter()
                    .filter_map(|path| self.add_path(path).err())
                    .collect();
                if problems.is_empty() {
                    self.add_dialog = AddDialog::default();
                } else {
                    self.add_dialog.problem = problems.join(" ");
                }
            }
            Some(AddDialogChoice::Close) => self.add_dialog = AddDialog::default(),
            None => {}
        }
    }

    /// Queues a video, or every video directly inside a folder, and says
    /// what happened in the sidebar. Fails with a sentence for the person
    /// when the path leads to nothing that can be added.
    fn add_path(&mut self, path: PathBuf) -> Result<(), String> {
        let videos = library::videos_at(&path).map_err(|error| format!("{error:#}"))?;
        if videos.is_empty() {
            return Err(format!("No videos were found in {}.", path.display()));
        }

        let mut added = 0;
        let mut already_listed = None;
        for video in &videos {
            match self.queue_video(video) {
                Queued::Added => added += 1,
                Queued::AlreadyListed(job) => already_listed = Some(job),
            }
        }
        self.add_feedback = match (added, already_listed) {
            (0, Some(job)) => {
                // Show the one they asked for, since it is here already.
                self.current = Some(job);
                counted_already_listed(videos.len())
            }
            _ => format!(
                "Added {}. Scanning starts by itself.",
                counted(added, "video")
            ),
        };
        Ok(())
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

    fn queue_video(&mut self, video: &Path) -> Queued {
        if let Some(job) = self.jobs.iter().position(|job| &*job.video == video) {
            return Queued::AlreadyListed(job);
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
        Queued::Added
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
            // Without a result the job would stay "scanning" for ever and
            // hold up every video queued behind it.
            let event = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or(JobEvent::Crashed);
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
                        DetectionSource::StoredScan => "Using the scan from last time.",
                    };
                    // The review words what was found itself, from the cuts it is given.
                    let scan_note = source_note;
                    JobStage::Reviewing(Box::new(Review::new(
                        Arc::clone(&job.video),
                        detection,
                        cuts,
                        scan_note,
                    )))
                }
                JobEvent::Scanned(Err(error)) => JobStage::Failed(format!(
                    "This file could not be read as a video. It may be damaged, or it is not a video.\n\nDetails: {error:#}"
                )),
                JobEvent::Exported(Ok(report)) => JobStage::Finished(report),
                JobEvent::Exported(Err(error)) => JobStage::Failed(format!(
                    "The video could not be split. The original was not changed.\n\nDetails: {error:#}"
                )),
                JobEvent::Crashed => JobStage::Failed(
                    "Something went wrong while working on this video. The original was not changed. \
                     Scene files that were already written may be incomplete."
                        .to_owned(),
                ),
            };
        }
    }

    fn apply_job_action(&mut self, job_index: usize, action: JobAction) {
        let job = &mut self.jobs[job_index];
        match action {
            JobAction::ReviewAfterAll => {
                if let JobStage::Skipped(review) =
                    std::mem::replace(&mut job.stage, JobStage::Queued)
                {
                    job.stage = JobStage::Reviewing(review);
                }
            }
            JobAction::TryAgain => {
                job.cache_use = CacheUse::ReuseStoredScan;
                job.stage = JobStage::Queued;
            }
            JobAction::OpenFolder => {
                if let JobStage::Finished(report) = &job.stage {
                    open_in_file_manager(&report.out_dir);
                }
            }
            JobAction::AddMoreVideos => self.open_add_dialog(),
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
        let add_button = ui
            .add(theme::primary_button("Add videos"))
            .on_hover_text("Choose videos or a folder, or paste a path (Ctrl+O).");
        if add_button.clicked() {
            self.open_add_dialog();
        }
        ui.label(theme::caption("Or drop videos or a folder anywhere."));
        if !self.add_feedback.is_empty() {
            ui.label(theme::caption(&self.add_feedback));
        }

        ui.add_space(4.0);
        ui.label(theme::semibold("Videos", 14.0));
        if self.jobs.is_empty() {
            ui.label(theme::caption(
                "Videos you add are listed here with their progress. Click one to open it.",
            ));
        }
        for (job_index, job) in self.jobs.iter().enumerate() {
            let is_current = self.current == Some(job_index);
            if show_job_card(ui, job_index, job, is_current) {
                self.current = Some(job_index);
            }
        }

        ui.add_space(8.0);
        egui::CollapsingHeader::new(theme::semibold("Settings", 14.0)).show(ui, |ui| {
            show_settings(ui, &mut self.settings, &self.settings_problem)
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
            if show_welcome(ctx) {
                self.open_add_dialog();
            }
            return;
        };
        let job = &mut self.jobs[job_index];
        let name = file_name(&job.video);

        let mut action = None;
        let request = match &mut job.stage {
            JobStage::Reviewing(review) => {
                let playhead = self
                    .player
                    .as_ref()
                    .filter(|active| active.job == job_index)
                    .map(|active| active.player.position());
                review.show(ctx, &mut self.settings, playhead)
            }
            JobStage::Queued => {
                show_message(ctx, &name, "Waiting. Another video is being scanned first.");
                None
            }
            JobStage::Scanning => {
                let what = match job.scan_kind {
                    ScanKind::BlackScreens => format!("Scanning {name} for scene changes"),
                    ScanKind::HardCuts(_) => format!(
                        "Looking at every picture of {name} for sudden changes. \
                         This takes much longer than the first scan."
                    ),
                };
                show_busy(ctx, &what);
                None
            }
            JobStage::Exporting => {
                show_busy(ctx, &format!("Writing and checking the scenes of {name}"));
                None
            }
            JobStage::Finished(report) => {
                action = show_finished(ctx, &name, report);
                None
            }
            JobStage::Skipped(_) => {
                action = show_skipped(ctx, &name);
                None
            }
            JobStage::Failed(reason) => {
                action = show_failed(ctx, &name, reason);
                None
            }
        };
        if let Some(action) = action {
            self.apply_job_action(job_index, action);
        }

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
                let job = &mut self.jobs[job_index];
                // The review moves into the skipped stage, so it can be
                // taken up again exactly as it was.
                if let JobStage::Reviewing(review) =
                    std::mem::replace(&mut job.stage, JobStage::Queued)
                {
                    job.stage = JobStage::Skipped(review);
                }
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
    if !problem.is_empty() {
        ui.colored_label(theme::DANGER, problem);
    }

    ui.label(theme::semibold("When a video is split", 13.0));
    let mut recycle = settings.export.original_policy == OriginalPolicy::RecycleOnceVerified;
    let recycle_box = ui
        .checkbox(
            &mut recycle,
            format!("Move the original to the {TRASH_NAME} after the check"),
        )
        .on_hover_text(
            "The new files are compared with the original first. If anything is missing or different, the original stays.",
        );
    if recycle_box.changed() {
        settings.export.original_policy = if recycle {
            OriginalPolicy::RecycleOnceVerified
        } else {
            OriginalPolicy::Keep
        };
    }
    let mut new_folder = settings.export.scene_location == SceneLocation::NewFolder;
    if ui
        .checkbox(&mut new_folder, "Put the scenes in a new folder")
        .changed()
    {
        settings.export.scene_location = if new_folder {
            SceneLocation::NewFolder
        } else {
            SceneLocation::BesideVideo
        };
    }
    ui.checkbox(&mut settings.show_tips, "Show tips on the review screen");

    ui.add_space(4.0);
    ui.label(theme::semibold("Looking for missed scenes", 13.0));
    show_hard_cut_settings(ui, &mut settings.hard_cuts);

    // What counts as a black screen needs some knowledge of video to set,
    // and the defaults suit most films, so it stays folded away.
    ui.add_space(4.0);
    egui::CollapsingHeader::new("Advanced: what counts as a black screen").show(ui, |ui| {
        show_black_screen_settings(ui, settings);
    });

    ui.add_space(4.0);
    ui.label(theme::caption(
        "Changes are saved straight away. Detection settings apply the next time a video is scanned.",
    ));
    let restore = ui
        .button("Restore default settings")
        .on_hover_text("Puts every setting here back to how it was at first.");
    if restore.clicked() {
        *settings = Settings::default();
    }
}

fn show_black_screen_settings(ui: &mut egui::Ui, settings: &mut Settings) {
    let criteria = &mut settings.scan.criteria;
    ui.horizontal(|ui| {
        ui.label("Shortest black screen");
        ui.add(
            egui::DragValue::new(&mut criteria.min_seconds)
                .range(SHORTEST_BLACK_SECONDS)
                .speed(0.01)
                .fixed_decimals(2)
                .suffix(" s"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("How dark counts as black").on_hover_text(
            "0 is pure black, 255 is white. Raise this if dark grey screens are missed.",
        );
        ui.add(egui::DragValue::new(&mut criteria.max_dark_luma));
    });
    ui.horizontal(|ui| {
        ui.label("How much of the picture must be black")
            .on_hover_text("A logo in a corner is why this is not 100%.");
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
        .checkbox(&mut every_frame, "Check every picture (much slower)")
        .on_hover_text(
            "Use this if black screens are being missed, for example in grainy footage.",
        );
    if every_frame_box.changed() {
        settings.scan.mode = if every_frame {
            ScanMode::EveryFrame
        } else {
            ScanMode::Suspicious
        };
    }
}

fn show_hard_cut_settings(ui: &mut egui::Ui, hard_cuts: &mut HardCutSettings) {
    ui.horizontal(|ui| {
        ui.label("Detect");
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
        ui.label("Ignore scenes shorter than");
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
            ui.add(egui::Label::new(theme::semibold(file_name(&job.video), 14.0)).truncate());
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
            ui.label(theme::semibold("Setting up (one time only)", 16.0));
            ui.label(theme::muted(format!(
                "Scene Splitter is downloading the video tools it needs (ffmpeg {}, about {} MB). \
                 You can add videos now. Scanning starts when the download is done.",
                tools::FFMPEG_VERSION,
                megabytes(progress.total_bytes.max(progress.received_bytes))
            )));
            ui.add(egui::ProgressBar::new(progress.fraction()).show_percentage());
        }
        Setup::Failed(reason) => {
            ui.label(
                theme::semibold("The video tools could not be downloaded", 16.0)
                    .color(theme::DANGER),
            );
            ui.label("Check your internet connection and press Try again.");
            show_details(ui, reason);
            retry = ui.add(theme::primary_button("Try again")).clicked();
        }
    });
    retry
}

/// The technical reason behind a message, readable and copyable but not in
/// the way of the plain sentence above it.
fn show_details(ui: &mut egui::Ui, details: &str) {
    egui::ScrollArea::vertical()
        .max_height(DETAILS_MAX_HEIGHT)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(theme::caption(details))
                    .selectable(true)
                    .wrap(),
            );
        });
}

fn megabytes(bytes: u64) -> u64 {
    bytes / BYTES_PER_MEGABYTE
}

/// A card in the middle of the main area, for everything that is not the
/// review screen. It narrows with the window instead of being cut off.
fn show_centered_card(ctx: &egui::Context, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default()
        .frame(theme::panel(theme::BACKGROUND))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 5.0);
                let width = MESSAGE_CARD_WIDTH.min(ui.available_width() - CARD_SIDE_ROOM);
                theme::card().show(ui, |ui| {
                    ui.set_width(width);
                    ui.vertical_centered(add_contents);
                });
            });
        });
}

/// Dims the page behind a dialog. Returns true when the person clicked the
/// dimmed part, which closes the dialog like a click beside any menu does.
fn show_backdrop(ctx: &egui::Context) -> bool {
    egui::Area::new(egui::Id::new("dialog_backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            let response = ui.allocate_rect(screen, egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, BACKDROP);
            response.clicked()
        })
        .inner
}

/// The add dialog: the system's own file and folder pickers, and a box to
/// paste a path into. Returns what the person chose, if anything yet.
fn show_add_dialog(ctx: &egui::Context, dialog: &mut AddDialog) -> Option<AddDialogChoice> {
    let clicked_outside = show_backdrop(ctx);
    let mut choice = None;
    let mut open = true;
    egui::Window::new("Add videos")
        .open(&mut open)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_width(ADD_DIALOG_WIDTH.min(ctx.screen_rect().width() - CARD_SIDE_ROOM));
            ui.horizontal_wrapped(|ui| {
                if ui.add(theme::primary_button("Choose videos")).clicked() {
                    // The system's picker blocks until it is closed, which
                    // is what a person expects of it.
                    let picked = rfd::FileDialog::new()
                        .add_filter("Videos", &library::VIDEO_EXTENSIONS)
                        .pick_files();
                    choice = picked.map(AddDialogChoice::Paths);
                }
                let choose_folder = egui::Button::new("Choose a folder")
                    .min_size(egui::vec2(0.0, theme::PRIMARY_BUTTON_HEIGHT));
                if ui.add(choose_folder).clicked() {
                    let picked = rfd::FileDialog::new().pick_folder();
                    choice = picked.map(|folder| AddDialogChoice::Paths(vec![folder]));
                }
            });
            ui.label(theme::caption(
                "A folder adds the videos in it, but not those in its subfolders.",
            ));

            ui.add_space(6.0);
            ui.label("Or paste the location of a video or folder");
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut dialog.pasted_path)
                        .hint_text(PATH_HINT)
                        .desired_width(ui.available_width() - ADD_BUTTON_SPACE),
                );
                if std::mem::take(&mut dialog.focus_path_box) {
                    field.request_focus();
                }
                let pasted = path_from_pasted_text(&dialog.pasted_path);
                let pressed_enter =
                    field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                let add_clicked = ui
                    .add_enabled(pasted.is_some(), egui::Button::new("Add"))
                    .clicked();
                if add_clicked || pressed_enter {
                    choice = pasted.map(|path| AddDialogChoice::Paths(vec![path]));
                }
            });
            if !dialog.problem.is_empty() {
                ui.colored_label(theme::DANGER, &dialog.problem);
            }
            ui.label(theme::caption(
                "You can also drop videos or a folder anywhere in the window.",
            ));
        });

    let pressed_escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    if !open || pressed_escape || clicked_outside {
        return Some(AddDialogChoice::Close);
    }
    choice
}

/// The first screen, before any video is added: what the app does and the
/// three steps of using it. Returns true when the person pressed its button.
fn show_welcome(ctx: &egui::Context) -> bool {
    let mut add_videos = false;
    show_centered_card(ctx, |ui| {
        ui.label(theme::semibold("Cut a long video into its scenes", 20.0));
        ui.label(theme::muted(DROP_EXPLANATION));
        ui.add_space(4.0);
        add_videos = ui
            .add(theme::primary_button("Add videos"))
            .on_hover_text("Ctrl+O")
            .clicked();
        ui.label(theme::caption(DROP_PROMPT));
        ui.add_space(10.0);
        // The text column wraps only when told how wide it may be, and that
        // has to be measured out here: inside the grid no width is known yet.
        let room_for_text = ui.available_width() - STEP_NUMBER_ROOM;
        egui::Grid::new("welcome_steps")
            .num_columns(2)
            .min_col_width(0.0)
            .spacing([12.0, 14.0])
            .show(ui, |ui| {
                for (number, (step, explanation)) in WELCOME_STEPS.iter().enumerate() {
                    theme::pill(
                        ui,
                        theme::semibold((number + 1).to_string(), 13.0).color(egui::Color32::WHITE),
                        theme::PRIMARY,
                    );
                    ui.vertical(|ui| {
                        ui.set_max_width(room_for_text);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(theme::semibold(*step, 14.0));
                        ui.add(egui::Label::new(theme::muted(*explanation)).wrap());
                    });
                    ui.end_row();
                }
            });
    });
    add_videos
}

fn show_message(ctx: &egui::Context, title: &str, text: &str) {
    show_centered_card(ctx, |ui| {
        ui.add(egui::Label::new(theme::semibold(title, 16.0)).truncate());
        ui.label(theme::muted(text));
    });
}

/// A video that was skipped, with the way back.
fn show_skipped(ctx: &egui::Context, name: &str) -> Option<JobAction> {
    let mut action = None;
    show_centered_card(ctx, |ui| {
        ui.add(egui::Label::new(theme::semibold(name, 16.0)).truncate());
        ui.label(theme::muted(
            "Skipped. Nothing was written and the video was not changed.",
        ));
        if ui.button("Review it after all").clicked() {
            action = Some(JobAction::ReviewAfterAll);
        }
    });
    action
}

/// A video something went wrong with: what happened in plain words, the
/// technical reason underneath, and a way to try again.
fn show_failed(ctx: &egui::Context, name: &str, reason: &str) -> Option<JobAction> {
    let (plain_words, details) = reason.split_once("\n\nDetails: ").unwrap_or((reason, ""));
    let mut action = None;
    show_centered_card(ctx, |ui| {
        ui.label(theme::semibold("This video could not be finished", 16.0).color(theme::DANGER));
        ui.add(egui::Label::new(theme::semibold(name, 14.0)).truncate());
        ui.label(plain_words);
        if !details.is_empty() {
            show_details(ui, details);
        }
        if ui.button("Try again").clicked() {
            action = Some(JobAction::TryAgain);
        }
    });
    action
}

/// The result of a split, and what to do next.
fn show_finished(ctx: &egui::Context, name: &str, report: &ExportReport) -> Option<JobAction> {
    let mut action = None;
    show_centered_card(ctx, |ui| {
        let (verdict, verdict_color) = if report.is_clean() {
            ("Done", theme::SUCCESS)
        } else {
            (
                "The files were written, but check the note below",
                theme::DANGER,
            )
        };
        ui.label(theme::semibold(verdict, 20.0).color(verdict_color));
        ui.add(egui::Label::new(theme::semibold(name, 14.0)).truncate());
        ui.add(egui::Label::new(report.summary()).wrap());
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::primary_button("Open folder")).clicked() {
                action = Some(JobAction::OpenFolder);
            }
            let add_more = egui::Button::new("Add more videos")
                .min_size(egui::vec2(0.0, theme::PRIMARY_BUTTON_HEIGHT));
            if ui.add(add_more).clicked() {
                action = Some(JobAction::AddMoreVideos);
            }
        });
    });
    action
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
