//! The window: a queue of videos on the left, the selected video on the
//! right. Videos are scanned in the background, one after another, so the
//! next one is ready by the time the current one has been reviewed.
//!
//! The videos and their stages live in `queue`, the review of a video in
//! `review_session`. Everything drawn here only reads them and returns what
//! the person asked for as an `AppRequest`. `SceneSplitterApp::apply` is the
//! one place that acts on those. The sidebar's Settings section is the one
//! exception: it edits the settings it is handed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use eframe::egui;

use crate::cache::{self, CacheUse, SourcedDetection};
use crate::detect::{ScanKind, ScanMode, ScanProgress, ScanStage};
use crate::export::{self, ExportReport, OriginalPolicy, SplitMethod, TRASH_NAME};
use crate::keyboard::{self, KeyboardOwner};
use crate::library;
use crate::player::{Player, PlayerEvent};
use crate::queue::{Failure, Job, JobId, JobStatus, Queue, Refused, ScanOrder};
use crate::review::{file_name, ReviewScreen};
use crate::review_session::{counted, EarlierSceneFiles, FollowUp, ReviewRequest, ReviewSession};
use crate::settings::{LoadedSettings, RecyclingQuestion, Settings, SettingsChange};
use crate::settings_controls;
use crate::split::{self, SceneLocation};
use crate::theme;
use crate::tools::{self, Availability, DownloadProgress};

const WINDOW_TITLE: &str = "Scene Splitter";
const WINDOW_SIZE: [f32; 2] = [1320.0, 900.0];
/// Below this the review screen cannot show its buttons and a scene card.
const MIN_WINDOW_SIZE: [f32; 2] = [900.0, 600.0];
const QUEUE_PANEL_WIDTH: f32 = 270.0;
const QUEUE_PANEL_WIDTHS: std::ops::RangeInclusive<f32> = 230.0..=380.0;
const MESSAGE_CARD_WIDTH: f32 = 560.0;
/// Room kept on both sides of a card or dialog, for its margin and shadow.
const CARD_SIDE_ROOM: f32 = 48.0;
const DETAILS_MAX_HEIGHT: f32 = 140.0;
/// The width of the numbered pill and its gap on the welcome card.
const STEP_NUMBER_ROOM: f32 = 48.0;
const JOB_CARD_ROUNDING: f32 = 10.0;
const STATUS_DOT_SIZE: f32 = 8.0;
const ADD_DIALOG_WIDTH: f32 = 440.0;
const RECYCLING_DIALOG_WIDTH: f32 = 480.0;
/// The bar under the status of the video being scanned, in the sidebar.
const SIDEBAR_SCAN_BAR_HEIGHT: f32 = 6.0;
/// The bar on the page of the video being scanned.
const SCAN_BAR_HEIGHT: f32 = 10.0;
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
/// The ranges the settings section offers.
const SHORTEST_BLACK_SECONDS: std::ops::RangeInclusive<f64> = 0.05..=10.0;
const BLACK_SHARE_PERCENT: std::ops::RangeInclusive<f64> = 50.0..=100.0;
/// Problems are few and each is a line of text. Should something go wrong
/// over and over, only the newest are kept, so the sidebar stays usable.
const MOST_PROBLEMS_KEPT: usize = 5;
const DROP_EXPLANATION: &str =
    "Scene Splitter finds the black screens between scenes and cuts the video there, \
     without re-encoding it.";

/// Opens the window and blocks until it is closed. With `first_path` that
/// video, or every video in that folder, is queued at once, to be scanned
/// as `first_scan` says.
pub fn run(
    loaded: LoadedSettings,
    first_path: Option<PathBuf>,
    first_scan: ScanKind,
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
                app.queue.scan_waiting_videos_as(first_scan);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow!("could not open the window: {error}"))
}

/// What the person asked for somewhere in the window during one frame.
enum AppRequest {
    OpenAddDialog,
    CloseAddDialog,
    /// Add what was chosen or pasted in the add dialog.
    AddFromDialog(Vec<PathBuf>),
    /// Add what was dropped on the window.
    AddDropped(Vec<PathBuf>),
    /// Show this video beside the list.
    ShowJob(JobId),
    ReviewAfterAll(JobId),
    TryAgain(JobId),
    /// Take up the review of a video whose split failed, as it was.
    BackToReview(JobId),
    /// The person has read the problems listed in the sidebar.
    DismissProblems,
    /// Show this folder in the system's file manager.
    OpenFolder(PathBuf),
    /// Try once more to download the video tools.
    GetToolsAgain,
    /// What the person asked for on the review screen of this video, in the
    /// order it is to be applied.
    Review {
        job: JobId,
        requests: Vec<ReviewRequest>,
    },
    ClosePlayer,
    /// Split the video the player is playing, where it is playing now.
    SplitAtPlayhead {
        job: JobId,
        time: f64,
    },
    /// What the person said when asked, once, about recycling originals.
    AnswerRecyclingQuestion(RecyclingAnswer),
}

/// The answers to the question that is asked before the first split that
/// would move an original to the Recycle Bin or Trash.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RecyclingAnswer {
    MoveOriginal,
    KeepOriginal,
    /// Not now. Nothing is split, and the question comes back next time.
    Cancel,
}

/// The settings to split with after `answer`, or `None` when the person
/// backed out, so nothing is split and nothing is remembered.
fn settings_after_recycling_answer(
    settings: Settings,
    answer: RecyclingAnswer,
) -> Option<Settings> {
    let original_policy = match answer {
        RecyclingAnswer::MoveOriginal => OriginalPolicy::RecycleOnceVerified,
        RecyclingAnswer::KeepOriginal => OriginalPolicy::Keep,
        RecyclingAnswer::Cancel => return None,
    };
    let mut answered = settings;
    answered.export.original_policy = original_policy;
    answered.recycling_question = RecyclingQuestion::Answered;
    Some(answered)
}

/// The result of background work on one job.
struct JobUpdate {
    job: JobId,
    event: JobEvent,
}

enum JobEvent {
    /// The scan is still running and has come this far.
    ScanProgressed(ScanProgress),
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
    queue: Queue,
    /// What the review screen of each video in review has drawn and loaded.
    /// The reviews themselves are in the queue.
    review_screens: HashMap<JobId, ReviewScreen>,
    updates_sender: Sender<JobUpdate>,
    updates: Receiver<JobUpdate>,
    add_dialog: AddDialog,
    add_feedback: String,
    player: Option<ActivePlayer>,
    /// The video whose split waits for the person to say, this once, what
    /// happens to originals.
    recycling_question_for: Option<JobId>,
    /// Things that went wrong outside any one video, newest last, shown in
    /// the sidebar until the person dismisses them.
    problems: Vec<String>,
    /// Whether a text box had the keyboard when the last frame ended.
    text_box_had_the_keyboard: bool,
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

fn counted_already_listed(video_count: usize) -> String {
    if video_count == 1 {
        "That video is already in the list.".to_owned()
    } else {
        "Those videos are already in the list.".to_owned()
    }
}

/// A scan reports its progress a few hundred times, from several threads.
/// This lets one report through for each whole percent, the smallest step
/// the window shows, so the scan does not fill the channel to the window
/// with reports nobody would see.
#[derive(Default)]
struct WholePercentSteps {
    /// One more than the highest percentage let through so far, or zero when
    /// nothing was. A report that arrives late, behind a later one, is below
    /// this and is dropped.
    percent_to_reach: AtomicU32,
}

impl WholePercentSteps {
    /// Whether a report of `fraction` is news worth sending.
    fn admits(&self, fraction: f32) -> bool {
        let percent = percent_done(fraction);
        let percent_to_reach = self
            .percent_to_reach
            .fetch_max(percent + 1, Ordering::Relaxed);
        percent >= percent_to_reach
    }
}

/// A share from 0.0 to 1.0 as a whole percentage, rounded down, so that
/// 100% is only said of a scan that is done.
fn percent_done(fraction: f32) -> u32 {
    // Not a number at all counts as nothing done.
    (fraction.clamp(0.0, 1.0) * PERCENT as f32) as u32
}

fn scan_stage_in_words(stage: ScanStage) -> &'static str {
    match stage {
        ScanStage::BlackScreens => "Looking for black screens.",
        ScanStage::HardCuts => "Looking at every picture for sudden changes.",
    }
}

/// Whether a scan the person asked to repeat may be answered from the store.
fn stored_scan_use_for_rescan(scan_kind: ScanKind) -> CacheUse {
    match scan_kind {
        // "Scan again" means look at the video again, not at what the last
        // look left behind.
        ScanKind::BlackScreens => CacheUse::ScanFresh,
        // The slow part of an earlier hard-cut scan is stored, so only
        // changed settings are applied when it can be reused.
        ScanKind::HardCuts(_) => CacheUse::ReuseStoredScan,
    }
}

/// The player, and the job whose video it is playing.
struct ActivePlayer {
    job: JobId,
    player: Player,
}

impl eframe::App for SceneSplitterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.run_frame(ctx);
    }
}

/// What the person asked for without clicking anything that is drawn: the
/// shortcut for adding videos, and files dropped on the window.
fn requests_from_keys_and_drops(ctx: &egui::Context) -> Vec<AppRequest> {
    let mut requests = Vec::new();
    let pressed_ctrl_o =
        ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::O));
    if pressed_ctrl_o {
        requests.push(AppRequest::OpenAddDialog);
    }
    let dropped_paths: Vec<PathBuf> = ctx.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .filter_map(|file| file.path.clone())
            .collect()
    });
    if !dropped_paths.is_empty() {
        requests.push(AppRequest::AddDropped(dropped_paths));
    }
    requests
}

impl SceneSplitterApp {
    fn new(loaded: LoadedSettings) -> Self {
        let (updates_sender, updates) = mpsc::channel();
        Self {
            settings: loaded.settings,
            saved_settings: loaded.settings,
            settings_problem: loaded.problem.unwrap_or_default(),
            setup: Setup::Ready,
            queue: Queue::default(),
            review_screens: HashMap::new(),
            updates_sender,
            updates,
            add_dialog: AddDialog::default(),
            add_feedback: String::new(),
            player: None,
            recycling_question_for: None,
            problems: Vec::new(),
            text_box_had_the_keyboard: false,
        }
    }

    /// One frame of the window: take in what happened in the background,
    /// draw everything, and do what the person asked for.
    fn run_frame(&mut self, ctx: &egui::Context) {
        self.follow_setup();
        self.apply_job_updates();
        self.tell_the_review_where_new_splits_land();
        self.apply(ctx, requests_from_keys_and_drops(ctx));
        self.start_next_scan(ctx);
        self.close_player_unless_its_video_is_in_review();
        self.forget_review_screens_no_longer_shown();
        self.drop_the_recycling_question_unless_its_video_is_in_review();

        let scene_location_before = self.settings.export.scene_location;
        let keyboard = self.keyboard_owner();
        self.leave_escape_to_the_text_box(ctx, keyboard);
        // The player is in front of the review screen but drawn after it,
        // so it is offered the keys, Escape among them, before that is drawn.
        if let Some(active) = &mut self.player {
            active.player.take_keys(ctx, keyboard);
        }

        let mut requests = Vec::new();
        egui::SidePanel::left("queue")
            .frame(theme::panel(theme::SIDEBAR))
            .default_width(QUEUE_PANEL_WIDTH)
            .width_range(QUEUE_PANEL_WIDTHS)
            .show(ctx, |ui| {
                // One scroll area for the whole panel, so opening the
                // settings can never push the video list out of reach.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        requests.extend(show_queue(
                            ui,
                            &self.queue,
                            &self.add_feedback,
                            &self.problems,
                            &mut self.settings,
                            &self.settings_problem,
                        ));
                    });
            });
        requests.extend(self.show_current_job(ctx, keyboard));
        if self.add_dialog.open {
            requests.extend(show_add_dialog(ctx, &mut self.add_dialog));
        }
        requests.extend(self.show_player(ctx));
        requests.extend(self.show_recycling_question(ctx));
        self.apply(ctx, requests);

        self.forget_export_obstacles_if_scenes_go_elsewhere(scene_location_before);
        self.text_box_had_the_keyboard = ctx.wants_keyboard_input();
        self.save_settings_once_changed(ctx);
    }

    /// A split the person adds lands on the nearest keyframe or on the very
    /// picture, as the setting says. Told to the review on show before
    /// anything is drawn or asked of it, so a change of the setting holds
    /// from the next frame on.
    fn tell_the_review_where_new_splits_land(&mut self) {
        let precision = self.settings.export.cut_precision;
        let review_on_show = self
            .queue
            .current()
            .and_then(|job| self.queue.review_mut(job));
        if let Some(review) = review_on_show {
            review.place_new_splits(precision);
        }
    }

    /// A dialog in front of everything has the keyboard to itself.
    fn keyboard_owner(&self) -> KeyboardOwner {
        if self.add_dialog.open || self.recycling_question_for.is_some() {
            KeyboardOwner::Dialog
        } else {
            KeyboardOwner::Screens
        }
    }

    /// Escape pressed while typing means "I am done with this box" and
    /// nothing more. egui takes the keyboard away from the box at the very
    /// start of the frame in which Escape is pressed, so by now nothing looks
    /// as if it were being typed into, and the player and the review screen
    /// would read the same press as theirs. Hence the look back at how the
    /// last frame ended.
    ///
    /// A dialog is different: Escape closes it, also from its text box.
    fn leave_escape_to_the_text_box(&self, ctx: &egui::Context, keyboard: KeyboardOwner) {
        if keyboard == KeyboardOwner::Screens && self.text_box_had_the_keyboard {
            // Taken and dropped: leaving the box was all it was for.
            let _left_the_text_box = keyboard::take_escape(ctx);
        }
    }

    /// A warning about the scene files of an earlier split names the folder
    /// the scenes go to. Wherever in the window the person changes that
    /// folder, the sidebar or a review screen, this is the one place that
    /// notices, so no warning about the old folder stays on any review.
    fn forget_export_obstacles_if_scenes_go_elsewhere(&mut self, location_before: SceneLocation) {
        if self.settings.export.scene_location != location_before {
            self.queue.forget_export_obstacles();
        }
    }

    /// Keeps a problem that belongs to no one video for the sidebar to show.
    fn note_problem(&mut self, problem: String) {
        self.problems.push(problem);
        let too_many = self.problems.len().saturating_sub(MOST_PROBLEMS_KEPT);
        self.problems.drain(..too_many);
    }

    /// The queue refuses a change that makes no sense for a video at its
    /// stage. Nothing here asks for one, so a refusal is a bug: it stops a
    /// debug build, and in a release build the video stays as it is and the
    /// person is told in the sidebar.
    fn note_refusal(&mut self, outcome: Result<(), Refused>) {
        let Err(refused) = outcome else {
            return;
        };
        debug_assert!(false, "the queue refused a change: {refused}");
        self.note_problem(format!(
            "Something unexpected happened, and a video was left as it is. ({refused})"
        ));
    }

    /// Does everything the person asked for during one frame. This is the
    /// only place where what is drawn leads to a change.
    fn apply(&mut self, ctx: &egui::Context, requests: Vec<AppRequest>) {
        if requests.is_empty() {
            return;
        }
        for request in requests {
            self.apply_request(ctx, request);
        }
        // What changed is drawn in the next frame, which must not wait for
        // the person to move the mouse.
        ctx.request_repaint();
    }

    fn apply_request(&mut self, ctx: &egui::Context, request: AppRequest) {
        match request {
            AppRequest::OpenAddDialog => {
                self.add_dialog.open = true;
                self.add_dialog.focus_path_box = true;
            }
            AppRequest::CloseAddDialog => self.add_dialog = AddDialog::default(),
            AppRequest::AddFromDialog(paths) => self.add_from_dialog(paths),
            AppRequest::AddDropped(paths) => {
                for path in paths {
                    if let Err(problem) = self.add_path(path) {
                        self.add_feedback = problem;
                    }
                }
            }
            AppRequest::ShowJob(job) => {
                let shown = self.queue.show(job);
                self.note_refusal(shown);
            }
            AppRequest::ReviewAfterAll(job) => {
                let taken_up = self.queue.review_after_all(job);
                self.note_refusal(taken_up);
            }
            AppRequest::TryAgain(job) => {
                let queued = self.queue.try_again(job);
                self.note_refusal(queued);
            }
            AppRequest::BackToReview(job) => {
                let taken_up = self.queue.back_to_review(job);
                self.note_refusal(taken_up);
            }
            AppRequest::DismissProblems => self.problems.clear(),
            AppRequest::OpenFolder(folder) => {
                if let Err(problem) = open_in_file_manager(&folder) {
                    self.note_problem(problem);
                }
            }
            AppRequest::GetToolsAgain => self.get_tools_if_missing(ctx),
            AppRequest::Review { job, requests } => self.apply_to_review(ctx, job, requests),
            AppRequest::ClosePlayer => self.player = None,
            AppRequest::SplitAtPlayhead { job, time } => self.split_at_playhead(job, time),
            AppRequest::AnswerRecyclingQuestion(answer) => {
                self.answer_recycling_question(ctx, answer);
            }
        }
    }

    /// Starts the split, unless it would move the original to the Recycle
    /// Bin or Trash and the person has never been told what that means. Then
    /// the split waits for their answer.
    fn split_once_recycling_is_settled(&mut self, ctx: &egui::Context, job: JobId) {
        if !self.settings.must_ask_before_recycling() {
            return self.start_export(ctx, job);
        }
        // The person is done with the player, and its keys must not go on
        // changing the scenes behind the question.
        self.player = None;
        self.recycling_question_for = Some(job);
    }

    fn answer_recycling_question(&mut self, ctx: &egui::Context, answer: RecyclingAnswer) {
        let Some(job) = self.recycling_question_for.take() else {
            return;
        };
        let Some(answered) = settings_after_recycling_answer(self.settings, answer) else {
            return;
        };
        self.settings = answered;
        self.start_export(ctx, job);
    }

    /// The question belongs to one split. If its video is no longer the one
    /// under review, there is nothing left to ask about.
    fn drop_the_recycling_question_unless_its_video_is_in_review(&mut self) {
        let still_in_review = self.recycling_question_for.is_some_and(|job| {
            self.queue.current() == Some(job) && self.queue.review(job).is_some()
        });
        if !still_in_review {
            self.recycling_question_for = None;
        }
    }

    fn show_recycling_question(&self, ctx: &egui::Context) -> Option<AppRequest> {
        let job = self.queue.job(self.recycling_question_for?)?;
        show_recycling_question(ctx, &file_name(job.video()))
            .map(AppRequest::AnswerRecyclingQuestion)
    }

    /// The dialog stays open on a mistake, with the reason in it, so a
    /// mistyped path can be corrected in place.
    fn add_from_dialog(&mut self, paths: Vec<PathBuf>) {
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

    fn apply_to_review(&mut self, ctx: &egui::Context, job: JobId, requests: Vec<ReviewRequest>) {
        // The video may have left its review earlier in this same frame.
        let Some(session) = self.queue.review_mut(job) else {
            return;
        };
        let video = Arc::clone(session.video());
        let scene_location = self.settings.export.scene_location;
        let look_for_files = || look_for_earlier_scene_files(&video, scene_location);
        let Some(follow_up) = session.apply_frame(requests, &look_for_files) else {
            return;
        };
        match follow_up {
            FollowUp::Export => self.split_once_recycling_is_settled(ctx, job),
            FollowUp::Skip => {
                let skipped = self.queue.skip(job);
                self.note_refusal(skipped);
            }
            FollowUp::Rescan(scan_kind) => {
                let cache_use = stored_scan_use_for_rescan(scan_kind);
                let queued = self.queue.rescan(job, scan_kind, cache_use);
                self.note_refusal(queued);
            }
            FollowUp::PlayFrom(time) => self.play_from(ctx, job, time),
            FollowUp::ChangeSettings(change) => self.change_settings(change),
        }
    }

    /// Takes over a change of settings made on a review screen. It is saved
    /// with every other change once the frame is over.
    fn change_settings(&mut self, change: SettingsChange) {
        self.settings = self.settings.changed_by(change);
    }

    fn split_at_playhead(&mut self, job: JobId, time: f64) {
        // The player only stays open while its video is in review.
        let Some(session) = self.queue.review_mut(job) else {
            return;
        };
        let feedback = session.split_near(time);
        if let Some(active) = &mut self.player {
            active.player.show_split_feedback(feedback);
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

    /// Queues a video, or every video directly inside a folder, and says
    /// what happened in the sidebar. Fails with a sentence for the person
    /// when the path leads to nothing that can be added.
    fn add_path(&mut self, path: PathBuf) -> Result<(), String> {
        let videos = library::videos_at(&path).map_err(|error| format!("{error:#}"))?;
        if videos.is_empty() {
            return Err(format!("No videos were found in {}.", path.display()));
        }

        let added = self.queue.add_all(&videos);
        self.add_feedback = if added.added == 0 {
            counted_already_listed(videos.len())
        } else {
            format!(
                "Added {}. Scanning starts by itself.",
                counted(added.added, "video")
            )
        };
        Ok(())
    }

    fn start_next_scan(&mut self, ctx: &egui::Context) {
        // Scanning needs ffmpeg, so videos wait in the queue until it is here.
        if !matches!(self.setup, Setup::Ready) {
            return;
        }
        let Some(order) = self.queue.start_next_scan() else {
            return;
        };
        let ScanOrder {
            job,
            video,
            cache_use,
            depth,
        } = order;
        let settings = self.settings.scan;
        let updates_sender = self.updates_sender.clone();
        let window = ctx.clone();
        self.run_in_background(ctx, job, move || {
            let steps = WholePercentSteps::default();
            let report_progress = |progress: ScanProgress| {
                if !steps.admits(progress.fraction) {
                    return;
                }
                let update = JobUpdate {
                    job,
                    event: JobEvent::ScanProgressed(progress),
                };
                // The send only fails once the window is gone.
                if updates_sender.send(update).is_ok() {
                    window.request_repaint();
                }
            };
            // What the scan could not decode travels with its result into
            // the review, which tells the person.
            JobEvent::Scanned(cache::detect_with_cache(
                &video,
                &settings,
                cache_use,
                depth,
                &report_progress,
            ))
        });
    }

    /// The player belongs to the video under review. Switching to another
    /// video, or sending this one off to be split, closes it.
    fn close_player_unless_its_video_is_in_review(&mut self) {
        let still_in_review = self.player.as_ref().is_some_and(|active| {
            self.queue.current() == Some(active.job) && self.queue.review(active.job).is_some()
        });
        if !still_in_review {
            self.player = None;
        }
    }

    /// A review screen is kept for as long as its review is: while the video
    /// is in review, skipped with the review put aside, or being split with
    /// the review held until that has worked. Its previews are dropped with
    /// it.
    fn forget_review_screens_no_longer_shown(&mut self) {
        let queue = &self.queue;
        self.review_screens.retain(|&job, _| {
            queue
                .job(job)
                .is_some_and(|job| job.status().kept_review().is_some())
        });
    }

    fn show_player(&mut self, ctx: &egui::Context) -> Option<AppRequest> {
        let active = self.player.as_mut()?;
        let job = active.job;
        let (scenes, split_would_land_at) = match self.queue.review(job) {
            Some(review) => (
                review.scenes(),
                review.split_time_near(active.player.position()),
            ),
            None => (Vec::new(), None),
        };
        match active.player.show(ctx, &scenes, split_would_land_at) {
            PlayerEvent::StillOpen => None,
            PlayerEvent::Closed => Some(AppRequest::ClosePlayer),
            PlayerEvent::SplitRequested(time) => Some(AppRequest::SplitAtPlayhead { job, time }),
        }
    }

    fn start_export(&mut self, ctx: &egui::Context, job: JobId) {
        // The player keeps the video file open, which would stop the
        // original from being recycled after the split.
        self.player = None;

        let Some(video) = self.queue.job(job).map(|job| Arc::clone(job.video())) else {
            return self.note_refusal(Err(Refused::NoSuchJob(job)));
        };
        let method = self.settings.export.split_method();
        let out_dir = match split::scene_dir(&video, self.settings.export.scene_location) {
            Ok(out_dir) => out_dir,
            Err(error) => {
                let failure = Failure::in_plain_words(format!("{error:#}"));
                let failed = self.queue.failed(job, failure);
                return self.note_refusal(failed);
            }
        };
        let order = match self.queue.start_export(job) {
            Ok(order) => order,
            Err(refused) => return self.note_refusal(Err(refused)),
        };
        self.run_in_background(ctx, job, move || {
            JobEvent::Exported(export::export_scenes_as(
                &order.video,
                &order.cuts,
                &out_dir,
                method,
            ))
        });
    }

    fn run_in_background(
        &self,
        ctx: &egui::Context,
        job: JobId,
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

    /// Tells the queue how far the background work has come, and how the
    /// work that has ended went.
    fn apply_job_updates(&mut self) {
        let updates: Vec<JobUpdate> = self.updates.try_iter().collect();
        for JobUpdate { job, event } in updates {
            let told = match event {
                JobEvent::ScanProgressed(progress) => self.queue.scan_progressed(job, progress),
                JobEvent::Scanned(Ok(sourced)) => self.queue.scan_finished(job, sourced),
                JobEvent::Scanned(Err(error)) => {
                    self.queue.failed(job, Failure::from_scan_error(&error))
                }
                JobEvent::Exported(Ok(report)) => self.queue.finished(job, report),
                JobEvent::Exported(Err(error)) => {
                    self.queue.failed(job, Failure::from_split_error(&error))
                }
                JobEvent::Crashed => self.queue.crashed(job),
            };
            self.note_refusal(told);
        }
    }

    /// Plays the video under review from `time`, in the player that is
    /// already open if there is one.
    fn play_from(&mut self, ctx: &egui::Context, job: JobId, time: f64) {
        if let Some(active) = &mut self.player {
            active.player.play_at(time, ctx);
            return;
        }
        let Some(review) = self.queue.review(job) else {
            return;
        };
        self.player = Some(ActivePlayer {
            job,
            player: Player::play_from(
                Arc::clone(review.video()),
                review.video_end(),
                review.frame_seconds(),
                time,
                ctx,
            ),
        });
    }

    /// Draws the video the person picked in the list, as far along as it is.
    fn show_current_job(
        &mut self,
        ctx: &egui::Context,
        keyboard: KeyboardOwner,
    ) -> Vec<AppRequest> {
        if !matches!(self.setup, Setup::Ready) {
            return show_setup(ctx, &self.setup).into_iter().collect();
        }
        let Some(job) = self.queue.current_job() else {
            return show_welcome(ctx).into_iter().collect();
        };
        let id = job.id();
        let name = file_name(job.video());

        let request = match job.status() {
            JobStatus::Reviewing(session) => {
                let playhead = self
                    .player
                    .as_ref()
                    .filter(|active| active.job == id)
                    .map(|active| active.player.position());
                let screen = self.review_screens.entry(id).or_default();
                let requests = screen.show(ctx, session, &self.settings, playhead, keyboard);
                // While the question about recycling is open, the review
                // behind it is not to be changed, not even with its keys.
                let may_change_the_review = self.recycling_question_for.is_none();
                (may_change_the_review && !requests.is_empty())
                    .then_some(AppRequest::Review { job: id, requests })
            }
            JobStatus::Waiting => {
                show_message(ctx, &name, "Waiting. Another video is being scanned first.");
                None
            }
            JobStatus::Scanning(progress) => {
                let what = match job.scan_kind() {
                    ScanKind::BlackScreens => format!("Scanning {name} for scene changes"),
                    ScanKind::HardCuts(_) => format!(
                        "Looking at every picture of {name} for sudden changes. \
                         This takes much longer than the first scan."
                    ),
                };
                match progress {
                    Some(progress) => show_scan_progress(ctx, &what, *progress),
                    // The scan has not said yet how much there is to do.
                    None => show_busy(ctx, &what),
                }
                None
            }
            JobStatus::Exporting(_) => {
                show_busy(ctx, &format!("Writing and checking the scenes of {name}"));
                None
            }
            JobStatus::Finished(report) => show_finished(ctx, &name, report),
            JobStatus::Skipped(_) => show_skipped(ctx, id, &name),
            JobStatus::Failed { failure, review } => {
                show_failed(ctx, id, &name, failure, review.as_deref())
            }
        };
        request.into_iter().collect()
    }
}

/// Looks where the scenes of this video would go now, and counts the scene
/// files an earlier split of it left there.
fn look_for_earlier_scene_files(
    video: &Path,
    scene_location: SceneLocation,
) -> Result<EarlierSceneFiles, String> {
    let scene_dir =
        split::scene_dir(video, scene_location).map_err(|error| format!("{error:#}"))?;
    let file_count = split::scene_files_from_earlier_splits(video, &scene_dir)
        .map_err(|error| format!("{error:#}"))?
        .len();
    Ok(EarlierSceneFiles {
        scene_dir,
        file_count,
    })
}

/// How far along a video is, as the sidebar says it.
fn status_label(status: &JobStatus) -> String {
    match status {
        JobStatus::Waiting => "Waiting".to_owned(),
        JobStatus::Scanning(None) => "Scanning".to_owned(),
        JobStatus::Scanning(Some(progress)) => {
            format!("Scanning, {}%", percent_done(progress.fraction))
        }
        JobStatus::Reviewing(review) => {
            format!(
                "Ready to review, {}",
                counted(review.scene_count(), "scene")
            )
        }
        JobStatus::Exporting(_) => "Writing and checking".to_owned(),
        JobStatus::Finished(report) if report.is_clean() => "Done, checked".to_owned(),
        JobStatus::Finished(_) => "Done, check the result".to_owned(),
        JobStatus::Skipped(_) => "Skipped".to_owned(),
        JobStatus::Failed { .. } => "Failed".to_owned(),
    }
}

fn status_color(status: &JobStatus) -> egui::Color32 {
    match status {
        JobStatus::Waiting | JobStatus::Skipped(_) => theme::MUTED_TEXT,
        JobStatus::Scanning(_) | JobStatus::Exporting(_) => theme::ACCENT,
        JobStatus::Reviewing(_) => theme::PRIMARY,
        JobStatus::Finished(report) if report.is_clean() => theme::SUCCESS,
        JobStatus::Finished(_) | JobStatus::Failed { .. } => theme::DANGER,
    }
}

/// The sidebar: the way to add videos, the videos with their progress, and
/// the settings.
fn show_queue(
    ui: &mut egui::Ui,
    queue: &Queue,
    add_feedback: &str,
    problems: &[String],
    settings: &mut Settings,
    settings_problem: &str,
) -> Vec<AppRequest> {
    let mut requests = Vec::new();
    ui.label(theme::semibold("Scene Splitter", 20.0).color(theme::PRIMARY));
    let add_button = ui
        .add(theme::primary_button("Add videos"))
        .on_hover_text("Choose videos or a folder, or paste a path (Ctrl+O).");
    if add_button.clicked() {
        requests.push(AppRequest::OpenAddDialog);
    }
    ui.label(theme::caption("Or drop videos or a folder anywhere."));
    if !add_feedback.is_empty() {
        ui.label(theme::caption(add_feedback));
    }

    ui.add_space(4.0);
    ui.label(theme::semibold("Videos", 14.0));
    if queue.is_empty() {
        ui.label(theme::caption(
            "Videos you add are listed here with their progress. Click one to open it.",
        ));
    }
    for job in queue.jobs() {
        let is_current = queue.current() == Some(job.id());
        requests.extend(show_job_card(ui, job, is_current));
    }
    requests.extend(show_problems(ui, problems));

    ui.add_space(8.0);
    egui::CollapsingHeader::new(theme::semibold("Settings", 14.0))
        .show(ui, |ui| show_settings(ui, settings, settings_problem));
    requests
}

/// What went wrong outside any one video, with a way to copy it and a way to
/// put it out of sight. Draws nothing when nothing went wrong.
fn show_problems(ui: &mut egui::Ui, problems: &[String]) -> Option<AppRequest> {
    if problems.is_empty() {
        return None;
    }
    let mut request = None;
    ui.add_space(4.0);
    ui.label(theme::semibold("Something went wrong", 14.0).color(theme::DANGER));
    for problem in problems {
        ui.add(theme::selectable(theme::caption(problem)).wrap());
    }
    ui.horizontal_wrapped(|ui| {
        theme::copy_details_button(ui, &problems.join("\n"));
        if ui.button("Dismiss").clicked() {
            request = Some(AppRequest::DismissProblems);
        }
    });
    request
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
        ui.add(theme::selectable(
            egui::RichText::new(problem).color(theme::DANGER),
        ));
        theme::copy_details_button(ui, problem);
    }

    ui.label(theme::semibold("When a video is split", 13.0));
    settings_controls::show_cut_precision_checkbox(ui, &mut settings.export.cut_precision);
    // An exact split keeps the original whatever this box says, so the box
    // is not offered as if it had a say.
    let recycling_has_a_say = matches!(
        settings.export.split_method(),
        SplitMethod::CopyAtKeyframes { .. }
    );
    ui.add_enabled_ui(recycling_has_a_say, |ui| {
        settings_controls::show_original_policy_checkbox(
            ui,
            &mut settings.export.original_policy,
            format!("Move the original to the {TRASH_NAME} after the check"),
        )
        .on_hover_text(
            "The new files are compared with the original first. If anything is missing or different, the original stays.",
        )
        .on_disabled_hover_text(settings_controls::ORIGINAL_KEPT_WITH_EXACT_CUTS);
    });
    settings_controls::show_scene_location_checkbox(ui, &mut settings.export.scene_location);
    ui.checkbox(&mut settings.show_tips, "Show tips on the review screen");

    ui.add_space(4.0);
    ui.label(theme::semibold("Looking for missed scenes", 13.0));
    ui.horizontal_wrapped(|ui| {
        settings_controls::show_hard_cut_controls(
            ui,
            &mut settings.hard_cuts,
            "settings_hard_cut_sensitivity",
        );
    });

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
    let restore = ui.button("Restore default settings").on_hover_text(
        "Puts every setting here back to how it was at first, except what happens to originals.",
    );
    if restore.clicked() {
        *settings = settings.restored_to_defaults();
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

/// One video in the sidebar. Clicking it asks for it to be shown.
fn show_job_card(ui: &mut egui::Ui, job: &Job, is_current: bool) -> Option<AppRequest> {
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
            ui.add(egui::Label::new(theme::semibold(file_name(job.video()), 14.0)).truncate());
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(
                    egui::vec2(STATUS_DOT_SIZE, STATUS_DOT_SIZE),
                    egui::Sense::hover(),
                );
                ui.painter().circle_filled(
                    dot.center(),
                    STATUS_DOT_SIZE / 2.0,
                    status_color(job.status()),
                );
                ui.label(theme::caption(status_label(job.status())));
            });
            if let JobStatus::Scanning(Some(progress)) = job.status() {
                theme::progress_bar(ui, progress.fraction, SIDEBAR_SCAN_BAR_HEIGHT);
            }
        })
        .response;
    let clicked = ui
        .interact(
            card.rect,
            ui.id().with(("job", job.id())),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked();
    clicked.then_some(AppRequest::ShowJob(job.id()))
}

/// The first-run screen, with the way to try the download again when it
/// failed.
fn show_setup(ctx: &egui::Context, setup: &Setup) -> Option<AppRequest> {
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
    retry.then_some(AppRequest::GetToolsAgain)
}

/// The technical reason behind a message, not in the way of the plain
/// sentence above it. The text can be selected, and the button under it
/// copies all of it, including what has scrolled out of view. Returns that
/// button.
fn show_details(ui: &mut egui::Ui, details: &str) -> egui::Response {
    egui::ScrollArea::vertical()
        .max_height(DETAILS_MAX_HEIGHT)
        .show(ui, |ui| {
            ui.add(theme::selectable(theme::caption(details)));
        });
    theme::copy_details_button(ui, details)
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

/// The add dialog: the system's own file and folder pickers, and a box to
/// paste a path into. Returns what the person chose, if anything yet.
fn show_add_dialog(ctx: &egui::Context, dialog: &mut AddDialog) -> Option<AppRequest> {
    let clicked_outside = theme::show_backdrop(ctx, "add_dialog_backdrop", egui::Order::Middle);
    let mut paths_to_add: Option<Vec<PathBuf>> = None;
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
                    paths_to_add = rfd::FileDialog::new()
                        .add_filter("Videos", &library::VIDEO_EXTENSIONS)
                        .pick_files();
                }
                let choose_folder = egui::Button::new("Choose a folder")
                    .min_size(egui::vec2(0.0, theme::PRIMARY_BUTTON_HEIGHT));
                if ui.add(choose_folder).clicked() {
                    let picked = rfd::FileDialog::new().pick_folder();
                    paths_to_add = picked.map(|folder| vec![folder]);
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
                    paths_to_add = pasted.map(|path| vec![path]);
                }
            });
            if !dialog.problem.is_empty() {
                ui.colored_label(theme::DANGER, &dialog.problem);
            }
            ui.label(theme::caption(
                "You can also drop videos or a folder anywhere in the window.",
            ));
        });

    // The dialog is in front of everything, so Escape is its to take.
    let pressed_escape = keyboard::take_escape(ctx);
    if !open || pressed_escape || clicked_outside {
        return Some(AppRequest::CloseAddDialog);
    }
    paths_to_add.map(AppRequest::AddFromDialog)
}

/// Asked once, before the first split that would move an original to the
/// Recycle Bin or Trash: what will happen, and on what condition. Returns
/// the person's answer, if they gave one in this frame.
fn show_recycling_question(ctx: &egui::Context, video_name: &str) -> Option<RecyclingAnswer> {
    let clicked_outside =
        theme::show_backdrop(ctx, "recycling_question_backdrop", egui::Order::Middle);
    let mut answer = None;
    let mut open = true;
    egui::Window::new(format!("Move the original to the {TRASH_NAME}?"))
        .open(&mut open)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_width(RECYCLING_DIALOG_WIDTH.min(ctx.screen_rect().width() - CARD_SIDE_ROOM));
            ui.add(
                egui::Label::new(format!(
                    "After the split, the original video is moved to the {TRASH_NAME}:"
                ))
                .wrap(),
            );
            ui.add(egui::Label::new(theme::semibold(video_name, 14.0)).truncate())
                .on_hover_text(video_name);
            ui.add(
                egui::Label::new(
                    "That only happens if the new files pass the check: together they must \
                     contain the whole original, with nothing missing or changed. If the \
                     check does not pass, the original stays where it is.",
                )
                .wrap(),
            );
            ui.add(
                egui::Label::new(theme::caption(format!(
                    "You can restore the original from the {TRASH_NAME}. This is asked only \
                     once. You can change your choice under the name of any video, or in \
                     Settings."
                )))
                .wrap(),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let move_it = theme::primary_button(format!("Move it to the {TRASH_NAME}"));
                if ui.add(move_it).clicked() {
                    answer = Some(RecyclingAnswer::MoveOriginal);
                }
                let keep_it = egui::Button::new("Keep the original")
                    .min_size(egui::vec2(0.0, theme::PRIMARY_BUTTON_HEIGHT));
                let keep_it = ui.add(keep_it).on_hover_text(
                    "Splits the video and leaves the original where it is, this time and from now on.",
                );
                if keep_it.clicked() {
                    answer = Some(RecyclingAnswer::KeepOriginal);
                }
                let cancel = egui::Button::new("Cancel")
                    .min_size(egui::vec2(0.0, theme::PRIMARY_BUTTON_HEIGHT));
                if ui.add(cancel).on_hover_text("Does not split yet.").clicked() {
                    answer = Some(RecyclingAnswer::Cancel);
                }
            });
        });

    let pressed_escape = keyboard::take_escape(ctx);
    if !open || pressed_escape || clicked_outside {
        return Some(RecyclingAnswer::Cancel);
    }
    answer
}

/// The first screen, before any video is added: what the app does and the
/// three steps of using it, and the button that starts the first of them.
fn show_welcome(ctx: &egui::Context) -> Option<AppRequest> {
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
    add_videos.then_some(AppRequest::OpenAddDialog)
}

fn show_message(ctx: &egui::Context, title: &str, text: &str) {
    show_centered_card(ctx, |ui| {
        ui.add(egui::Label::new(theme::semibold(title, 16.0)).truncate());
        ui.label(theme::muted(text));
    });
}

/// A video that was skipped, with the way back.
fn show_skipped(ctx: &egui::Context, job: JobId, name: &str) -> Option<AppRequest> {
    let mut request = None;
    show_centered_card(ctx, |ui| {
        ui.add(egui::Label::new(theme::semibold(name, 16.0)).truncate());
        ui.label(theme::muted(
            "Skipped. Nothing was written and the video was not changed.",
        ));
        if ui.button("Review it after all").clicked() {
            request = Some(AppRequest::ReviewAfterAll(job));
        }
    });
    request
}

/// A video something went wrong with: what happened in plain words, the
/// technical reason underneath, and the ways on from here. `review` is the
/// review the video still has when it was its split that failed.
fn show_failed(
    ctx: &egui::Context,
    job: JobId,
    name: &str,
    failure: &Failure,
    review: Option<&ReviewSession>,
) -> Option<AppRequest> {
    let mut request = None;
    show_centered_card(ctx, |ui| {
        ui.label(theme::semibold("This video could not be finished", 16.0).color(theme::DANGER));
        ui.add(egui::Label::new(theme::semibold(name, 14.0)).truncate());
        ui.label(&failure.plain_words);
        if let Some(details) = &failure.details {
            show_details(ui, details);
        }
        ui.horizontal_wrapped(|ui| {
            let try_again = egui::Button::new("Try again")
                .min_size(egui::vec2(0.0, theme::PRIMARY_BUTTON_HEIGHT));
            match review {
                Some(review) => {
                    let back = ui
                        .add(theme::primary_button("Back to review"))
                        .on_hover_text(format!(
                            "Shows the {} again as you left them, so you can split once more.",
                            counted(review.scene_count(), "scene")
                        ));
                    if back.clicked() {
                        request = Some(AppRequest::BackToReview(job));
                    }
                    let scan_again = ui.add(try_again).on_hover_text(
                        "Scans the video again from the start. The merges and splits you made are not kept.",
                    );
                    if scan_again.clicked() {
                        request = Some(AppRequest::TryAgain(job));
                    }
                }
                None => {
                    if ui.add(try_again).clicked() {
                        request = Some(AppRequest::TryAgain(job));
                    }
                }
            }
        });
    });
    request
}

/// The result of a split, and what to do next.
fn show_finished(ctx: &egui::Context, name: &str, report: &ExportReport) -> Option<AppRequest> {
    let mut request = None;
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
        ui.add(egui::Label::new(report.plain_summary()).wrap());
        if let Some(details) = report.details() {
            show_details(ui, &details);
        }
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::primary_button("Open folder")).clicked() {
                request = Some(AppRequest::OpenFolder(report.out_dir.clone()));
            }
            let add_more = egui::Button::new("Add more videos")
                .min_size(egui::vec2(0.0, theme::PRIMARY_BUTTON_HEIGHT));
            if ui.add(add_more).clicked() {
                request = Some(AppRequest::OpenAddDialog);
            }
        });
    });
    request
}

/// Shows a folder in the system's file manager. Fails with a sentence for
/// the person, who can still find the folder by the path on screen.
fn open_in_file_manager(folder: &Path) -> Result<(), String> {
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(folder)
        .spawn()
        .map(|_file_manager| ())
        .map_err(|error| {
            format!(
                "The folder {} could not be opened: {error}",
                folder.display()
            )
        })
}

fn show_busy(ctx: &egui::Context, text: &str) {
    show_centered_card(ctx, |ui| {
        ui.spinner();
        ui.label(theme::semibold(text, 16.0));
    });
}

/// A scan under way: what is being scanned, how far it has come as a bar
/// and a number, and what it is doing now.
fn show_scan_progress(ctx: &egui::Context, text: &str, progress: ScanProgress) {
    show_centered_card(ctx, |ui| {
        ui.label(theme::semibold(text, 16.0));
        theme::progress_bar(ui, progress.fraction, SCAN_BAR_HEIGHT);
        ui.label(theme::muted(format!(
            "{}% done. {}",
            percent_done(progress.fraction),
            scan_stage_in_words(progress.stage)
        )));
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

    #[test]
    fn one_video_already_listed_is_told_apart_from_several() {
        assert_eq!(
            counted_already_listed(1),
            "That video is already in the list."
        );
        assert_eq!(
            counted_already_listed(4),
            "Those videos are already in the list."
        );
    }

    #[test]
    fn progress_is_said_in_whole_percent_rounded_down() {
        assert_eq!(percent_done(0.0), 0);
        assert_eq!(percent_done(0.129), 12);
        assert_eq!(percent_done(0.999), 99);
        assert_eq!(percent_done(1.0), 100);
    }

    #[test]
    fn progress_outside_the_scan_is_held_to_its_ends() {
        assert_eq!(percent_done(-0.5), 0);
        assert_eq!(percent_done(7.0), 100);
        assert_eq!(percent_done(f32::NAN), 0);
    }

    #[test]
    fn one_report_is_let_through_for_each_whole_percent() {
        let steps = WholePercentSteps::default();
        // A scan reports every half percent.
        let let_through = (0..=200)
            .filter(|half_percents| steps.admits(*half_percents as f32 / 200.0))
            .count();
        assert_eq!(let_through, 101);
    }

    #[test]
    fn the_first_report_and_the_last_are_always_let_through() {
        let steps = WholePercentSteps::default();
        assert!(steps.admits(0.0));
        assert!(!steps.admits(0.004));
        assert!(steps.admits(1.0));
        assert!(!steps.admits(1.0));
    }

    #[test]
    fn a_report_that_arrives_behind_a_later_one_is_dropped() {
        let steps = WholePercentSteps::default();
        assert!(steps.admits(0.50));
        assert!(!steps.admits(0.42));
        assert!(steps.admits(0.51));
    }

    #[test]
    fn the_sidebar_says_how_far_a_scan_has_come() {
        assert_eq!(status_label(&JobStatus::Scanning(None)), "Scanning");
        let halfway = ScanProgress {
            stage: ScanStage::BlackScreens,
            fraction: 0.5,
        };
        assert_eq!(
            status_label(&JobStatus::Scanning(Some(halfway))),
            "Scanning, 50%"
        );
    }

    #[test]
    fn saying_yes_to_recycling_is_remembered_and_keeps_the_choice() {
        let answered =
            settings_after_recycling_answer(Settings::default(), RecyclingAnswer::MoveOriginal)
                .unwrap();
        assert_eq!(
            answered.export.original_policy,
            OriginalPolicy::RecycleOnceVerified
        );
        assert_eq!(answered.recycling_question, RecyclingQuestion::Answered);
        assert!(!answered.must_ask_before_recycling());
    }

    #[test]
    fn keeping_the_original_is_remembered_and_unticks_recycling() {
        let answered =
            settings_after_recycling_answer(Settings::default(), RecyclingAnswer::KeepOriginal)
                .unwrap();
        assert_eq!(answered.export.original_policy, OriginalPolicy::Keep);
        assert_eq!(answered.recycling_question, RecyclingQuestion::Answered);
    }

    #[test]
    fn cancelling_the_recycling_question_changes_nothing() {
        assert_eq!(
            settings_after_recycling_answer(Settings::default(), RecyclingAnswer::Cancel),
            None
        );
    }

    /// An app with one video in review, as if its scan had just come in.
    fn app_with_a_video_in_review() -> (SceneSplitterApp, JobId) {
        let mut app = SceneSplitterApp::new(LoadedSettings {
            settings: Settings::default(),
            problem: None,
        });
        let crate::queue::Added::New(job) = app.queue.add(Path::new("no_such_video.mp4")) else {
            panic!("the queue was empty");
        };
        app.queue.start_scan(job).unwrap();
        app.queue
            .begin_review(job, crate::review_session::test_sessions::five_scenes())
            .unwrap();
        (app, job)
    }

    #[test]
    fn the_first_split_that_would_recycle_waits_for_an_answer() {
        let ctx = egui::Context::default();
        let (mut app, job) = app_with_a_video_in_review();

        app.split_once_recycling_is_settled(&ctx, job);
        assert_eq!(app.recycling_question_for, Some(job));
        // Nothing was started: the video is still in review.
        assert!(app.queue.review(job).is_some());
    }

    #[test]
    fn cancelling_leaves_the_video_in_review_and_the_question_for_next_time() {
        let ctx = egui::Context::default();
        let (mut app, job) = app_with_a_video_in_review();
        app.split_once_recycling_is_settled(&ctx, job);

        app.answer_recycling_question(&ctx, RecyclingAnswer::Cancel);
        assert_eq!(app.recycling_question_for, None);
        assert!(app.queue.review(job).is_some());
        assert!(app.settings.must_ask_before_recycling());
    }

    #[test]
    fn the_recycling_question_goes_away_with_its_review() {
        let ctx = egui::Context::default();
        let (mut app, job) = app_with_a_video_in_review();
        app.split_once_recycling_is_settled(&ctx, job);

        app.queue.skip(job).unwrap();
        app.drop_the_recycling_question_unless_its_video_is_in_review();
        assert_eq!(app.recycling_question_for, None);
    }

    /// Draws the details of an error for one frame and returns what the
    /// frame put on the clipboard and where the copy button was.
    fn details_frame(
        ctx: &egui::Context,
        details: &str,
        events: Vec<egui::Event>,
    ) -> (String, egui::Rect) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut copy_button = egui::Rect::NOTHING;
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                copy_button = show_details(ui, details).rect;
            });
        });
        (output.platform_output.copied_text, copy_button)
    }

    #[test]
    fn copy_details_puts_the_whole_text_on_the_clipboard() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        // Far more lines than the box shows at once.
        let details: String = (1..=60)
            .map(|line| format!("line {line} of the reason\n"))
            .collect();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        let (copied, copy_button) = details_frame(&ctx, &details, Vec::new());
        assert_eq!(copied, "");
        let pos = copy_button.center();
        details_frame(&ctx, &details, vec![egui::Event::PointerMoved(pos)]);
        details_frame(&ctx, &details, vec![button(pos, true)]);
        let (copied, _) = details_frame(&ctx, &details, vec![button(pos, false)]);
        assert_eq!(copied, details);
    }

    #[test]
    fn a_video_with_no_scene_files_beside_it_has_none_to_replace() {
        let folder = std::env::temp_dir().join(format!(
            "scene_splitter_test_{}_no_earlier_files",
            std::process::id()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let video = folder.join("film.mp4");

        let found = look_for_earlier_scene_files(&video, SceneLocation::BesideVideo);
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(
            found,
            Ok(EarlierSceneFiles {
                scene_dir: folder,
                file_count: 0,
            })
        );
    }

    #[test]
    fn earlier_scene_files_are_counted_in_the_folder_the_scenes_go_to_now() {
        let folder = std::env::temp_dir().join(format!(
            "scene_splitter_test_{}_earlier_files_by_folder",
            std::process::id()
        ));
        let new_folder = folder.join("film_scenes");
        std::fs::create_dir_all(&new_folder).unwrap();
        std::fs::write(folder.join("film_scene_001.mp4"), b"").unwrap();
        std::fs::write(new_folder.join("film_scene_001.mp4"), b"").unwrap();
        std::fs::write(new_folder.join("film_scene_002.mp4"), b"").unwrap();
        let video = folder.join("film.mp4");

        let beside = look_for_earlier_scene_files(&video, SceneLocation::BesideVideo);
        let in_new_folder = look_for_earlier_scene_files(&video, SceneLocation::NewFolder);
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(
            beside,
            Ok(EarlierSceneFiles {
                scene_dir: folder,
                file_count: 1,
            })
        );
        assert_eq!(
            in_new_folder,
            Ok(EarlierSceneFiles {
                scene_dir: new_folder,
                file_count: 2,
            })
        );
    }

    #[test]
    fn scanning_again_looks_afresh_but_a_hard_cut_scan_may_use_what_is_stored() {
        assert_eq!(
            stored_scan_use_for_rescan(ScanKind::BlackScreens),
            CacheUse::ScanFresh
        );
        assert_eq!(
            stored_scan_use_for_rescan(ScanKind::HardCuts(Default::default())),
            CacheUse::ReuseStoredScan
        );
    }

    #[test]
    fn only_the_newest_problems_are_kept() {
        let (mut app, _) = app_with_a_video_in_review();
        for number in 0..MOST_PROBLEMS_KEPT + 2 {
            app.note_problem(format!("problem {number}"));
        }
        assert_eq!(app.problems.len(), MOST_PROBLEMS_KEPT);
        assert_eq!(app.problems[0], "problem 2");
        assert_eq!(app.problems.last().unwrap(), "problem 6");
    }

    #[test]
    fn a_change_of_where_scenes_go_takes_the_warning_off_every_review() {
        let (mut app, job) = app_with_a_video_in_review();
        let three_in_the_way = || -> Result<EarlierSceneFiles, String> {
            Ok(EarlierSceneFiles {
                scene_dir: PathBuf::from("videos"),
                file_count: 3,
            })
        };
        let review = app.queue.review_mut(job).unwrap();
        review.apply(ReviewRequest::Split, &three_in_the_way);
        assert!(review.export_obstacle().is_some());

        // Nothing changed: the warning stays.
        let location_before = app.settings.export.scene_location;
        app.forget_export_obstacles_if_scenes_go_elsewhere(location_before);
        assert!(app.queue.review(job).unwrap().export_obstacle().is_some());

        // Ticked in the sidebar, which sends no request to any review.
        app.settings.export.scene_location = SceneLocation::NewFolder;
        app.forget_export_obstacles_if_scenes_go_elsewhere(location_before);
        assert!(app.queue.review(job).unwrap().export_obstacle().is_none());
    }

    #[test]
    fn a_setting_changed_on_the_review_screen_becomes_the_apps_setting() {
        let (mut app, _) = app_with_a_video_in_review();
        assert!(app.settings.show_tips);
        app.change_settings(SettingsChange::HideTips);
        assert!(!app.settings.show_tips);
    }

    const WINDOW: egui::Vec2 = egui::vec2(1320.0, 900.0);

    /// An app as a person sees it, driven one frame at a time with keys.
    /// Nothing here changes a setting, so nothing is written to disk.
    struct Window {
        ctx: egui::Context,
        app: SceneSplitterApp,
        job: JobId,
    }

    impl Window {
        fn with_a_video_in_review() -> Self {
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let (app, job) = app_with_a_video_in_review();
            let mut window = Self { ctx, app, job };
            // Windows and cards take their place in the first frames.
            window.frame(Vec::new());
            window.frame(Vec::new());
            window
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW)),
                events,
                ..Default::default()
            };
            let _ = self.ctx.run(input, |ctx| self.app.run_frame(ctx));
        }

        fn press(&mut self, key: egui::Key) {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            self.frame(Vec::new());
        }

        /// Opens the player on the video under review, paused, without
        /// starting a decoder.
        fn open_the_player(&mut self) {
            let review = self.app.queue.review(self.job).unwrap();
            let player = Player::paused_at(
                Arc::clone(review.video()),
                review.video_end(),
                review.frame_seconds(),
                47.0,
                &self.ctx,
            );
            self.app.player = Some(ActivePlayer {
                job: self.job,
                player,
            });
        }

        fn select_the_first_two_scenes(&mut self) {
            use crate::plan::SceneIndex;
            use crate::selection::ClickKind;
            let review = self.app.queue.review_mut(self.job).unwrap();
            for (scene, kind) in [(0, ClickKind::Single), (1, ClickKind::ExtendRange)] {
                let click = ReviewRequest::ClickScene {
                    scene: SceneIndex::new(scene),
                    kind,
                };
                review.apply(
                    click,
                    &crate::review_session::test_sessions::nothing_in_the_way,
                );
            }
        }

        fn selected_scene_count(&self) -> usize {
            let review = self.app.queue.review(self.job).unwrap();
            review.selection().scenes().count()
        }

        fn scene_count(&self) -> usize {
            self.app.queue.review(self.job).unwrap().scene_count()
        }
    }

    #[test]
    fn escape_closes_the_player_and_only_the_next_one_clears_the_selection() {
        let mut window = Window::with_a_video_in_review();
        window.select_the_first_two_scenes();
        window.open_the_player();

        window.press(egui::Key::Escape);
        assert!(window.app.player.is_none());
        assert_eq!(window.selected_scene_count(), 2);

        window.press(egui::Key::Escape);
        assert_eq!(window.selected_scene_count(), 0);
    }

    #[test]
    fn keys_do_not_reach_the_review_or_the_player_behind_the_add_dialog() {
        let mut window = Window::with_a_video_in_review();
        window.select_the_first_two_scenes();
        window.open_the_player();
        window.app.add_dialog.open = true;

        // M would merge the selection, S would split at the playhead.
        window.press(egui::Key::M);
        window.press(egui::Key::S);
        assert_eq!(window.scene_count(), 5);

        // Escape closes the dialog, and nothing behind it.
        window.press(egui::Key::Escape);
        assert!(!window.app.add_dialog.open);
        assert!(window.app.player.is_some());
        assert_eq!(window.selected_scene_count(), 2);

        // With the dialog gone the keys work again.
        window.press(egui::Key::M);
        assert_eq!(window.scene_count(), 4);
    }

    #[test]
    fn keys_do_not_reach_the_review_behind_the_recycling_question() {
        let mut window = Window::with_a_video_in_review();
        window.select_the_first_two_scenes();
        window.app.recycling_question_for = Some(window.job);

        window.press(egui::Key::M);
        assert_eq!(window.scene_count(), 5);

        // Escape answers the question with "not now" and does nothing else.
        window.press(egui::Key::Escape);
        assert_eq!(window.app.recycling_question_for, None);
        assert_eq!(window.selected_scene_count(), 2);
        assert!(window.app.queue.review(window.job).is_some());
    }

    #[test]
    fn escape_pressed_while_typing_only_leaves_the_text_box() {
        let mut window = Window::with_a_video_in_review();
        window.select_the_first_two_scenes();
        window.open_the_player();
        // As the frame before ended, the person was typing a time into the
        // "Add a split at" box.
        window.app.text_box_had_the_keyboard = true;

        window.press(egui::Key::Escape);
        assert!(window.app.player.is_some());
        assert_eq!(window.selected_scene_count(), 2);

        // The box is left, so the next Escape is the player's.
        window.press(egui::Key::Escape);
        assert!(window.app.player.is_none());
    }

    #[test]
    fn a_failed_split_offers_the_way_back_to_the_review() {
        let ctx = egui::Context::default();
        let (mut app, job) = app_with_a_video_in_review();
        app.queue.start_export(job).unwrap();
        app.queue
            .failed(job, Failure::could_not_split("the disk is full".to_owned()))
            .unwrap();

        app.apply_request(&ctx, AppRequest::BackToReview(job));
        assert_eq!(app.queue.review(job).unwrap().scene_count(), 5);
        assert!(app.problems.is_empty());
    }

    #[test]
    fn a_folder_that_cannot_be_opened_is_a_problem_the_sidebar_shows() {
        let (mut app, _) = app_with_a_video_in_review();
        app.note_problem("The folder D:\\gone could not be opened: not found".to_owned());
        assert_eq!(app.problems.len(), 1);

        let ctx = egui::Context::default();
        app.apply_request(&ctx, AppRequest::DismissProblems);
        assert!(app.problems.is_empty());
    }
}
