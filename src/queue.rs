//! The videos the person has added and how far each one has come: waiting,
//! being scanned, in review, ready to be split, being split, done, skipped
//! or failed.
//!
//! Every change of status is a method here, and each one refuses a video
//! that is not at a stage the change makes sense for. Nothing in this module
//! draws, starts a thread or touches a file. The window asks it what to do
//! next and tells it how that went.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cache::{CacheUse, SourcedDetection};
use crate::detect::{ScanDepth, ScanKind, ScanProgress};
use crate::exact_split::ExactCutError;
use crate::export::ExportReport;
use crate::fates::ScenesToWrite;
use crate::ffmpeg::FfmpegError;
use crate::review_session::ReviewSession;
use crate::scenes::CutTimes;

/// Names one video for as long as the window is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(u64);

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "video {}", self.0)
    }
}

/// Why a video could not be finished: a sentence for the person, and the
/// technical reason behind it when there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub plain_words: String,
    /// `None` when the sentence says all there is to say.
    pub details: Option<String>,
}

impl Failure {
    pub fn unreadable_video(details: String) -> Self {
        Self {
            plain_words:
                "This file could not be read as a video. It may be damaged, or it is not a video."
                    .to_owned(),
            details: Some(details),
        }
    }

    /// Why a scan failed, in words that fit what went wrong. A scan that ran
    /// out of time or could not start its tools says nothing about the file
    /// being damaged, so it is not reported as if it did.
    pub fn from_scan_error(error: &anyhow::Error) -> Self {
        let details = format!("{error:#}");
        let ffmpeg_error = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<FfmpegError>());
        let plain_words = match ffmpeg_error {
            Some(FfmpegError::TimedOut { .. }) => {
                "The scan stopped because the video could not be read in time. \
                 Check the drive it is on and try again."
            }
            Some(FfmpegError::CouldNotStart { .. }) => {
                "The video tools this app needs could not be started. \
                 Close the app, open it again and try once more."
            }
            Some(
                FfmpegError::Failed { .. }
                | FfmpegError::OutputNotUnderstood { .. }
                | FfmpegError::LostContact { .. },
            )
            | None => return Self::unreadable_video(details),
        };
        Self {
            plain_words: plain_words.to_owned(),
            details: Some(details),
        }
    }

    /// Why a split failed. A failed exact cut says what it says about
    /// itself, which includes the way back to a cut without encoding.
    pub fn from_split_error(error: &anyhow::Error) -> Self {
        let details = format!("{error:#}");
        match error.downcast_ref::<ExactCutError>() {
            Some(exact_cut) => Self {
                plain_words: exact_cut.in_plain_words(),
                details: Some(details),
            },
            None => Self::could_not_split(details),
        }
    }

    pub fn could_not_split(details: String) -> Self {
        Self {
            plain_words: "The video could not be split. The original was not changed.".to_owned(),
            details: Some(details),
        }
    }

    /// For a problem that is already a sentence the person can act on.
    pub fn in_plain_words(plain_words: String) -> Self {
        Self {
            plain_words,
            details: None,
        }
    }

    fn cut_outside_video(details: String) -> Self {
        Self {
            plain_words: "The scan of this video gave a scene change that is not inside the video."
                .to_owned(),
            details: Some(details),
        }
    }

    fn crashed() -> Self {
        Self::in_plain_words(
            "Something went wrong while working on this video. The original was not changed. \
             Scene files that were already written may be incomplete."
                .to_owned(),
        )
    }
}

#[derive(Debug)]
pub enum JobStatus {
    Waiting,
    /// How far the scan has come, once it has said so. A scan that turns out
    /// to be stored from last time never does.
    Scanning(Option<ScanProgress>),
    Reviewing(Box<ReviewSession>),
    /// Reviewed, and waiting to be split together with the other videos
    /// that are ready, when the person says so. Its scenes cannot be edited
    /// until it is taken back into review.
    ReadyToSplit(Box<ReviewSession>),
    /// The review is kept while its split runs. If the split fails, nothing
    /// about the review was wrong, and the person gets it back.
    Exporting(Box<ReviewSession>),
    Finished(ExportReport),
    /// Left as it is. The review is kept, in case that was a slip.
    Skipped(Box<ReviewSession>),
    Failed {
        failure: Failure,
        /// The review the video was in when its split failed. `None` when
        /// the scan failed, before there was anything to review.
        review: Option<Box<ReviewSession>>,
    },
}

/// A status without what it carries, for comparing and for messages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Waiting,
    Scanning,
    Reviewing,
    ReadyToSplit,
    Exporting,
    Finished,
    Skipped,
    Failed,
}

impl fmt::Display for Stage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let in_words = match self {
            Self::Waiting => "waiting",
            Self::Scanning => "being scanned",
            Self::Reviewing => "in review",
            Self::ReadyToSplit => "ready to be split",
            Self::Exporting => "being split",
            Self::Finished => "finished",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        };
        formatter.write_str(in_words)
    }
}

impl JobStatus {
    pub fn stage(&self) -> Stage {
        match self {
            Self::Waiting => Stage::Waiting,
            Self::Scanning(_) => Stage::Scanning,
            Self::Reviewing(_) => Stage::Reviewing,
            Self::ReadyToSplit(_) => Stage::ReadyToSplit,
            Self::Exporting(_) => Stage::Exporting,
            Self::Finished(_) => Stage::Finished,
            Self::Skipped(_) => Stage::Skipped,
            Self::Failed { .. } => Stage::Failed,
        }
    }

    /// The review of the video, wherever it is kept: under way, put aside,
    /// waiting to be split, or waiting for the outcome of its split.
    pub fn kept_review(&self) -> Option<&ReviewSession> {
        match self {
            Self::Reviewing(review)
            | Self::ReadyToSplit(review)
            | Self::Exporting(review)
            | Self::Skipped(review) => Some(review),
            Self::Failed { review, .. } => review.as_deref(),
            Self::Waiting | Self::Scanning(_) | Self::Finished(_) => None,
        }
    }

    fn kept_review_mut(&mut self) -> Option<&mut ReviewSession> {
        match self {
            Self::Reviewing(review)
            | Self::ReadyToSplit(review)
            | Self::Exporting(review)
            | Self::Skipped(review) => Some(review),
            Self::Failed { review, .. } => review.as_deref_mut(),
            Self::Waiting | Self::Scanning(_) | Self::Finished(_) => None,
        }
    }

    /// Takes the review out of the status, leaving the video waiting until
    /// the caller sets what it has become.
    fn take_kept_review(&mut self) -> Option<Box<ReviewSession>> {
        match std::mem::replace(self, Self::Waiting) {
            Self::Reviewing(review)
            | Self::ReadyToSplit(review)
            | Self::Exporting(review)
            | Self::Skipped(review) => Some(review),
            Self::Failed { review, .. } => review,
            Self::Waiting | Self::Scanning(_) | Self::Finished(_) => None,
        }
    }
}

impl Stage {
    /// Whether the person still has something to do or wait for here.
    fn is_open(self) -> bool {
        matches!(self, Self::Waiting | Self::Scanning | Self::Reviewing)
    }
}

/// One video in the queue.
#[derive(Debug)]
pub struct Job {
    id: JobId,
    video: Arc<Path>,
    status: JobStatus,
    /// Whether the next scan of this video may reuse a stored one.
    cache_use: CacheUse,
    scan_kind: ScanKind,
}

impl Job {
    pub fn id(&self) -> JobId {
        self.id
    }

    pub fn video(&self) -> &Arc<Path> {
        &self.video
    }

    pub fn status(&self) -> &JobStatus {
        &self.status
    }

    pub fn scan_kind(&self) -> ScanKind {
        self.scan_kind
    }
}

/// A change of status, named for the message of a refusal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transition {
    StartScan,
    ScanProgressed,
    ScanFinished,
    BeginReview,
    Skip,
    ReviewAfterAll,
    Rescan,
    MarkReady,
    EditAgain,
    StartExport,
    StartReadyExport,
    Finished,
    Failed,
    Crashed,
    TryAgain,
    BackToReview,
}

impl fmt::Display for Transition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let in_words = match self {
            Self::StartScan => "starting the scan",
            Self::ScanProgressed => "reporting how far the scan is",
            Self::ScanFinished => "taking the result of the scan",
            Self::BeginReview => "beginning the review",
            Self::Skip => "skipping",
            Self::ReviewAfterAll => "reviewing after all",
            Self::Rescan => "scanning again",
            Self::MarkReady => "marking as ready to split",
            Self::EditAgain => "editing the scenes again",
            Self::StartExport => "starting the split",
            Self::StartReadyExport => "starting the split of a ready video",
            Self::Finished => "taking the result of the split",
            Self::Failed => "failing",
            Self::Crashed => "reporting a crash",
            Self::TryAgain => "trying again",
            Self::BackToReview => "going back to the review",
        };
        formatter.write_str(in_words)
    }
}

/// Why the queue did not do what it was asked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refused {
    /// No such video is in the queue.
    NoSuchJob(JobId),
    /// The video is not at a stage this change can start from.
    WrongStage {
        job: JobId,
        transition: Transition,
        stage: Stage,
    },
    /// Scans run one at a time, and this video is being scanned already.
    AlreadyScanning(JobId),
    /// The video failed before it had a review, so there is none to go
    /// back to.
    NoReviewKept(JobId),
}

impl fmt::Display for Refused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchJob(job) => write!(formatter, "{job} is not in the queue"),
            Self::WrongStage {
                job,
                transition,
                stage,
            } => write!(
                formatter,
                "{transition} is not possible for {job}, which is {stage}"
            ),
            Self::AlreadyScanning(job) => write!(
                formatter,
                "another scan cannot start while {job} is being scanned"
            ),
            Self::NoReviewKept(job) => {
                write!(
                    formatter,
                    "{job} failed before it had a review to go back to"
                )
            }
        }
    }
}

impl std::error::Error for Refused {}

/// What happened to a video that was asked to be added.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Added {
    New(JobId),
    /// It is in the queue already, as this job.
    AlreadyListed(JobId),
}

/// What adding several videos at once came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AddedVideos {
    pub added: usize,
    /// One of the videos that were in the queue already, if any were.
    pub already_listed: Option<JobId>,
}

/// A scan to run in the background.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanOrder {
    pub job: JobId,
    pub video: Arc<Path>,
    pub cache_use: CacheUse,
    pub depth: ScanDepth,
}

/// A split to run in the background.
#[derive(Debug, PartialEq)]
pub struct ExportOrder {
    pub job: JobId,
    pub video: Arc<Path>,
    /// Where to cut, and which of the scenes between the cuts to write.
    pub scenes: ScenesToWrite,
}

#[derive(Debug, Default)]
pub struct Queue {
    /// In the order they were added.
    jobs: Vec<Job>,
    /// The job shown beside the list.
    current: Option<JobId>,
    jobs_ever_added: u64,
}

impl Queue {
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn job(&self, id: JobId) -> Option<&Job> {
        self.jobs.iter().find(|job| job.id == id)
    }

    pub fn current(&self) -> Option<JobId> {
        self.current
    }

    pub fn current_job(&self) -> Option<&Job> {
        self.job(self.current?)
    }

    /// The review of a video that is being reviewed right now.
    pub fn review(&self, id: JobId) -> Option<&ReviewSession> {
        match &self.job(id)?.status {
            JobStatus::Reviewing(session) => Some(session),
            _ => None,
        }
    }

    pub fn review_mut(&mut self, id: JobId) -> Option<&mut ReviewSession> {
        match &mut self.job_mut(id).ok()?.status {
            JobStatus::Reviewing(session) => Some(session),
            _ => None,
        }
    }

    /// Adds a video to the end of the queue, unless it is there already.
    /// The first video added is the one shown.
    pub fn add(&mut self, video: &Path) -> Added {
        if let Some(listed) = self.jobs.iter().find(|job| &*job.video == video) {
            return Added::AlreadyListed(listed.id);
        }
        let id = JobId(self.jobs_ever_added);
        self.jobs_ever_added += 1;
        self.jobs.push(Job {
            id,
            video: Arc::from(video),
            status: JobStatus::Waiting,
            cache_use: CacheUse::ReuseStoredScan,
            scan_kind: ScanKind::BlackScreens,
        });
        if self.current.is_none() {
            self.current = Some(id);
        }
        Added::New(id)
    }

    /// Adds every video that is not in the queue yet. When all of them are,
    /// one of them is shown, since the person asked for it.
    pub fn add_all(&mut self, videos: &[PathBuf]) -> AddedVideos {
        let mut summary = AddedVideos {
            added: 0,
            already_listed: None,
        };
        for video in videos {
            match self.add(video) {
                Added::New(_) => summary.added += 1,
                Added::AlreadyListed(job) => summary.already_listed = Some(job),
            }
        }
        if let (0, Some(job)) = (summary.added, summary.already_listed) {
            self.current = Some(job);
        }
        summary
    }

    /// Shows this video beside the list.
    pub fn show(&mut self, id: JobId) -> Result<(), Refused> {
        self.job_mut(id)?;
        self.current = Some(id);
        Ok(())
    }

    /// Chooses the scan for every video still waiting.
    pub fn scan_waiting_videos_as(&mut self, scan_kind: ScanKind) {
        for job in &mut self.jobs {
            if job.status.stage() == Stage::Waiting {
                job.scan_kind = scan_kind;
            }
        }
    }

    /// Starts the scan of the first waiting video, unless a scan is running
    /// or nothing waits. A scan already uses every core, so running several
    /// at once would only make each of them slower.
    pub fn start_next_scan(&mut self) -> Option<ScanOrder> {
        let waiting = self
            .jobs
            .iter()
            .find(|job| job.status.stage() == Stage::Waiting)?
            .id;
        self.start_scan(waiting).ok()
    }

    pub fn start_scan(&mut self, id: JobId) -> Result<ScanOrder, Refused> {
        if let Some(scanning) = self.scanning_job() {
            return Err(Refused::AlreadyScanning(scanning));
        }
        let job = self.job_at(id, Transition::StartScan, &[Stage::Waiting])?;
        job.status = JobStatus::Scanning(None);
        Ok(ScanOrder {
            job: id,
            video: Arc::clone(&job.video),
            cache_use: job.cache_use,
            depth: job.scan_kind.depth(),
        })
    }

    /// The scan says how far it has come. A scan reports from several
    /// threads, so a report can arrive after one that was made later. The
    /// furthest one heard so far is kept, and the bar never moves backwards.
    pub fn scan_progressed(&mut self, id: JobId, progress: ScanProgress) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::ScanProgressed, &[Stage::Scanning])?;
        let is_further = match &job.status {
            JobStatus::Scanning(Some(furthest)) => progress.fraction > furthest.fraction,
            _ => true,
        };
        if is_further {
            job.status = JobStatus::Scanning(Some(progress));
        }
        Ok(())
    }

    /// The scan is in. The video goes to review, starting from the cuts of
    /// the scan the person chose, or fails when the scan makes no sense.
    pub fn scan_finished(&mut self, id: JobId, scan: SourcedDetection) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::ScanFinished, &[Stage::Scanning])?;
        let video = Arc::clone(&job.video);
        let detected_times = scan.detection.cuts_for(job.scan_kind);
        // A scan never suggests a cut outside its video. A stored scan that was
        // damaged on disk could, and a review built on it would write nonsense.
        let cuts = match CutTimes::inside(scan.detection.video, detected_times) {
            Ok(cuts) => cuts,
            Err(error) => return self.failed(id, Failure::cut_outside_video(error.to_string())),
        };
        self.begin_review(id, ReviewSession::new(video, scan, cuts))
    }

    pub fn begin_review(&mut self, id: JobId, session: ReviewSession) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::BeginReview, &[Stage::Scanning])?;
        job.status = JobStatus::Reviewing(Box::new(session));
        Ok(())
    }

    /// Leaves the video as it is and shows the next one that needs
    /// attention. The review is kept, so it can be taken up again exactly as
    /// it was.
    pub fn skip(&mut self, id: JobId) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::Skip, &[Stage::Reviewing])?;
        job.status = match std::mem::replace(&mut job.status, JobStatus::Waiting) {
            JobStatus::Reviewing(session) => JobStatus::Skipped(session),
            other => other,
        };
        self.move_on_from(id);
        Ok(())
    }

    pub fn review_after_all(&mut self, id: JobId) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::ReviewAfterAll, &[Stage::Skipped])?;
        job.status = match std::mem::replace(&mut job.status, JobStatus::Waiting) {
            JobStatus::Skipped(session) => JobStatus::Reviewing(session),
            other => other,
        };
        Ok(())
    }

    /// Sends a video in review back to be scanned again, as the kind of scan
    /// given. Its review, with every edit, is gone.
    ///
    /// `cache_use` says whether a stored scan may answer. A scan for black
    /// screens that the person asked to repeat must not be answered from the
    /// store. A hard-cut scan may: its slow part is stored, and only the
    /// settings are applied afresh.
    pub fn rescan(
        &mut self,
        id: JobId,
        scan_kind: ScanKind,
        cache_use: CacheUse,
    ) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::Rescan, &[Stage::Reviewing])?;
        job.cache_use = cache_use;
        job.scan_kind = scan_kind;
        job.status = JobStatus::Waiting;
        Ok(())
    }

    /// Puts a reviewed video with those that are ready to be split, and
    /// shows the next video that needs attention. Nothing is written yet.
    pub fn mark_ready(&mut self, id: JobId) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::MarkReady, &[Stage::Reviewing])?;
        job.status = match std::mem::replace(&mut job.status, JobStatus::Waiting) {
            JobStatus::Reviewing(session) => JobStatus::ReadyToSplit(session),
            other => other,
        };
        self.move_on_from(id);
        Ok(())
    }

    /// Takes a ready video back into review, with every merge and added
    /// split as it was. It is no longer ready: a video that is split
    /// unattended must be exactly the one the person last looked at and
    /// called ready, so after more edits they say so again. The leave to
    /// replace earlier scene files is not kept either, as what is on disk
    /// may have changed since.
    pub fn edit_again(&mut self, id: JobId) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::EditAgain, &[Stage::ReadyToSplit])?;
        job.status = match std::mem::replace(&mut job.status, JobStatus::Waiting) {
            JobStatus::ReadyToSplit(mut session) => {
                session.forget_export_obstacle();
                JobStatus::Reviewing(session)
            }
            other => other,
        };
        Ok(())
    }

    /// The videos that are ready to be split, in the order of the list.
    pub fn ready_jobs(&self) -> Vec<JobId> {
        self.jobs
            .iter()
            .filter(|job| job.status.stage() == Stage::ReadyToSplit)
            .map(|job| job.id)
            .collect()
    }

    /// The review of a video that is ready to be split, which says what it
    /// would be split into.
    pub fn ready_review(&self, id: JobId) -> Option<&ReviewSession> {
        match &self.job(id)?.status {
            JobStatus::ReadyToSplit(session) => Some(session),
            _ => None,
        }
    }

    /// Takes the scenes of the review as final and shows the next video that
    /// needs attention, so the person can keep reviewing while this one is
    /// written. The review stays with the video until the split has worked.
    pub fn start_export(&mut self, id: JobId) -> Result<ExportOrder, Refused> {
        let order = self.begin_export(id, Transition::StartExport, Stage::Reviewing)?;
        self.move_on_from(id);
        Ok(order)
    }

    /// Starts the split of a video that waited to be split with the others.
    /// What is shown stays as it is: the person is looking at another video
    /// by now, and a run of splits must not page through the list.
    pub fn start_ready_export(&mut self, id: JobId) -> Result<ExportOrder, Refused> {
        self.begin_export(id, Transition::StartReadyExport, Stage::ReadyToSplit)
    }

    fn begin_export(
        &mut self,
        id: JobId,
        transition: Transition,
        from: Stage,
    ) -> Result<ExportOrder, Refused> {
        let job = self.job_at(id, transition, &[from])?;
        let video = Arc::clone(&job.video);
        // A video at either of the stages a split starts from has its review.
        let Some(session) = job.status.take_kept_review() else {
            return Err(Refused::NoReviewKept(id));
        };
        let scenes = session.scenes_to_write();
        job.status = JobStatus::Exporting(session);
        Ok(ExportOrder {
            job: id,
            video,
            scenes,
        })
    }

    /// The split worked. Only now is the review let go.
    pub fn finished(&mut self, id: JobId, report: ExportReport) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::Finished, &[Stage::Exporting])?;
        job.status = JobStatus::Finished(report);
        Ok(())
    }

    /// The scan or the split did not work out, or the split could not even
    /// be started from the review. A video that had a review keeps it.
    pub fn failed(&mut self, id: JobId, failure: Failure) -> Result<(), Refused> {
        let may_fail = [
            Stage::Scanning,
            Stage::Reviewing,
            Stage::ReadyToSplit,
            Stage::Exporting,
        ];
        let job = self.job_at(id, Transition::Failed, &may_fail)?;
        let review = job.status.take_kept_review();
        job.status = JobStatus::Failed { failure, review };
        Ok(())
    }

    /// The background work on the video stopped without a result.
    pub fn crashed(&mut self, id: JobId) -> Result<(), Refused> {
        let in_flight = [Stage::Scanning, Stage::Exporting];
        let job = self.job_at(id, Transition::Crashed, &in_flight)?;
        let review = job.status.take_kept_review();
        job.status = JobStatus::Failed {
            failure: Failure::crashed(),
            review,
        };
        Ok(())
    }

    /// Puts a failed video back in line to be scanned. A review it still
    /// had is gone.
    pub fn try_again(&mut self, id: JobId) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::TryAgain, &[Stage::Failed])?;
        job.cache_use = CacheUse::ReuseStoredScan;
        job.status = JobStatus::Waiting;
        Ok(())
    }

    /// Takes up the review of a video whose split failed, with every merge
    /// and added split as it was. A warning about earlier scene files is
    /// not kept: what is on disk may have changed since.
    pub fn back_to_review(&mut self, id: JobId) -> Result<(), Refused> {
        let job = self.job_at(id, Transition::BackToReview, &[Stage::Failed])?;
        if job.status.kept_review().is_none() {
            return Err(Refused::NoReviewKept(id));
        }
        if let Some(mut review) = job.status.take_kept_review() {
            review.forget_export_obstacle();
            job.status = JobStatus::Reviewing(review);
        }
        Ok(())
    }

    /// The scenes will be written somewhere else from now on, so no warning
    /// about the scene files in the old place applies any more, in any
    /// review.
    pub fn forget_export_obstacles(&mut self) {
        for review in self
            .jobs
            .iter_mut()
            .filter_map(|job| job.status.kept_review_mut())
        {
            review.forget_export_obstacle();
        }
    }

    fn scanning_job(&self) -> Option<JobId> {
        self.jobs
            .iter()
            .find(|job| job.status.stage() == Stage::Scanning)
            .map(|job| job.id)
    }

    fn job_mut(&mut self, id: JobId) -> Result<&mut Job, Refused> {
        self.jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or(Refused::NoSuchJob(id))
    }

    /// The job, if it is at one of the stages `transition` can start from.
    fn job_at(
        &mut self,
        id: JobId,
        transition: Transition,
        possible_from: &[Stage],
    ) -> Result<&mut Job, Refused> {
        let job = self.job_mut(id)?;
        let stage = job.status.stage();
        if !possible_from.contains(&stage) {
            return Err(Refused::WrongStage {
                job: id,
                transition,
                stage,
            });
        }
        Ok(job)
    }

    /// The jobs after this one in the list, then those before it.
    fn jobs_around(&self, id: JobId) -> impl Iterator<Item = &Job> {
        let position = self
            .jobs
            .iter()
            .position(|job| job.id == id)
            .unwrap_or(self.jobs.len());
        let (earlier, this_and_later) = self.jobs.split_at(position);
        this_and_later.iter().skip(1).chain(earlier)
    }

    /// Shows the next video that still needs attention, if there is one.
    fn move_on_from(&mut self, done_with: JobId) {
        let next_open_job = self
            .jobs_around(done_with)
            .find(|job| job.status.stage().is_open())
            .map(|job| job.id);
        if let Some(job) = next_open_job {
            self.current = Some(job);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::DetectionSource;
    use crate::detect::{Detection, ScanStage};
    use crate::export::{OriginalOutcome, Verification};
    use crate::ffmpeg::{Program, TimeLimit};
    use crate::hardcut::{HardCutCandidate, HardCutSettings};
    use crate::plan::SceneIndex;
    use crate::review_session::{test_sessions, ExportObstacle, ReviewRequest};
    use crate::scan::UndecodedFrames;
    use crate::timecode::TimeSpan;
    use std::time::Duration;

    /// A scan of a 100 second video that suggests the given cuts.
    fn scan_suggesting(cuts: Vec<f64>) -> SourcedDetection {
        SourcedDetection {
            detection: Detection {
                video: TimeSpan {
                    start: 0.0,
                    end: 100.0,
                },
                keyframe_times: vec![0.0, 20.0, 40.0, 60.0, 80.0],
                frame_count: 3000,
                black_screens: Vec::new(),
                cuts,
                decoded_frames: 0,
                decoded_windows: 0,
                hard_cut_candidates: None,
                chapters: crate::detect::ChapterMarks::NeverRead,
            },
            source: DetectionSource::FreshScan,
            undecoded: Vec::new(),
        }
    }

    fn two_cuts() -> SourcedDetection {
        scan_suggesting(vec![20.0, 60.0])
    }

    fn report() -> ExportReport {
        ExportReport {
            scene_files: Vec::new(),
            out_dir: PathBuf::from("scenes"),
            dropped_scenes: 0,
            verification: Verification::Passed,
            original: OriginalOutcome::Kept,
        }
    }

    fn failure() -> Failure {
        Failure::in_plain_words("It did not work.".to_owned())
    }

    fn queue_with(video_count: usize) -> (Queue, Vec<JobId>) {
        let mut queue = Queue::default();
        let ids = (0..video_count)
            .map(|number| {
                let Added::New(id) = queue.add(Path::new(&format!("video_{number}.mp4"))) else {
                    panic!("each of these videos is new to the queue");
                };
                id
            })
            .collect();
        (queue, ids)
    }

    fn stage(queue: &Queue, id: JobId) -> Stage {
        queue.job(id).unwrap().status().stage()
    }

    fn stages(queue: &Queue) -> Vec<Stage> {
        queue
            .jobs()
            .iter()
            .map(|job| job.status().stage())
            .collect()
    }

    fn scan_for_black_screens_again(queue: &mut Queue, id: JobId) -> Result<(), Refused> {
        queue.rescan(id, ScanKind::BlackScreens, CacheUse::ScanFresh)
    }

    /// Takes a waiting video as far as its review.
    fn bring_to_review(queue: &mut Queue, id: JobId) {
        queue.start_scan(id).unwrap();
        queue.scan_finished(id, two_cuts()).unwrap();
    }

    /// A queue of one video at the given stage.
    fn queue_at(wanted: Stage) -> (Queue, JobId) {
        let (mut queue, ids) = queue_with(1);
        let id = ids[0];
        match wanted {
            Stage::Waiting => {}
            Stage::Scanning => {
                queue.start_scan(id).unwrap();
            }
            Stage::Reviewing => bring_to_review(&mut queue, id),
            Stage::ReadyToSplit => {
                bring_to_review(&mut queue, id);
                queue.mark_ready(id).unwrap();
            }
            Stage::Exporting => {
                bring_to_review(&mut queue, id);
                queue.start_export(id).unwrap();
            }
            Stage::Finished => {
                bring_to_review(&mut queue, id);
                queue.start_export(id).unwrap();
                queue.finished(id, report()).unwrap();
            }
            Stage::Skipped => {
                bring_to_review(&mut queue, id);
                queue.skip(id).unwrap();
            }
            Stage::Failed => {
                queue.start_scan(id).unwrap();
                queue.failed(id, failure()).unwrap();
            }
        }
        assert_eq!(stage(&queue, id), wanted);
        (queue, id)
    }

    const EVERY_STAGE: [Stage; 8] = [
        Stage::Waiting,
        Stage::Scanning,
        Stage::Reviewing,
        Stage::ReadyToSplit,
        Stage::Exporting,
        Stage::Finished,
        Stage::Skipped,
        Stage::Failed,
    ];

    /// Checks that `change` is refused, and changes nothing, from every
    /// stage except `possible_from`.
    fn assert_refused_elsewhere(
        transition: Transition,
        possible_from: &[Stage],
        change: impl Fn(&mut Queue, JobId) -> Result<(), Refused>,
    ) {
        for from in EVERY_STAGE {
            if possible_from.contains(&from) {
                continue;
            }
            let (mut queue, id) = queue_at(from);
            assert_eq!(
                change(&mut queue, id),
                Err(Refused::WrongStage {
                    job: id,
                    transition,
                    stage: from,
                }),
                "{transition:?} from {from:?}"
            );
            assert_eq!(stage(&queue, id), from, "{transition:?} from {from:?}");
        }
    }

    #[test]
    fn the_first_video_added_is_the_one_shown() {
        let (queue, ids) = queue_with(3);
        assert_eq!(queue.current(), Some(ids[0]));
        assert_eq!(
            stages(&queue),
            vec![Stage::Waiting, Stage::Waiting, Stage::Waiting]
        );
    }

    #[test]
    fn an_empty_queue_shows_nothing() {
        let queue = Queue::default();
        assert!(queue.is_empty());
        assert_eq!(queue.current(), None);
        assert!(queue.current_job().is_none());
    }

    #[test]
    fn a_video_already_in_the_queue_is_not_added_twice() {
        let (mut queue, ids) = queue_with(2);
        assert_eq!(
            queue.add(Path::new("video_1.mp4")),
            Added::AlreadyListed(ids[1])
        );
        assert_eq!(queue.jobs().len(), 2);
    }

    #[test]
    fn adding_several_videos_counts_the_new_ones() {
        let (mut queue, ids) = queue_with(2);
        let added = queue.add_all(&[
            PathBuf::from("video_1.mp4"),
            PathBuf::from("holiday.mp4"),
            PathBuf::from("wedding.mp4"),
        ]);

        assert_eq!(
            added,
            AddedVideos {
                added: 2,
                already_listed: Some(ids[1]),
            }
        );
        // Something new was added, so what is shown stays as it was.
        assert_eq!(queue.current(), Some(ids[0]));
    }

    #[test]
    fn adding_only_videos_that_are_listed_shows_one_of_them() {
        let (mut queue, ids) = queue_with(3);
        let added = queue.add_all(&[PathBuf::from("video_2.mp4")]);

        assert_eq!(added.added, 0);
        assert_eq!(queue.current(), Some(ids[2]));
    }

    #[test]
    fn any_video_in_the_queue_can_be_shown() {
        let (mut queue, ids) = queue_with(3);
        queue.show(ids[2]).unwrap();
        assert_eq!(queue.current_job().unwrap().id(), ids[2]);
    }

    #[test]
    fn a_video_that_is_not_in_the_queue_cannot_be_shown() {
        let (mut queue, ids) = queue_with(2);
        // The third video of a longer queue is not in this one.
        let stranger = queue_with(3).1[2];

        assert_eq!(queue.show(stranger), Err(Refused::NoSuchJob(stranger)));
        assert_eq!(queue.current(), Some(ids[0]));
    }

    #[test]
    fn videos_are_scanned_one_at_a_time_in_the_order_they_were_added() {
        let (mut queue, ids) = queue_with(2);

        let first = queue.start_next_scan().unwrap();
        assert_eq!(first.job, ids[0]);
        assert_eq!(&*first.video, Path::new("video_0.mp4"));
        assert_eq!(first.cache_use, CacheUse::ReuseStoredScan);
        assert_eq!(first.depth, ScanDepth::BlackScreens);

        // The second waits for the first.
        assert_eq!(queue.start_next_scan(), None);
        assert_eq!(stages(&queue), vec![Stage::Scanning, Stage::Waiting]);

        queue.scan_finished(ids[0], two_cuts()).unwrap();
        assert_eq!(queue.start_next_scan().unwrap().job, ids[1]);
    }

    #[test]
    fn a_second_scan_is_refused_while_one_is_running() {
        let (mut queue, ids) = queue_with(2);
        queue.start_scan(ids[0]).unwrap();

        assert_eq!(
            queue.start_scan(ids[1]),
            Err(Refused::AlreadyScanning(ids[0]))
        );
        assert_eq!(stage(&queue, ids[1]), Stage::Waiting);
    }

    #[test]
    fn nothing_is_scanned_when_nothing_waits() {
        let (mut queue, _) = queue_at(Stage::Reviewing);
        assert_eq!(queue.start_next_scan(), None);
        assert_eq!(Queue::default().start_next_scan(), None);
    }

    #[test]
    fn only_a_waiting_video_can_start_its_scan() {
        assert_refused_elsewhere(
            Transition::StartScan,
            // A scan in flight is refused for its own reason, tested above.
            &[Stage::Waiting, Stage::Scanning],
            |queue, id| queue.start_scan(id).map(|_order| ()),
        );
    }

    fn black_screens_done(fraction: f32) -> ScanProgress {
        ScanProgress {
            stage: ScanStage::BlackScreens,
            fraction,
        }
    }

    fn scan_progress(queue: &Queue, id: JobId) -> Option<ScanProgress> {
        match queue.job(id).unwrap().status() {
            JobStatus::Scanning(progress) => *progress,
            other => panic!("the video is not being scanned but {:?}", other.stage()),
        }
    }

    #[test]
    fn a_scan_has_no_progress_until_it_reports_some() {
        let (queue, id) = queue_at(Stage::Scanning);
        assert_eq!(scan_progress(&queue, id), None);
    }

    #[test]
    fn a_scan_shows_the_progress_it_reported() {
        let (mut queue, id) = queue_at(Stage::Scanning);
        queue.scan_progressed(id, black_screens_done(0.25)).unwrap();
        assert_eq!(scan_progress(&queue, id), Some(black_screens_done(0.25)));

        let hard_cuts = ScanProgress {
            stage: ScanStage::HardCuts,
            fraction: 0.5,
        };
        queue.scan_progressed(id, hard_cuts).unwrap();
        assert_eq!(scan_progress(&queue, id), Some(hard_cuts));
    }

    #[test]
    fn a_report_that_arrives_late_does_not_move_the_progress_backwards() {
        let (mut queue, id) = queue_at(Stage::Scanning);
        queue.scan_progressed(id, black_screens_done(0.5)).unwrap();
        queue.scan_progressed(id, black_screens_done(0.4)).unwrap();
        assert_eq!(scan_progress(&queue, id), Some(black_screens_done(0.5)));
    }

    #[test]
    fn a_new_scan_of_the_same_video_starts_without_progress() {
        let (mut queue, id) = queue_at(Stage::Scanning);
        queue.scan_progressed(id, black_screens_done(1.0)).unwrap();
        queue.scan_finished(id, two_cuts()).unwrap();
        scan_for_black_screens_again(&mut queue, id).unwrap();

        queue.start_scan(id).unwrap();
        assert_eq!(scan_progress(&queue, id), None);
    }

    #[test]
    fn progress_is_refused_for_a_video_that_is_not_being_scanned() {
        assert_refused_elsewhere(
            Transition::ScanProgressed,
            &[Stage::Scanning],
            |queue, id| queue.scan_progressed(id, black_screens_done(0.5)),
        );
    }

    #[test]
    fn the_scan_chosen_for_waiting_videos_does_not_touch_the_others() {
        let (mut queue, ids) = queue_with(2);
        bring_to_review(&mut queue, ids[0]);
        let hard_cuts = ScanKind::HardCuts(HardCutSettings::default());

        queue.scan_waiting_videos_as(hard_cuts);
        assert_eq!(
            queue.job(ids[0]).unwrap().scan_kind(),
            ScanKind::BlackScreens
        );
        assert_eq!(queue.job(ids[1]).unwrap().scan_kind(), hard_cuts);
        assert_eq!(
            queue.start_next_scan().unwrap().depth,
            ScanDepth::WithHardCuts
        );
    }

    #[test]
    fn a_scanned_video_goes_to_review_with_a_scene_on_each_side_of_every_cut() {
        let (queue, id) = queue_at(Stage::Reviewing);
        let review = queue.review(id).unwrap();
        assert_eq!(review.scene_count(), 3);
        assert_eq!(review.accepted_cuts().as_slice(), [20.0, 60.0]);
        assert_eq!(review.scan_summary(), "Found 0 black screens.");
    }

    #[test]
    fn a_review_built_on_a_stored_scan_says_so() {
        let (mut queue, id) = queue_at(Stage::Scanning);
        let stored = SourcedDetection {
            source: DetectionSource::StoredScan,
            ..two_cuts()
        };
        queue.scan_finished(id, stored).unwrap();

        assert_eq!(
            queue.review(id).unwrap().scan_summary(),
            "Found 0 black screens. Using the scan from last time."
        );
    }

    #[test]
    fn what_the_scan_could_not_decode_reaches_the_review() {
        let (mut queue, id) = queue_at(Stage::Scanning);
        let stretch = UndecodedFrames {
            from_seconds: 12.0,
            missing_frames: 4,
            stretch_frames: 100,
        };
        let scan = SourcedDetection {
            undecoded: vec![stretch],
            ..two_cuts()
        };
        queue.scan_finished(id, scan).unwrap();

        assert_eq!(queue.review(id).unwrap().undecoded(), [stretch]);
    }

    #[test]
    fn a_hard_cut_scan_starts_the_review_from_those_cuts_too() {
        let (mut queue, id) = queue_at(Stage::Waiting);
        let settings = HardCutSettings {
            min_scene_seconds: 10.0,
            ..HardCutSettings::default()
        };
        queue.scan_waiting_videos_as(ScanKind::HardCuts(settings));
        queue.start_scan(id).unwrap();

        let mut scan = two_cuts();
        scan.detection.hard_cut_candidates = Some(vec![HardCutCandidate {
            time: 40.0,
            strength: u8::MAX,
        }]);
        queue.scan_finished(id, scan).unwrap();

        assert_eq!(
            queue.review(id).unwrap().accepted_cuts().as_slice(),
            [20.0, 40.0, 60.0]
        );
    }

    #[test]
    fn a_scan_with_a_cut_outside_the_video_fails_instead_of_being_reviewed() {
        let (mut queue, id) = queue_at(Stage::Scanning);
        queue
            .scan_finished(id, scan_suggesting(vec![20.0, 140.0]))
            .unwrap();

        let JobStatus::Failed { failure, review } = queue.job(id).unwrap().status() else {
            panic!("a cut beyond the end of the video must not reach the review");
        };
        assert!(
            failure.plain_words.contains("not inside the video"),
            "{failure:?}"
        );
        assert!(
            failure.details.as_deref().unwrap().contains("140"),
            "{failure:?}"
        );
        assert!(review.is_none());
    }

    #[test]
    fn a_scan_result_is_refused_for_a_video_that_is_not_being_scanned() {
        assert_refused_elsewhere(Transition::ScanFinished, &[Stage::Scanning], |queue, id| {
            queue.scan_finished(id, two_cuts())
        });
    }

    #[test]
    fn a_review_only_begins_on_a_video_that_was_being_scanned() {
        assert_refused_elsewhere(Transition::BeginReview, &[Stage::Scanning], |queue, id| {
            queue.begin_review(id, test_sessions::five_scenes())
        });

        let (mut queue, id) = queue_at(Stage::Scanning);
        queue
            .begin_review(id, test_sessions::five_scenes())
            .unwrap();
        assert_eq!(queue.review(id).unwrap().scene_count(), 5);
    }

    #[test]
    fn skipping_keeps_the_review_and_shows_the_next_video_that_needs_attention() {
        let (mut queue, ids) = queue_with(3);
        bring_to_review(&mut queue, ids[0]);
        bring_to_review(&mut queue, ids[1]);
        queue.show(ids[1]).unwrap();

        queue.skip(ids[1]).unwrap();
        assert_eq!(stage(&queue, ids[1]), Stage::Skipped);
        // The video after it is still waiting, so that one comes first.
        assert_eq!(queue.current(), Some(ids[2]));
        assert!(queue.review(ids[1]).is_none());
    }

    #[test]
    fn with_nothing_later_to_do_the_queue_moves_on_to_an_earlier_video() {
        let (mut queue, ids) = queue_with(2);
        bring_to_review(&mut queue, ids[0]);
        bring_to_review(&mut queue, ids[1]);
        queue.show(ids[1]).unwrap();

        queue.skip(ids[1]).unwrap();
        assert_eq!(queue.current(), Some(ids[0]));
    }

    #[test]
    fn with_nothing_else_to_do_the_skipped_video_stays_on_screen() {
        let (queue, id) = queue_at(Stage::Skipped);
        assert_eq!(queue.current(), Some(id));
    }

    #[test]
    fn only_a_video_in_review_can_be_skipped() {
        assert_refused_elsewhere(Transition::Skip, &[Stage::Reviewing], Queue::skip);
    }

    #[test]
    fn a_skipped_video_is_reviewed_after_all_exactly_as_it_was_left() {
        let (mut queue, id) = queue_at(Stage::Reviewing);
        let merge_the_first_two =
            ReviewRequest::MergeScenes(SceneIndex::new(0)..=SceneIndex::new(1));
        queue
            .review_mut(id)
            .unwrap()
            .apply(merge_the_first_two, &test_sessions::nothing_in_the_way);

        queue.skip(id).unwrap();
        queue.review_after_all(id).unwrap();
        assert_eq!(queue.review(id).unwrap().accepted_cuts().as_slice(), [60.0]);
        assert!(queue.review(id).unwrap().can_undo());
    }

    #[test]
    fn only_a_skipped_video_can_be_reviewed_after_all() {
        assert_refused_elsewhere(
            Transition::ReviewAfterAll,
            &[Stage::Skipped],
            Queue::review_after_all,
        );
    }

    #[test]
    fn scanning_again_for_black_screens_can_ignore_the_stored_scan() {
        let (mut queue, id) = queue_at(Stage::Reviewing);
        scan_for_black_screens_again(&mut queue, id).unwrap();

        let order = queue.start_next_scan().unwrap();
        assert_eq!(order.job, id);
        assert_eq!(order.cache_use, CacheUse::ScanFresh);
        assert_eq!(order.depth, ScanDepth::BlackScreens);
    }

    #[test]
    fn scanning_again_for_hard_cuts_can_reuse_what_is_stored() {
        let (mut queue, id) = queue_at(Stage::Reviewing);
        let hard_cuts = ScanKind::HardCuts(HardCutSettings::default());
        queue
            .rescan(id, hard_cuts, CacheUse::ReuseStoredScan)
            .unwrap();

        assert_eq!(queue.job(id).unwrap().scan_kind(), hard_cuts);
        let order = queue.start_next_scan().unwrap();
        assert_eq!(order.cache_use, CacheUse::ReuseStoredScan);
        assert_eq!(order.depth, ScanDepth::WithHardCuts);
    }

    #[test]
    fn scanning_again_after_a_hard_cut_scan_goes_back_to_black_screens() {
        let (mut queue, id) = queue_at(Stage::Reviewing);
        let hard_cuts = ScanKind::HardCuts(HardCutSettings::default());
        queue
            .rescan(id, hard_cuts, CacheUse::ReuseStoredScan)
            .unwrap();
        bring_to_review(&mut queue, id);

        scan_for_black_screens_again(&mut queue, id).unwrap();
        assert_eq!(queue.job(id).unwrap().scan_kind(), ScanKind::BlackScreens);
    }

    #[test]
    fn only_a_video_in_review_can_be_scanned_again() {
        assert_refused_elsewhere(
            Transition::Rescan,
            &[Stage::Reviewing],
            scan_for_black_screens_again,
        );
    }

    #[test]
    fn a_split_takes_the_cuts_of_the_review_and_moves_on() {
        let (mut queue, ids) = queue_with(2);
        bring_to_review(&mut queue, ids[0]);

        let order = queue.start_export(ids[0]).unwrap();
        assert_eq!(order.job, ids[0]);
        assert_eq!(&*order.video, Path::new("video_0.mp4"));
        assert_eq!(order.scenes.cuts().as_slice(), [20.0, 60.0]);
        assert!(!order.scenes.has_dropped_scenes());
        assert_eq!(stage(&queue, ids[0]), Stage::Exporting);
        assert_eq!(queue.current(), Some(ids[1]));
    }

    #[test]
    fn a_split_leaves_out_the_scenes_the_person_dropped() {
        let (mut queue, ids) = queue_with(1);
        bring_to_review(&mut queue, ids[0]);
        let nothing_in_the_way = test_sessions::nothing_in_the_way;
        let middle_scene = SceneIndex::new(1);
        queue
            .review_mut(ids[0])
            .unwrap()
            .apply(ReviewRequest::DropScene(middle_scene), &nothing_in_the_way);

        let order = queue.start_export(ids[0]).unwrap();
        assert_eq!(order.scenes.cuts().as_slice(), [20.0, 60.0]);
        assert_eq!(order.scenes.dropped_count(), 1);
        assert_eq!(order.scenes.kept_scenes().len(), 2);
    }

    #[test]
    fn a_video_being_split_is_not_in_review_but_keeps_its_review() {
        let (queue, id) = queue_at(Stage::Exporting);
        // Nothing can be asked of the review while the split runs.
        assert!(queue.review(id).is_none());
        let kept = queue.job(id).unwrap().status().kept_review().unwrap();
        assert_eq!(kept.accepted_cuts().as_slice(), [20.0, 60.0]);
    }

    #[test]
    fn only_videos_that_were_reviewed_keep_a_review() {
        for (at, keeps_a_review) in [
            (Stage::Waiting, false),
            (Stage::Scanning, false),
            (Stage::Reviewing, true),
            (Stage::ReadyToSplit, true),
            (Stage::Exporting, true),
            (Stage::Finished, false),
            (Stage::Skipped, true),
            // `queue_at` fails this one during its scan.
            (Stage::Failed, false),
        ] {
            let (queue, id) = queue_at(at);
            let kept = queue.job(id).unwrap().status().kept_review();
            assert_eq!(kept.is_some(), keeps_a_review, "{at:?}");
        }
    }

    #[test]
    fn only_a_video_in_review_can_be_split() {
        assert_refused_elsewhere(Transition::StartExport, &[Stage::Reviewing], |queue, id| {
            queue.start_export(id).map(|_order| ())
        });
    }

    #[test]
    fn a_split_that_worked_is_finished_with_its_report() {
        let (queue, id) = queue_at(Stage::Finished);
        let JobStatus::Finished(report) = queue.job(id).unwrap().status() else {
            panic!("the split has reported back");
        };
        assert!(report.is_clean());
    }

    #[test]
    fn a_report_is_refused_for_a_video_that_is_not_being_split() {
        assert_refused_elsewhere(Transition::Finished, &[Stage::Exporting], |queue, id| {
            queue.finished(id, report())
        });
    }

    #[test]
    fn a_reviewed_video_can_wait_to_be_split_and_the_next_one_is_shown() {
        let (mut queue, ids) = queue_with(2);
        bring_to_review(&mut queue, ids[0]);

        queue.mark_ready(ids[0]).unwrap();
        assert_eq!(stage(&queue, ids[0]), Stage::ReadyToSplit);
        assert_eq!(queue.ready_jobs(), vec![ids[0]]);
        assert_eq!(queue.current(), Some(ids[1]));
        // Nothing can be asked of the review while it waits.
        assert!(queue.review(ids[0]).is_none());
        let ready = queue.ready_review(ids[0]).unwrap();
        assert_eq!(ready.accepted_cuts().as_slice(), [20.0, 60.0]);
    }

    #[test]
    fn only_a_video_in_review_can_be_marked_ready() {
        assert_refused_elsewhere(
            Transition::MarkReady,
            &[Stage::Reviewing],
            Queue::mark_ready,
        );
    }

    #[test]
    fn the_ready_videos_are_listed_in_the_order_of_the_list() {
        let (mut queue, ids) = queue_with(3);
        for &id in &ids {
            bring_to_review(&mut queue, id);
        }
        queue.mark_ready(ids[2]).unwrap();
        queue.mark_ready(ids[0]).unwrap();

        assert_eq!(queue.ready_jobs(), vec![ids[0], ids[2]]);
        assert!(queue.ready_review(ids[1]).is_none());
    }

    #[test]
    fn a_ready_video_taken_up_again_is_in_review_as_it_was_and_no_longer_ready() {
        let (mut queue, id) = queue_with_an_edited_review();
        queue.mark_ready(id).unwrap();

        queue.edit_again(id).unwrap();
        assert_eq!(stage(&queue, id), Stage::Reviewing);
        assert!(queue.ready_jobs().is_empty());
        let review = queue.review(id).unwrap();
        assert_eq!(review.accepted_cuts().as_slice(), [60.0]);
        assert!(review.can_undo());
    }

    #[test]
    fn only_a_ready_video_can_be_edited_again() {
        assert_refused_elsewhere(
            Transition::EditAgain,
            &[Stage::ReadyToSplit],
            Queue::edit_again,
        );
    }

    #[test]
    fn leave_to_replace_earlier_files_lasts_while_ready_and_ends_with_more_edits() {
        let (mut queue, id) = queue_at(Stage::Reviewing);
        let review = queue.review_mut(id).unwrap();
        review.apply(ReviewRequest::SplitLater, &earlier_files_in_the_way);
        let agreed = ReviewRequest::ReplaceEarlierFiles {
            scene_dir: test_sessions::scene_dir(),
        };
        review.apply(agreed, &earlier_files_in_the_way);
        queue.mark_ready(id).unwrap();
        assert!(queue
            .ready_review(id)
            .unwrap()
            .may_split_over(&earlier_files_in_the_way()));

        queue.edit_again(id).unwrap();
        assert!(!queue
            .review(id)
            .unwrap()
            .may_split_over(&earlier_files_in_the_way()));
    }

    #[test]
    fn the_split_of_a_ready_video_takes_its_cuts_and_leaves_what_is_shown() {
        let (mut queue, ids) = queue_with(3);
        bring_to_review(&mut queue, ids[0]);
        bring_to_review(&mut queue, ids[1]);
        queue.mark_ready(ids[0]).unwrap();
        assert_eq!(queue.current(), Some(ids[1]));

        let order = queue.start_ready_export(ids[0]).unwrap();
        assert_eq!(order.job, ids[0]);
        assert_eq!(&*order.video, Path::new("video_0.mp4"));
        assert_eq!(order.scenes.cuts().as_slice(), [20.0, 60.0]);
        assert_eq!(stage(&queue, ids[0]), Stage::Exporting);
        // The person is reviewing the second video and stays there.
        assert_eq!(queue.current(), Some(ids[1]));
    }

    #[test]
    fn only_a_ready_video_is_split_with_the_ready_videos() {
        assert_refused_elsewhere(
            Transition::StartReadyExport,
            &[Stage::ReadyToSplit],
            |queue, id| queue.start_ready_export(id).map(|_order| ()),
        );
    }

    #[test]
    fn a_ready_video_whose_split_fails_keeps_its_review() {
        let (mut queue, id) = queue_with_an_edited_review();
        queue.mark_ready(id).unwrap();
        queue.start_ready_export(id).unwrap();
        queue.failed(id, failure()).unwrap();

        queue.back_to_review(id).unwrap();
        assert_eq!(queue.review(id).unwrap().accepted_cuts().as_slice(), [60.0]);
    }

    #[test]
    fn a_ready_video_whose_split_could_not_be_started_keeps_its_review() {
        let (mut queue, id) = queue_with_an_edited_review();
        queue.mark_ready(id).unwrap();
        queue.failed(id, failure()).unwrap();

        queue.back_to_review(id).unwrap();
        assert_eq!(queue.review(id).unwrap().accepted_cuts().as_slice(), [60.0]);
    }

    #[test]
    fn a_ready_video_is_not_one_that_still_needs_attention() {
        let (mut queue, ids) = queue_with(3);
        for &id in &ids {
            bring_to_review(&mut queue, id);
        }
        queue.mark_ready(ids[1]).unwrap();
        queue.show(ids[0]).unwrap();

        // Skipping the first moves past the ready one to the third.
        queue.skip(ids[0]).unwrap();
        assert_eq!(queue.current(), Some(ids[2]));
    }

    #[test]
    fn a_scan_a_review_and_a_split_can_fail() {
        for from in [
            Stage::Scanning,
            Stage::Reviewing,
            Stage::ReadyToSplit,
            Stage::Exporting,
        ] {
            let (mut queue, id) = queue_at(from);
            queue.failed(id, failure()).unwrap();

            let JobStatus::Failed { failure: why, .. } = queue.job(id).unwrap().status() else {
                panic!("the video failed from {from:?}");
            };
            assert_eq!(*why, failure());
        }
    }

    /// A video in review whose first two scenes the person merged.
    fn queue_with_an_edited_review() -> (Queue, JobId) {
        let (mut queue, id) = queue_at(Stage::Reviewing);
        let merge_the_first_two =
            ReviewRequest::MergeScenes(SceneIndex::new(0)..=SceneIndex::new(1));
        queue
            .review_mut(id)
            .unwrap()
            .apply(merge_the_first_two, &test_sessions::nothing_in_the_way);
        (queue, id)
    }

    #[test]
    fn a_split_that_fails_gives_the_review_back_as_it_was() {
        let (mut queue, id) = queue_with_an_edited_review();
        queue.start_export(id).unwrap();
        queue.failed(id, failure()).unwrap();
        assert_eq!(stage(&queue, id), Stage::Failed);

        queue.back_to_review(id).unwrap();
        let review = queue.review(id).unwrap();
        assert_eq!(review.accepted_cuts().as_slice(), [60.0]);
        // The merge can still be undone.
        assert!(review.can_undo());
    }

    #[test]
    fn a_split_that_crashes_gives_the_review_back_too() {
        let (mut queue, id) = queue_with_an_edited_review();
        queue.start_export(id).unwrap();
        queue.crashed(id).unwrap();

        queue.back_to_review(id).unwrap();
        assert_eq!(queue.review(id).unwrap().accepted_cuts().as_slice(), [60.0]);
    }

    #[test]
    fn a_split_that_could_not_be_started_keeps_the_review() {
        let (mut queue, id) = queue_with_an_edited_review();
        queue.failed(id, failure()).unwrap();

        queue.back_to_review(id).unwrap();
        assert_eq!(queue.review(id).unwrap().accepted_cuts().as_slice(), [60.0]);
    }

    #[test]
    fn a_video_whose_scan_failed_has_no_review_to_go_back_to() {
        let (mut queue, id) = queue_at(Stage::Failed);
        assert_eq!(queue.back_to_review(id), Err(Refused::NoReviewKept(id)));
        assert_eq!(stage(&queue, id), Stage::Failed);
    }

    #[test]
    fn only_a_failed_video_goes_back_to_its_review() {
        assert_refused_elsewhere(
            Transition::BackToReview,
            &[Stage::Failed],
            Queue::back_to_review,
        );
    }

    #[test]
    fn trying_again_after_a_failed_split_scans_afresh_and_drops_the_review() {
        let (mut queue, id) = queue_with_an_edited_review();
        queue.start_export(id).unwrap();
        queue.failed(id, failure()).unwrap();

        queue.try_again(id).unwrap();
        assert_eq!(stage(&queue, id), Stage::Waiting);
        assert!(queue.job(id).unwrap().status().kept_review().is_none());
    }

    fn earlier_files_in_the_way() -> Result<crate::review_session::EarlierSceneFiles, String> {
        Ok(crate::review_session::EarlierSceneFiles {
            scene_dir: test_sessions::scene_dir(),
            splits: crate::review_session::EarlierSceneFiles::a_split_into(3),
        })
    }

    /// A video in review that was asked to be split and warned instead.
    fn queue_with_a_warned_review() -> (Queue, JobId) {
        let (mut queue, id) = queue_at(Stage::Reviewing);
        let review = queue.review_mut(id).unwrap();
        review.apply(ReviewRequest::Split, &earlier_files_in_the_way);
        assert!(matches!(
            review.export_obstacle(),
            Some(ExportObstacle::EarlierSceneFiles(_))
        ));
        (queue, id)
    }

    #[test]
    fn a_warning_about_earlier_files_goes_from_every_review_when_scenes_go_elsewhere() {
        let (mut queue, id) = queue_with_a_warned_review();
        queue.forget_export_obstacles();
        assert_eq!(queue.review(id).unwrap().export_obstacle(), None);

        // A review that was put aside is told as well.
        let (mut queue, id) = queue_with_a_warned_review();
        queue.skip(id).unwrap();
        queue.forget_export_obstacles();
        queue.review_after_all(id).unwrap();
        assert_eq!(queue.review(id).unwrap().export_obstacle(), None);
    }

    #[test]
    fn a_review_taken_up_after_a_failed_split_asks_about_earlier_files_afresh() {
        let (mut queue, id) = queue_with_a_warned_review();
        queue.failed(id, failure()).unwrap();

        queue.back_to_review(id).unwrap();
        assert_eq!(queue.review(id).unwrap().export_obstacle(), None);
    }

    #[test]
    fn a_failed_exact_cut_says_the_original_is_untouched_and_offers_the_normal_cut() {
        let error = anyhow::Error::new(ExactCutError::EncoderMissing);
        let failure = Failure::from_split_error(&error);

        assert_eq!(
            failure.plain_words,
            ExactCutError::EncoderMissing.in_plain_words()
        );
        assert!(failure
            .plain_words
            .contains("The original was not changed."));
        assert!(failure.plain_words.contains("turn off"));
        assert_eq!(
            failure.details.as_deref(),
            Some("this ffmpeg has no libx264 encoder")
        );
    }

    #[test]
    fn any_other_failed_split_is_told_as_before() {
        let error = anyhow::anyhow!("the disk is full");
        assert_eq!(
            Failure::from_split_error(&error),
            Failure::could_not_split("the disk is full".to_owned())
        );
    }

    fn timed_out() -> FfmpegError {
        FfmpegError::TimedOut {
            program: Program::Ffmpeg,
            limit: TimeLimit::Silence(Duration::from_secs(60)),
        }
    }

    fn could_not_start() -> FfmpegError {
        FfmpegError::CouldNotStart {
            program: Program::Ffprobe,
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        }
    }

    fn lost_contact() -> FfmpegError {
        FfmpegError::LostContact {
            program: Program::Ffmpeg,
            source: std::io::Error::from(std::io::ErrorKind::BrokenPipe),
        }
    }

    fn output_not_understood() -> FfmpegError {
        FfmpegError::OutputNotUnderstood {
            program: Program::Ffprobe,
            detail: "no packets".to_owned(),
        }
    }

    #[test]
    fn a_scan_that_ran_out_of_time_is_not_called_a_damaged_file() {
        let error = anyhow::Error::new(timed_out()).context("could not scan film.mp4");
        let failure = Failure::from_scan_error(&error);

        assert!(
            failure.plain_words.contains("could not be read in time"),
            "{failure:?}"
        );
        assert!(!failure.plain_words.contains("damaged"), "{failure:?}");
        // The technical reason is still there to copy, with its context.
        let details = failure.details.unwrap();
        assert!(details.starts_with("could not scan film.mp4"), "{details}");
    }

    #[test]
    fn a_scan_whose_tools_did_not_start_says_so() {
        let failure = Failure::from_scan_error(&anyhow::Error::new(could_not_start()));
        assert!(
            failure.plain_words.contains("could not be started"),
            "{failure:?}"
        );
        assert!(!failure.plain_words.contains("damaged"), "{failure:?}");
    }

    #[test]
    fn a_scan_the_tools_could_not_make_sense_of_is_an_unreadable_video() {
        for error in [lost_contact(), output_not_understood()] {
            let error = anyhow::Error::new(error);
            let details = format!("{error:#}");
            assert_eq!(
                Failure::from_scan_error(&error),
                Failure::unreadable_video(details)
            );
        }
    }

    #[test]
    fn a_scan_error_that_did_not_come_from_the_tools_is_an_unreadable_video() {
        let error = anyhow::anyhow!("the file has no video stream");
        assert_eq!(
            Failure::from_scan_error(&error),
            Failure::unreadable_video("the file has no video stream".to_owned())
        );
    }

    #[test]
    fn the_cause_of_a_failed_scan_is_found_under_layers_of_context() {
        let error = anyhow::Error::new(timed_out())
            .context("could not decode a stretch")
            .context("could not scan film.mp4");
        assert!(Failure::from_scan_error(&error)
            .plain_words
            .contains("in time"));
    }

    #[test]
    fn a_video_nobody_is_working_on_cannot_fail() {
        assert_refused_elsewhere(
            Transition::Failed,
            &[
                Stage::Scanning,
                Stage::Reviewing,
                Stage::ReadyToSplit,
                Stage::Exporting,
            ],
            |queue, id| queue.failed(id, failure()),
        );
    }

    #[test]
    fn a_crash_fails_the_video_and_frees_the_next_scan() {
        let (mut queue, ids) = queue_with(2);
        queue.start_scan(ids[0]).unwrap();

        queue.crashed(ids[0]).unwrap();
        let JobStatus::Failed { failure: why, .. } = queue.job(ids[0]).unwrap().status() else {
            panic!("a crash fails the video");
        };
        assert!(
            why.plain_words.starts_with("Something went wrong"),
            "{why:?}"
        );
        assert_eq!(why.details, None);
        // Without this the video behind it would wait for ever.
        assert_eq!(queue.start_next_scan().unwrap().job, ids[1]);
    }

    #[test]
    fn only_background_work_can_crash() {
        assert_refused_elsewhere(
            Transition::Crashed,
            &[Stage::Scanning, Stage::Exporting],
            Queue::crashed,
        );
    }

    #[test]
    fn a_failed_video_can_be_tried_again_from_the_scan() {
        let (mut queue, id) = queue_at(Stage::Failed);
        queue.try_again(id).unwrap();

        assert_eq!(stage(&queue, id), Stage::Waiting);
        assert_eq!(queue.start_next_scan().unwrap().job, id);
    }

    #[test]
    fn trying_again_after_a_fresh_scan_failed_may_use_a_stored_scan() {
        let (mut queue, id) = queue_at(Stage::Reviewing);
        scan_for_black_screens_again(&mut queue, id).unwrap();
        queue.start_scan(id).unwrap();
        queue.failed(id, failure()).unwrap();

        queue.try_again(id).unwrap();
        assert_eq!(
            queue.start_next_scan().unwrap().cache_use,
            CacheUse::ReuseStoredScan
        );
    }

    #[test]
    fn only_a_failed_video_can_be_tried_again() {
        assert_refused_elsewhere(Transition::TryAgain, &[Stage::Failed], Queue::try_again);
    }

    #[test]
    fn a_refusal_says_what_was_asked_of_which_video() {
        let (mut queue, id) = queue_at(Stage::Finished);
        let refusal = queue.skip(id).unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "skipping is not possible for video 0, which is finished"
        );
    }

    #[test]
    fn every_stage_and_every_change_has_words_of_its_own() {
        let stages: Vec<String> = EVERY_STAGE.iter().map(Stage::to_string).collect();
        let changes: Vec<String> = [
            Transition::StartScan,
            Transition::ScanProgressed,
            Transition::ScanFinished,
            Transition::BeginReview,
            Transition::Skip,
            Transition::ReviewAfterAll,
            Transition::Rescan,
            Transition::MarkReady,
            Transition::EditAgain,
            Transition::StartExport,
            Transition::StartReadyExport,
            Transition::Finished,
            Transition::Failed,
            Transition::Crashed,
            Transition::TryAgain,
            Transition::BackToReview,
        ]
        .iter()
        .map(Transition::to_string)
        .collect();
        for words in [stages, changes] {
            let mut distinct = words.clone();
            distinct.sort();
            distinct.dedup();
            assert_eq!(distinct.len(), words.len(), "{words:?}");
            // Written for a sentence, not copied from the name in the code.
            assert!(
                words
                    .iter()
                    .all(|words| *words == words.to_lowercase() && !words.is_empty()),
                "{words:?}"
            );
        }
    }
}
