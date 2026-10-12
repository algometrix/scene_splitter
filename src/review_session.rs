//! The review of one video, without its screen: the split plan as the person
//! has edited it, what they have selected, and the questions that stand
//! between them and a split or a new scan.
//!
//! The review screen draws a session and reports what the person asked for
//! as `ReviewRequest`s. `ReviewSession::apply` is the one place those change
//! anything.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cache::{DetectionSource, SourcedDetection};
use crate::chapters::ChapterSplits;
use crate::detect::ScanKind;
use crate::folder_settings::DetectionSettings;
use crate::plan::{
    AddCutError, CutChoice, CutIndex, PlanSnapshot, SceneIndex, SceneLayout, SegmentIndex,
    SplitPlan,
};
use crate::scan::UndecodedFrames;
use crate::scenes::CutTimes;
use crate::selection::{ClickKind, SceneSelection};
use crate::settings::SettingsChange;
use crate::timecode::{format_timestamp, parse_timestamp, TimeSpan};

/// What looking for the scene files of an earlier split found.
#[derive(Debug, Clone, PartialEq)]
pub struct EarlierSceneFiles {
    /// The folder the scenes of this video would be written to now.
    pub scene_dir: PathBuf,
    /// How many scene files of this video are in it already.
    pub file_count: usize,
}

/// Why the split has not started after the person asked for it.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportObstacle {
    /// Scene files of the same names exist in the folder the scenes go to
    /// and would be replaced. The warning is about that folder only.
    EarlierSceneFiles(EarlierSceneFiles),
    /// The folder the scenes go to could not be looked into.
    CouldNotCheck(String),
}

/// How asking for a new split went, for the screens to say.
#[derive(Debug, PartialEq)]
pub enum SplitFeedback {
    /// A split was added at this time of the video.
    Added(f64),
    Refused(AddCutError),
    /// What was typed is not a time.
    NotATime,
}

impl SplitFeedback {
    /// Whether nothing was added, so the screens can tell it apart from a
    /// confirmation.
    pub fn is_refusal(&self) -> bool {
        match self {
            Self::Added(_) => false,
            Self::Refused(_) | Self::NotATime => true,
        }
    }

    pub fn in_words(&self) -> String {
        match self {
            Self::Added(cut_time) => format!(
                "Split at {}, the nearest point where the video can be cut.",
                format_timestamp(*cut_time)
            ),
            Self::Refused(why) => why.to_string(),
            Self::NotATime => "Type a time like 1:03:12.".to_owned(),
        }
    }
}

/// What the person asked for on the review screen. Scenes, segments and cuts
/// are named by their place in the plan as the screen drew it.
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewRequest {
    ClickScene {
        scene: SceneIndex,
        kind: ClickKind,
    },
    SelectAllScenes,
    ClearSelection,
    MergeSelected,
    MergeScenes(RangeInclusive<SceneIndex>),
    /// Take the selected merged scenes apart again.
    TakeSelectedApart,
    /// Make one part of a merged scene a scene of its own.
    MoveOutSegment(SegmentIndex),
    /// Split a merged scene in two again at one of its merged cuts.
    SeparateAt(CutIndex),
    /// What now stands in the box for the time of a new split.
    TypeNewSplitTime(String),
    /// Add a split at the time that stands in the box.
    AddTypedSplit,
    Undo,
    ResetScenes,
    /// Join the scenes on both sides of every split that comes from a
    /// chapter mark of the file.
    LeaveOutChapterSplits,
    Split,
    /// The scenes are right, but split this video later, together with the
    /// other videos that are ready.
    SplitLater,
    /// Go on although the scene files of an earlier split in this folder
    /// will be replaced. The folder is the one the warning named. What goes
    /// on is what was asked for when the warning came: the split, or
    /// putting the video with those that are ready.
    ReplaceEarlierFiles {
        scene_dir: PathBuf,
    },
    Skip,
    /// Scan the video again, as this kind of scan.
    Rescan(ScanKind),
    /// Scan again although that throws away the edits.
    RescanAnyway,
    /// Do not scan again after all.
    KeepEdits,
    /// Play the video from this time.
    PlayFrom(f64),
    /// Change a setting that is offered on the review screen. Settings are
    /// not part of any one review, so this is left to the app.
    ChangeSettings(SettingsChange),
    /// Change the detection settings this video is scanned with: those of
    /// its folder when the folder has its own, the general ones otherwise.
    /// Which of the two it is, is for the app to say.
    ChangeDetection(DetectionSettings),
    /// Keep the detection settings in use now for this video's folder.
    RememberForFolder,
    /// Forget the detection settings of this video's folder.
    UseGeneralSettings,
}

/// What a request leaves for the rest of the app to do. The session itself
/// can neither write files, nor scan, nor play, nor keep settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FollowUp {
    Export,
    /// Put the video with those that are ready to be split.
    MarkReady,
    Skip,
    Rescan(ScanKind),
    PlayFrom(f64),
    ChangeSettings(SettingsChange),
    ChangeDetection(DetectionSettings),
    RememberForFolder,
    UseGeneralSettings,
}

/// When the person wants the video they have reviewed to be split.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SplitTiming {
    Now,
    /// With the other videos that are ready, when the person says so.
    WithTheReadyVideos,
}

impl SplitTiming {
    /// What is left for the app to do once nothing stands in the way.
    fn follow_up(self) -> FollowUp {
        match self {
            Self::Now => FollowUp::Export,
            Self::WithTheReadyVideos => FollowUp::MarkReady,
        }
    }
}

/// Looks where the scenes of this video would go now, for scene files an
/// earlier split left there. It reads the disk and depends on the settings,
/// so the session is handed it instead of looking itself.
pub type LookForEarlierSceneFiles<'a> = &'a dyn Fn() -> Result<EarlierSceneFiles, String>;

#[derive(Debug)]
pub struct ReviewSession {
    video: Arc<Path>,
    plan: SplitPlan,
    /// The plan as it was before each change, newest last, for undo.
    undo_history: Vec<PlanSnapshot>,
    selection: SceneSelection,
    scan_summary: String,
    /// How the scan read the video, for the curious.
    scan_details: String,
    /// Stretches the scan could not decode, where it may have missed a
    /// scene change.
    undecoded: Vec<UndecodedFrames>,
    /// How long one frame of the video is shown, on average.
    frame_seconds: f64,
    /// What stands in the box for the time of a new split.
    new_split_time: String,
    /// How the last split typed into the box went, once one was tried.
    new_split_feedback: Option<SplitFeedback>,
    export_obstacle: Option<ExportObstacle>,
    /// What the person asked for when `export_obstacle` came up, which is
    /// what goes on once they agree to replace the files.
    split_asked_for: SplitTiming,
    /// The folder whose earlier scene files the person agreed to replace.
    /// A video that waits to be split with the others needs this later, when
    /// its turn comes and nobody is there to ask.
    replacing_agreed_in: Option<PathBuf>,
    /// A scan the person asked for that would throw away their edits, until
    /// they confirm or cancel it.
    rescan_awaiting_consent: Option<ScanKind>,
    /// The titles the file's chapter marks give to scenes.
    chapter_splits: ChapterSplits,
}

impl ReviewSession {
    /// `cuts` are the cuts to start from, which the caller picks from the
    /// scan: its black-screen cuts, or those plus its hard cuts.
    pub fn new(video: Arc<Path>, scan: SourcedDetection, cuts: CutTimes) -> Self {
        let SourcedDetection {
            detection,
            source,
            undecoded,
        } = scan;
        let video_length = detection.video.seconds();
        let frame_seconds = video_length / detection.frame_count.max(1) as f64;
        let black_screens = counted(detection.black_screens.len(), "black screen");
        let hard_cuts = cuts.len().saturating_sub(detection.cuts.len());
        let found = if hard_cuts > 0 {
            format!(
                "Found {}: {black_screens}, {}.",
                counted(cuts.len(), "scene change"),
                counted(hard_cuts, "sudden picture change")
            )
        } else {
            format!("Found {black_screens}.")
        };
        let chapter_splits = detection.chapter_splits(cuts.as_slice());
        let found = match splits_from_chapters_in_words(chapter_splits.cuts.len()) {
            Some(from_chapters) => format!("{found} {from_chapters}"),
            None => found,
        };
        let scan_summary = match source {
            DetectionSource::FreshScan => found,
            DetectionSource::StoredScan => format!("{found} Using the scan from last time."),
        };
        let decoded = format!(
            "{} of {} frames were decoded to find them.",
            detection.decoded_frames, detection.frame_count
        );
        let chapter_mark_count = detection.chapter_marks().len();
        let scan_details = if chapter_mark_count > 0 {
            format!(
                "{decoded} The file has {}.",
                counted(chapter_mark_count, "chapter mark")
            )
        } else {
            decoded
        };
        let mut plan = SplitPlan::from_detected_cuts(&cuts, detection.keyframe_times);
        plan.propose_chapter_cuts(&chapter_splits.cuts);
        Self {
            video,
            plan,
            undo_history: Vec::new(),
            selection: SceneSelection::default(),
            scan_summary,
            scan_details,
            undecoded,
            frame_seconds,
            new_split_time: String::new(),
            new_split_feedback: None,
            export_obstacle: None,
            split_asked_for: SplitTiming::Now,
            replacing_agreed_in: None,
            rescan_awaiting_consent: None,
            chapter_splits,
        }
    }

    /// The title of the chapter that begins where this scene begins, when
    /// the file has chapter marks with titles.
    pub fn chapter_title(&self, scene: TimeSpan) -> Option<&str> {
        self.chapter_splits.title_of_scene_starting_at(scene.start)
    }

    /// How many splits the review has now only because of a chapter mark.
    pub fn chapter_split_count(&self) -> usize {
        self.plan.chapter_split_count()
    }

    /// Whether a split may go ahead given what was `found` where the scenes
    /// go: nothing in the way, or only files the person already agreed to
    /// replace in that same folder.
    pub fn may_split_over(&self, found: &Result<EarlierSceneFiles, String>) -> bool {
        match found {
            Ok(earlier_files) => {
                earlier_files.file_count == 0
                    || self.replacing_agreed_in.as_deref() == Some(&earlier_files.scene_dir)
            }
            Err(_could_not_look) => false,
        }
    }

    pub fn video(&self) -> &Arc<Path> {
        &self.video
    }

    pub fn plan(&self) -> &SplitPlan {
        &self.plan
    }

    pub fn selection(&self) -> &SceneSelection {
        &self.selection
    }

    pub fn scan_summary(&self) -> &str {
        &self.scan_summary
    }

    pub fn scan_details(&self) -> &str {
        &self.scan_details
    }

    /// The stretches of the video the scan could not decode. Scene changes
    /// in them may have been missed, which the person should know.
    pub fn undecoded(&self) -> &[UndecodedFrames] {
        &self.undecoded
    }

    pub fn new_split_time(&self) -> &str {
        &self.new_split_time
    }

    pub fn new_split_feedback(&self) -> Option<&SplitFeedback> {
        self.new_split_feedback.as_ref()
    }

    pub fn export_obstacle(&self) -> Option<&ExportObstacle> {
        self.export_obstacle.as_ref()
    }

    /// A warning about earlier scene files was given for one folder and one
    /// plan, and so was the person's leave to replace them. Whoever changes
    /// where the scenes go calls this, and so does a review that is taken up
    /// again after its split failed or after it waited to be split.
    pub fn forget_export_obstacle(&mut self) {
        self.export_obstacle = None;
        self.replacing_agreed_in = None;
    }

    /// Whether the person still has to say if a scan may throw away their
    /// edits.
    pub fn is_asking_before_rescan(&self) -> bool {
        self.rescan_awaiting_consent.is_some()
    }

    pub fn scene_count(&self) -> usize {
        self.plan.scenes().len()
    }

    /// The time at which the video ends. The player counts time from zero,
    /// so this, and not the length of the video, is how far it can go.
    pub fn video_end(&self) -> f64 {
        self.plan.whole_video().end
    }

    pub fn frame_seconds(&self) -> f64 {
        self.frame_seconds
    }

    /// The keyframe a split asked for at `time` would land on, or `None`
    /// where no split is possible.
    pub fn split_time_near(&self, time: f64) -> Option<f64> {
        self.plan.cut_time_near(time).ok()
    }

    /// The scenes as they stand now, in order, for the player to show.
    pub fn scenes(&self) -> Vec<TimeSpan> {
        self.plan.scenes()
    }

    pub fn accepted_cuts(&self) -> CutTimes {
        self.plan.accepted_cuts()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_history.is_empty()
    }

    /// Whether the person has changed anything a new scan would throw away.
    pub fn is_edited(&self) -> bool {
        self.can_undo() || !self.plan.is_as_detected()
    }

    pub fn has_merged_scene_selected(&self) -> bool {
        self.selected_layouts().iter().any(SceneLayout::is_merged)
    }

    /// Applies everything the person asked for during one frame, in order,
    /// up to the first request that leaves something for the app to do.
    ///
    /// Requests name scenes and cuts by their place in the plan the screen
    /// drew. Once one request has changed the plan, those places may hold
    /// other scenes, so the rest of that frame's requests are dropped.
    pub fn apply_frame(
        &mut self,
        requests: Vec<ReviewRequest>,
        look_for_earlier_scene_files: LookForEarlierSceneFiles,
    ) -> Option<FollowUp> {
        if requests.is_empty() {
            return None;
        }
        let plan_as_drawn = self.plan.snapshot();
        for request in requests {
            let follow_up = self.apply(request, look_for_earlier_scene_files);
            if follow_up.is_some() {
                return follow_up;
            }
            if !self.plan.is_at(&plan_as_drawn) {
                break;
            }
        }
        None
    }

    /// Does what the person asked for, as far as that only concerns this
    /// review, and returns what is left for the app to do.
    pub fn apply(
        &mut self,
        request: ReviewRequest,
        look_for_earlier_scene_files: LookForEarlierSceneFiles,
    ) -> Option<FollowUp> {
        match request {
            ReviewRequest::ClickScene { scene, kind } => self.selection.click(scene, kind),
            ReviewRequest::SelectAllScenes => self.selection.select_all(self.scene_count()),
            ReviewRequest::ClearSelection => self.selection.clear(),
            ReviewRequest::MergeSelected => {
                if let Some(scenes) = self.selection.span_to_merge() {
                    self.merge_scenes(scenes);
                }
            }
            ReviewRequest::MergeScenes(scenes) => self.merge_scenes(scenes),
            ReviewRequest::TakeSelectedApart => self.take_selected_apart(),
            ReviewRequest::MoveOutSegment(segment) => {
                self.edit_plan(|plan| plan.isolate_segment(segment));
            }
            ReviewRequest::SeparateAt(cut) => {
                self.edit_plan(|plan| plan.set_choice(cut, CutChoice::Split));
            }
            ReviewRequest::TypeNewSplitTime(typed) => self.new_split_time = typed,
            ReviewRequest::AddTypedSplit => self.add_typed_split(),
            ReviewRequest::Undo => self.undo(),
            ReviewRequest::ResetScenes => self.edit_plan(SplitPlan::reset_to_detected),
            ReviewRequest::LeaveOutChapterSplits => self.edit_plan(SplitPlan::merge_chapter_cuts),
            ReviewRequest::Split => {
                return self.ask_to_split(look_for_earlier_scene_files(), SplitTiming::Now);
            }
            ReviewRequest::SplitLater => {
                return self.ask_to_split(
                    look_for_earlier_scene_files(),
                    SplitTiming::WithTheReadyVideos,
                );
            }
            ReviewRequest::ReplaceEarlierFiles { scene_dir } => {
                return self.replace_earlier_files(&scene_dir, look_for_earlier_scene_files());
            }
            ReviewRequest::Skip => return Some(FollowUp::Skip),
            ReviewRequest::Rescan(scan_kind) => return self.ask_to_rescan(scan_kind),
            ReviewRequest::RescanAnyway => {
                return self.rescan_awaiting_consent.take().map(FollowUp::Rescan);
            }
            ReviewRequest::KeepEdits => self.rescan_awaiting_consent = None,
            ReviewRequest::PlayFrom(time) => return Some(FollowUp::PlayFrom(time)),
            ReviewRequest::ChangeSettings(change) => {
                return Some(FollowUp::ChangeSettings(change));
            }
            ReviewRequest::ChangeDetection(detection) => {
                return Some(FollowUp::ChangeDetection(detection));
            }
            ReviewRequest::RememberForFolder => return Some(FollowUp::RememberForFolder),
            ReviewRequest::UseGeneralSettings => return Some(FollowUp::UseGeneralSettings),
        }
        None
    }

    /// Splits the scene playing at `time` in two, on the keyframe nearest to
    /// that time, and says what happened. This is what the player's split
    /// button does.
    pub fn split_near(&mut self, time: f64) -> SplitFeedback {
        match self.add_split_near(time) {
            Ok(cut_time) => SplitFeedback::Added(cut_time),
            Err(refusal) => SplitFeedback::Refused(refusal),
        }
    }

    /// Splitting replaces files of the same name, so existing scene files
    /// need a second, explicit confirmation.
    fn ask_to_split(
        &mut self,
        found: Result<EarlierSceneFiles, String>,
        timing: SplitTiming,
    ) -> Option<FollowUp> {
        self.split_asked_for = timing;
        // Leave given earlier was for what was on disk then.
        self.replacing_agreed_in = None;
        self.export_obstacle = match found {
            Ok(nothing_in_the_way) if nothing_in_the_way.file_count == 0 => None,
            Ok(earlier_files) => Some(ExportObstacle::EarlierSceneFiles(earlier_files)),
            Err(problem) => Some(ExportObstacle::CouldNotCheck(problem)),
        };
        match self.export_obstacle {
            None => Some(timing.follow_up()),
            Some(_) => None,
        }
    }

    /// The person agreed to replace the scene files in `confirmed_dir`. That
    /// says nothing about any other folder. If the scenes would go somewhere
    /// else by now, because the setting for it was changed in between, what
    /// is there is looked at and asked about like on a first try.
    fn replace_earlier_files(
        &mut self,
        confirmed_dir: &Path,
        found: Result<EarlierSceneFiles, String>,
    ) -> Option<FollowUp> {
        match found {
            Ok(earlier_files) if earlier_files.scene_dir == confirmed_dir => {
                self.export_obstacle = None;
                self.replacing_agreed_in = Some(earlier_files.scene_dir);
                Some(self.split_asked_for.follow_up())
            }
            found_elsewhere_or_not_at_all => {
                self.ask_to_split(found_elsewhere_or_not_at_all, self.split_asked_for)
            }
        }
    }

    /// A scan replaces the scenes, so edits are not thrown away unasked.
    fn ask_to_rescan(&mut self, scan_kind: ScanKind) -> Option<FollowUp> {
        if self.is_edited() {
            self.rescan_awaiting_consent = Some(scan_kind);
            return None;
        }
        Some(FollowUp::Rescan(scan_kind))
    }

    /// Every change to the plan goes through here, so each one can be undone
    /// and none leaves stale state behind.
    fn edit_plan<Outcome>(&mut self, edit: impl FnOnce(&mut SplitPlan) -> Outcome) -> Outcome {
        let plan_before = self.plan.snapshot();
        let outcome = edit(&mut self.plan);
        if !self.plan.is_at(&plan_before) {
            self.undo_history.push(plan_before);
            self.forget_what_depended_on_the_old_plan();
        }
        outcome
    }

    fn undo(&mut self) {
        let Some(earlier_plan) = self.undo_history.pop() else {
            return;
        };
        self.plan.restore(earlier_plan);
        self.forget_what_depended_on_the_old_plan();
    }

    /// Scene numbers shift when the plan changes, so a selection made
    /// before would point at other scenes. A warning about existing files
    /// was likewise given for the plan as it was.
    fn forget_what_depended_on_the_old_plan(&mut self) {
        self.selection.clear();
        self.forget_export_obstacle();
    }

    fn merge_scenes(&mut self, scenes: RangeInclusive<SceneIndex>) {
        let merged_scene = *scenes.start();
        self.edit_plan(|plan| plan.merge_scenes(scenes));
        self.selection = SceneSelection::only(merged_scene);
    }

    fn take_selected_apart(&mut self) {
        let selected_spans: Vec<TimeSpan> = self
            .selected_layouts()
            .iter()
            .map(|layout| layout.span)
            .collect();
        self.edit_plan(|plan| {
            for span in selected_spans {
                plan.restore_splits_within(span);
            }
        });
    }

    fn add_typed_split(&mut self) {
        let Some(requested_time) = parse_timestamp(&self.new_split_time) else {
            self.new_split_feedback = Some(SplitFeedback::NotATime);
            return;
        };
        let feedback = self.split_near(requested_time);
        if !feedback.is_refusal() {
            self.new_split_time.clear();
        }
        self.new_split_feedback = Some(feedback);
    }

    fn add_split_near(&mut self, time: f64) -> Result<f64, AddCutError> {
        self.edit_plan(|plan| plan.add_cut_near(time))
    }

    fn selected_layouts(&self) -> Vec<SceneLayout> {
        let layouts = self.plan.scene_layouts();
        self.selection
            .scenes()
            .filter_map(|scene| layouts.get(scene.position()).cloned())
            .collect()
    }
}

/// The sentence of the scan summary about chapter marks, or `None` when no
/// split comes from one.
fn splits_from_chapters_in_words(chapter_split_count: usize) -> Option<String> {
    match chapter_split_count {
        0 => None,
        1 => Some("1 more split comes from a chapter mark.".to_owned()),
        several => Some(format!("{several} more splits come from chapter marks.")),
    }
}

/// "1 scene", "3 scenes".
pub fn counted(count: usize, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

#[cfg(test)]
pub mod test_sessions {
    use super::*;
    use crate::detect::Detection;

    /// A 100 second video cut into five scenes of 20 seconds each, with a
    /// keyframe every 10 seconds.
    pub fn five_scenes() -> ReviewSession {
        scenes_of_twenty_seconds(5)
    }

    /// A video cut into `scene_count` scenes of 20 seconds each, with a
    /// keyframe every 10 seconds.
    pub fn scenes_of_twenty_seconds(scene_count: usize) -> ReviewSession {
        fresh_scan_of(scenes_of_twenty_seconds_detected(scene_count))
    }

    /// The review a fresh scan with this result starts.
    pub fn fresh_scan_of(detection: Detection) -> ReviewSession {
        review_of(SourcedDetection {
            detection,
            source: DetectionSource::FreshScan,
            undecoded: Vec::new(),
        })
    }

    /// The review of a scan, starting from its black-screen cuts.
    pub fn review_of(scan: SourcedDetection) -> ReviewSession {
        let cuts = CutTimes::inside(scan.detection.video, scan.detection.cuts.clone()).unwrap();
        let video: Arc<Path> = Arc::from(Path::new("no_such_video.mp4"));
        ReviewSession::new(video, scan, cuts)
    }

    pub fn scenes_of_twenty_seconds_detected(scene_count: usize) -> Detection {
        const SCENE_SECONDS: f64 = 20.0;
        const KEYFRAMES_PER_SCENE: usize = 2;
        const FRAMES_PER_SCENE: usize = 600;
        Detection {
            video: TimeSpan {
                start: 0.0,
                end: scene_count as f64 * SCENE_SECONDS,
            },
            keyframe_times: (0..scene_count * KEYFRAMES_PER_SCENE)
                .map(|keyframe| keyframe as f64 * SCENE_SECONDS / KEYFRAMES_PER_SCENE as f64)
                .collect(),
            frame_count: scene_count * FRAMES_PER_SCENE,
            black_screens: Vec::new(),
            cuts: (1..scene_count)
                .map(|cut| cut as f64 * SCENE_SECONDS)
                .collect(),
            decoded_frames: 0,
            decoded_windows: 0,
            hard_cut_candidates: None,
            chapters: None,
        }
    }

    /// The folder the scenes of the test video go to, unless a test says
    /// otherwise.
    pub fn scene_dir() -> PathBuf {
        PathBuf::from("videos")
    }

    /// No scene files of an earlier split are in the way.
    pub fn nothing_in_the_way() -> Result<EarlierSceneFiles, String> {
        Ok(EarlierSceneFiles {
            scene_dir: scene_dir(),
            file_count: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(position: usize) -> SceneIndex {
        SceneIndex::new(position)
    }

    fn scenes(first: usize, last: usize) -> RangeInclusive<SceneIndex> {
        scene(first)..=scene(last)
    }

    fn click(position: usize, kind: ClickKind) -> ReviewRequest {
        ReviewRequest::ClickScene {
            scene: scene(position),
            kind,
        }
    }

    /// For requests that must not need to look at the disk.
    fn no_disk() -> Result<EarlierSceneFiles, String> {
        panic!("this request has no reason to look for earlier scene files")
    }

    fn no_earlier_files() -> Result<EarlierSceneFiles, String> {
        test_sessions::nothing_in_the_way()
    }

    fn three_earlier_files() -> Result<EarlierSceneFiles, String> {
        Ok(EarlierSceneFiles {
            scene_dir: test_sessions::scene_dir(),
            file_count: 3,
        })
    }

    /// What is found once "Put the scenes in a new folder" has been ticked.
    fn new_folder() -> PathBuf {
        PathBuf::from("videos/no_such_video_scenes")
    }

    fn two_earlier_files_in_the_new_folder() -> Result<EarlierSceneFiles, String> {
        Ok(EarlierSceneFiles {
            scene_dir: new_folder(),
            file_count: 2,
        })
    }

    fn an_empty_new_folder() -> Result<EarlierSceneFiles, String> {
        Ok(EarlierSceneFiles {
            scene_dir: new_folder(),
            file_count: 0,
        })
    }

    fn unreadable_folder() -> Result<EarlierSceneFiles, String> {
        Err("could not read the folder".to_owned())
    }

    fn replace_in(scene_dir: PathBuf) -> ReviewRequest {
        ReviewRequest::ReplaceEarlierFiles { scene_dir }
    }

    fn hard_cut_scan() -> ScanKind {
        ScanKind::HardCuts(crate::hardcut::HardCutSettings::default())
    }

    struct Session(ReviewSession);

    impl Session {
        fn five_scenes() -> Self {
            Self(test_sessions::five_scenes())
        }

        fn ask(&mut self, request: ReviewRequest) -> Option<FollowUp> {
            self.0.apply(request, &no_disk)
        }

        fn scene_ends(&self) -> Vec<f64> {
            self.0.scenes().iter().map(|scene| scene.end).collect()
        }

        fn selected(&self) -> Vec<usize> {
            self.0
                .selection()
                .scenes()
                .map(SceneIndex::position)
                .collect()
        }

        /// Types `time` into the box and presses "Add split".
        fn add_split_at(&mut self, time: &str) {
            self.ask(ReviewRequest::TypeNewSplitTime(time.to_owned()));
            self.ask(ReviewRequest::AddTypedSplit);
        }
    }

    #[test]
    fn a_new_review_says_what_the_scan_found() {
        let session = test_sessions::five_scenes();
        assert_eq!(session.scan_summary(), "Found 0 black screens.");
        assert!(session.undecoded().is_empty());
        assert_eq!(
            session.scan_details(),
            "0 of 3000 frames were decoded to find them."
        );
        assert_eq!(session.frame_seconds(), 100.0 / 3000.0);
    }

    fn stored_scan_with_a_stretch_unread() -> SourcedDetection {
        SourcedDetection {
            detection: test_sessions::scenes_of_twenty_seconds_detected(5),
            source: DetectionSource::StoredScan,
            undecoded: vec![UndecodedFrames {
                from_seconds: 42.0,
                missing_frames: 7,
                stretch_frames: 250,
            }],
        }
    }

    #[test]
    fn a_review_built_on_a_stored_scan_says_so() {
        let session = test_sessions::review_of(stored_scan_with_a_stretch_unread());
        assert_eq!(
            session.scan_summary(),
            "Found 0 black screens. Using the scan from last time."
        );
    }

    #[test]
    fn a_review_keeps_the_stretches_the_scan_could_not_decode() {
        let session = test_sessions::review_of(stored_scan_with_a_stretch_unread());
        assert_eq!(
            session.undecoded(),
            [UndecodedFrames {
                from_seconds: 42.0,
                missing_frames: 7,
                stretch_frames: 250,
            }]
        );
    }

    #[test]
    fn a_scan_that_also_found_hard_cuts_counts_them_apart() {
        let scan = SourcedDetection {
            detection: test_sessions::scenes_of_twenty_seconds_detected(3),
            source: DetectionSource::FreshScan,
            undecoded: Vec::new(),
        };
        // The scan suggests cuts at 20 and 40. One more comes from a hard cut.
        let cuts = CutTimes::inside(scan.detection.video, vec![20.0, 30.0, 40.0]).unwrap();
        let video: Arc<Path> = Arc::from(Path::new("no_such_video.mp4"));
        let session = ReviewSession::new(video, scan, cuts);

        assert_eq!(
            session.scan_summary(),
            "Found 3 scene changes: 0 black screens, 1 sudden picture change."
        );
    }

    #[test]
    fn merging_a_run_of_scenes_selects_the_merged_scene() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(1, 3)));

        assert_eq!(session.scene_ends(), vec![20.0, 80.0, 100.0]);
        assert_eq!(session.selected(), vec![1]);
    }

    #[test]
    fn merge_selected_joins_from_the_first_selected_scene_to_the_last() {
        let mut session = Session::five_scenes();
        session.ask(click(0, ClickKind::Single));
        session.ask(click(2, ClickKind::Toggle));
        session.ask(ReviewRequest::MergeSelected);

        assert_eq!(session.scene_ends(), vec![60.0, 80.0, 100.0]);
    }

    #[test]
    fn merge_selected_with_one_scene_selected_changes_nothing() {
        let mut session = Session::five_scenes();
        session.ask(click(2, ClickKind::Single));
        session.ask(ReviewRequest::MergeSelected);

        assert_eq!(session.0.scene_count(), 5);
        assert!(!session.0.can_undo());
    }

    #[test]
    fn select_all_then_merge_leaves_one_scene() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::SelectAllScenes);
        assert_eq!(session.selected(), vec![0, 1, 2, 3, 4]);

        session.ask(ReviewRequest::MergeSelected);
        assert_eq!(session.scene_ends(), vec![100.0]);
    }

    #[test]
    fn the_selection_can_be_cleared() {
        let mut session = Session::five_scenes();
        session.ask(click(1, ClickKind::Single));
        session.ask(click(3, ClickKind::ExtendRange));
        assert_eq!(session.selected(), vec![1, 2, 3]);

        session.ask(ReviewRequest::ClearSelection);
        assert!(session.selected().is_empty());
    }

    #[test]
    fn taking_a_merged_scene_apart_brings_its_scenes_back() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(1, 3)));
        assert!(session.0.has_merged_scene_selected());

        session.ask(ReviewRequest::TakeSelectedApart);
        assert_eq!(session.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn a_scene_that_is_not_merged_is_not_a_merged_selection() {
        let mut session = Session::five_scenes();
        session.ask(click(1, ClickKind::Single));
        assert!(!session.0.has_merged_scene_selected());
    }

    #[test]
    fn a_part_moved_out_of_a_merged_scene_becomes_its_own_scene() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(0, 4)));
        let whole = session.0.plan().scene_layouts().remove(0);

        session.ask(ReviewRequest::MoveOutSegment(whole.segment_index(2)));
        assert_eq!(session.scene_ends(), vec![40.0, 60.0, 100.0]);
    }

    #[test]
    fn a_merged_scene_can_be_separated_at_one_of_its_merged_cuts() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(0, 2)));
        let merged = session.0.plan().scene_layouts().remove(0);

        session.ask(ReviewRequest::SeparateAt(merged.merged_cuts[1]));
        assert_eq!(session.scene_ends(), vec![40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn undo_takes_back_one_change_at_a_time() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(0, 2)));
        session.ask(ReviewRequest::MergeScenes(scenes(1, 2)));
        assert_eq!(session.scene_ends(), vec![60.0, 100.0]);

        session.ask(ReviewRequest::Undo);
        assert_eq!(session.scene_ends(), vec![60.0, 80.0, 100.0]);

        session.ask(ReviewRequest::Undo);
        assert_eq!(session.0.scene_count(), 5);
        assert!(!session.0.can_undo());

        // With nothing left to undo, another one is harmless.
        session.ask(ReviewRequest::Undo);
        assert_eq!(session.0.scene_count(), 5);
    }

    #[test]
    fn undo_clears_a_selection_that_would_point_at_other_scenes() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));
        assert_eq!(session.selected(), vec![0]);

        session.ask(ReviewRequest::Undo);
        assert!(session.selected().is_empty());
    }

    #[test]
    fn a_reset_undoes_merges_and_added_splits_and_can_itself_be_undone() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));
        session.0.split_near(47.0);

        session.ask(ReviewRequest::ResetScenes);
        assert_eq!(session.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
        assert!(session.0.plan().is_as_detected());

        session.ask(ReviewRequest::Undo);
        assert_eq!(session.scene_ends(), vec![40.0, 50.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn a_fresh_review_has_no_edits_and_a_merge_is_one() {
        let mut session = Session::five_scenes();
        assert!(!session.0.is_edited());

        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));
        assert!(session.0.is_edited());
    }

    #[test]
    fn undoing_the_only_change_leaves_a_review_without_edits() {
        let mut session = Session::five_scenes();
        session.0.split_near(47.0);
        assert_eq!(session.0.scene_count(), 6);

        session.ask(ReviewRequest::Undo);
        assert_eq!(session.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
        assert!(!session.0.is_edited());
    }

    #[test]
    fn splitting_from_the_player_cuts_the_playing_scene_in_two() {
        let mut session = Session::five_scenes();
        // 47 seconds is inside the scene from 40 to 60, nearest keyframe 50.
        let feedback = session.0.split_near(47.0);

        assert_eq!(feedback, SplitFeedback::Added(50.0));
        assert!(!feedback.is_refusal());
        assert_eq!(
            feedback.in_words(),
            "Split at 00:00:50.000, the nearest point where the video can be cut."
        );
        assert_eq!(
            session.scene_ends(),
            vec![20.0, 40.0, 50.0, 60.0, 80.0, 100.0]
        );
    }

    #[test]
    fn splitting_where_a_split_already_is_says_so() {
        let mut session = Session::five_scenes();
        let feedback = session.0.split_near(41.0);

        assert_eq!(
            feedback,
            SplitFeedback::Refused(AddCutError::AlreadyProposed { at: 40.0 })
        );
        assert!(feedback.is_refusal());
        assert_eq!(
            feedback.in_words(),
            "There is already a split point at 00:00:40.000."
        );
        assert_eq!(session.0.scene_count(), 5);
    }

    #[test]
    fn a_split_that_is_refused_leaves_nothing_to_undo() {
        let mut session = Session::five_scenes();
        session.0.split_near(41.0);
        assert!(!session.0.can_undo());
    }

    #[test]
    fn the_player_is_told_where_a_split_would_land() {
        let session = test_sessions::five_scenes();
        assert_eq!(session.split_time_near(47.0), Some(50.0));
        assert_eq!(session.split_time_near(250.0), None);
    }

    #[test]
    fn a_typed_split_is_added_and_the_box_is_emptied() {
        let mut session = Session::five_scenes();
        session.add_split_at("0:47");

        assert_eq!(
            session.scene_ends(),
            vec![20.0, 40.0, 50.0, 60.0, 80.0, 100.0]
        );
        assert_eq!(session.0.new_split_time(), "");
        assert_eq!(
            session.0.new_split_feedback(),
            Some(&SplitFeedback::Added(50.0))
        );
    }

    #[test]
    fn a_typed_time_that_is_refused_stays_in_the_box_to_be_corrected() {
        let mut session = Session::five_scenes();
        session.add_split_at("5:00");

        assert_eq!(session.0.scene_count(), 5);
        assert_eq!(session.0.new_split_time(), "5:00");
        assert_eq!(
            session.0.new_split_feedback(),
            Some(&SplitFeedback::Refused(AddCutError::OutsideVideo))
        );
    }

    #[test]
    fn something_that_is_not_a_time_asks_for_one() {
        let mut session = Session::five_scenes();
        session.add_split_at("soon");

        assert_eq!(session.0.scene_count(), 5);
        assert_eq!(
            session.0.new_split_feedback(),
            Some(&SplitFeedback::NotATime)
        );
        assert!(SplitFeedback::NotATime.is_refusal());
        assert_eq!(
            SplitFeedback::NotATime.in_words(),
            "Type a time like 1:03:12."
        );
    }

    #[test]
    fn nothing_is_said_about_a_new_split_before_one_was_tried() {
        assert_eq!(test_sessions::five_scenes().new_split_feedback(), None);
    }

    #[test]
    fn the_cuts_handed_to_the_split_follow_the_merges() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(1, 3)));
        assert_eq!(session.0.accepted_cuts().as_slice(), [20.0, 80.0]);
    }

    #[test]
    fn the_player_can_go_as_far_as_the_end_of_the_video() {
        assert_eq!(test_sessions::five_scenes().video_end(), 100.0);
    }

    #[test]
    fn a_split_starts_at_once_when_no_earlier_files_are_in_the_way() {
        let mut session = test_sessions::five_scenes();
        assert_eq!(
            session.apply(ReviewRequest::Split, &no_earlier_files),
            Some(FollowUp::Export)
        );
        assert_eq!(session.export_obstacle(), None);
    }

    #[test]
    fn earlier_scene_files_hold_the_split_until_replacing_them_is_confirmed() {
        let mut session = test_sessions::five_scenes();
        assert_eq!(
            session.apply(ReviewRequest::Split, &three_earlier_files),
            None
        );
        assert_eq!(
            session.export_obstacle(),
            Some(&ExportObstacle::EarlierSceneFiles(EarlierSceneFiles {
                scene_dir: test_sessions::scene_dir(),
                file_count: 3,
            }))
        );

        assert_eq!(
            session.apply(replace_in(test_sessions::scene_dir()), &three_earlier_files),
            Some(FollowUp::Export)
        );
        assert_eq!(session.export_obstacle(), None);
    }

    #[test]
    fn agreeing_to_replace_files_in_one_folder_does_not_replace_those_in_another() {
        let mut session = test_sessions::five_scenes();
        session.apply(ReviewRequest::Split, &three_earlier_files);

        // The setting for where scenes go was changed somewhere else, for
        // instance in the sidebar, and then "Replace them" was pressed.
        assert_eq!(
            session.apply(
                replace_in(test_sessions::scene_dir()),
                &two_earlier_files_in_the_new_folder
            ),
            None
        );
        // The person is now warned about the files that would really go.
        assert_eq!(
            session.export_obstacle(),
            Some(&ExportObstacle::EarlierSceneFiles(EarlierSceneFiles {
                scene_dir: new_folder(),
                file_count: 2,
            }))
        );

        // Agreeing to that warning does start the split.
        assert_eq!(
            session.apply(
                replace_in(new_folder()),
                &two_earlier_files_in_the_new_folder
            ),
            Some(FollowUp::Export)
        );
    }

    #[test]
    fn with_nothing_to_replace_where_the_scenes_go_now_the_split_just_starts() {
        let mut session = test_sessions::five_scenes();
        session.apply(ReviewRequest::Split, &three_earlier_files);

        assert_eq!(
            session.apply(replace_in(test_sessions::scene_dir()), &an_empty_new_folder),
            Some(FollowUp::Export)
        );
        assert_eq!(session.export_obstacle(), None);
    }

    #[test]
    fn a_folder_that_can_no_longer_be_checked_holds_a_confirmed_split() {
        let mut session = test_sessions::five_scenes();
        session.apply(ReviewRequest::Split, &three_earlier_files);

        assert_eq!(
            session.apply(replace_in(test_sessions::scene_dir()), &unreadable_folder),
            None
        );
        assert!(matches!(
            session.export_obstacle(),
            Some(ExportObstacle::CouldNotCheck(_))
        ));
    }

    #[test]
    fn a_folder_that_cannot_be_checked_holds_the_split_with_the_reason() {
        let mut session = test_sessions::five_scenes();
        assert_eq!(
            session.apply(ReviewRequest::Split, &unreadable_folder),
            None
        );
        assert_eq!(
            session.export_obstacle(),
            Some(&ExportObstacle::CouldNotCheck(
                "could not read the folder".to_owned()
            ))
        );
    }

    #[test]
    fn a_warning_about_earlier_files_goes_when_the_plan_changes() {
        let mut session = Session::five_scenes();
        session.0.apply(ReviewRequest::Split, &three_earlier_files);

        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));
        assert_eq!(session.0.export_obstacle(), None);
    }

    #[test]
    fn a_warning_about_earlier_files_can_be_forgotten() {
        let mut session = test_sessions::five_scenes();
        session.apply(ReviewRequest::Split, &three_earlier_files);

        session.forget_export_obstacle();
        assert_eq!(session.export_obstacle(), None);
    }

    #[test]
    fn a_change_of_settings_is_left_to_the_app() {
        let mut session = Session::five_scenes();
        assert_eq!(
            session.ask(ReviewRequest::ChangeSettings(SettingsChange::HideTips)),
            Some(FollowUp::ChangeSettings(SettingsChange::HideTips))
        );
    }

    #[test]
    fn an_untouched_review_is_scanned_again_without_asking() {
        let mut session = Session::five_scenes();
        assert_eq!(
            session.ask(ReviewRequest::Rescan(ScanKind::BlackScreens)),
            Some(FollowUp::Rescan(ScanKind::BlackScreens))
        );
        assert!(!session.0.is_asking_before_rescan());
    }

    #[test]
    fn an_edited_review_asks_before_a_scan_throws_the_edits_away() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));

        assert_eq!(session.ask(ReviewRequest::Rescan(hard_cut_scan())), None);
        assert!(session.0.is_asking_before_rescan());

        // The scan that was asked for is the one that runs.
        assert_eq!(
            session.ask(ReviewRequest::RescanAnyway),
            Some(FollowUp::Rescan(hard_cut_scan()))
        );
        assert!(!session.0.is_asking_before_rescan());
    }

    #[test]
    fn looking_harder_at_a_video_merged_into_one_scene_asks_first() {
        let mut session = Session::five_scenes();
        // Everything merged into one scene, which is when the "Look harder
        // for scenes" button shows. It asks for this scan.
        session.ask(ReviewRequest::MergeScenes(scenes(0, 4)));
        assert_eq!(session.0.scene_count(), 1);

        assert_eq!(session.ask(ReviewRequest::Rescan(hard_cut_scan())), None);
        assert!(session.0.is_asking_before_rescan());
    }

    #[test]
    fn keeping_the_edits_cancels_the_scan() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));
        session.ask(ReviewRequest::Rescan(ScanKind::BlackScreens));

        assert_eq!(session.ask(ReviewRequest::KeepEdits), None);
        assert!(!session.0.is_asking_before_rescan());
        assert_eq!(session.0.scene_count(), 4);
    }

    #[test]
    fn scanning_anyway_without_having_been_asked_does_nothing() {
        let mut session = Session::five_scenes();
        assert_eq!(session.ask(ReviewRequest::RescanAnyway), None);
    }

    #[test]
    fn skipping_and_playing_are_left_to_the_app() {
        let mut session = Session::five_scenes();
        assert_eq!(session.ask(ReviewRequest::Skip), Some(FollowUp::Skip));
        assert_eq!(
            session.ask(ReviewRequest::PlayFrom(42.5)),
            Some(FollowUp::PlayFrom(42.5))
        );
    }

    #[test]
    fn a_frame_of_requests_is_applied_in_order() {
        let mut session = test_sessions::five_scenes();
        let follow_up = session.apply_frame(
            vec![
                click(2, ClickKind::Single),
                ReviewRequest::PlayFrom(42.5),
                ReviewRequest::Skip,
            ],
            &no_disk,
        );

        // The first request that needs the app ends the frame.
        assert_eq!(follow_up, Some(FollowUp::PlayFrom(42.5)));
        assert!(session.selection().contains(scene(2)));
    }

    #[test]
    fn requests_drawn_for_the_old_plan_are_dropped_once_the_plan_has_changed() {
        let mut session = test_sessions::five_scenes();
        let follow_up = session.apply_frame(
            vec![
                ReviewRequest::MergeScenes(scenes(0, 1)),
                // Scene 4 was the last of five. It no longer exists.
                click(4, ClickKind::Single),
                ReviewRequest::MergeScenes(scenes(2, 3)),
            ],
            &no_disk,
        );

        assert_eq!(follow_up, None);
        assert_eq!(session.scene_count(), 4);
        assert!(!session.selection().contains(scene(4)));
    }

    #[test]
    fn requests_that_leave_the_plan_alone_do_not_stop_the_frame() {
        let mut session = test_sessions::five_scenes();
        session.apply_frame(
            vec![
                ReviewRequest::TypeNewSplitTime("0:47".to_owned()),
                ReviewRequest::AddTypedSplit,
            ],
            &no_disk,
        );
        assert_eq!(session.scene_count(), 6);
    }

    fn chapter(start: f64, title: &str) -> crate::chapters::Chapter {
        crate::chapters::Chapter {
            start,
            title: Some(title.to_owned()),
        }
    }

    /// Five scenes of 20 seconds found by the scan, in a file with four
    /// chapters: one that opens the video, one on a detected split, and two
    /// where the scan found nothing.
    fn five_scenes_with_chapters() -> ReviewSession {
        let mut detection = test_sessions::scenes_of_twenty_seconds_detected(5);
        detection.chapters = Some(vec![
            chapter(0.0, "Opening"),
            chapter(31.0, "The plan"),
            chapter(40.0, "The heist"),
            chapter(89.0, "Credits"),
        ]);
        test_sessions::fresh_scan_of(detection)
    }

    #[test]
    fn chapter_marks_become_splits_where_the_scan_found_none() {
        let session = five_scenes_with_chapters();
        // The marks at 31 and 89 land on the points to cut at 30 and 90.
        assert_eq!(
            session.accepted_cuts().as_slice(),
            [20.0, 30.0, 40.0, 60.0, 80.0, 90.0]
        );
        assert_eq!(session.chapter_split_count(), 2);
        // The person changed nothing yet.
        assert!(!session.is_edited());
    }

    #[test]
    fn the_scan_summary_says_how_many_splits_come_from_chapter_marks() {
        let session = five_scenes_with_chapters();
        assert_eq!(
            session.scan_summary(),
            "Found 0 black screens. 2 more splits come from chapter marks."
        );
        assert_eq!(
            session.scan_details(),
            "0 of 3000 frames were decoded to find them. The file has 4 chapter marks."
        );
    }

    #[test]
    fn one_split_from_a_chapter_mark_is_said_in_the_singular() {
        assert_eq!(splits_from_chapters_in_words(0), None);
        assert_eq!(
            splits_from_chapters_in_words(1).as_deref(),
            Some("1 more split comes from a chapter mark.")
        );
    }

    #[test]
    fn a_stored_scan_with_chapters_says_both_things() {
        let mut detection = test_sessions::scenes_of_twenty_seconds_detected(5);
        detection.chapters = Some(vec![chapter(31.0, "The plan")]);
        let session = test_sessions::review_of(SourcedDetection {
            detection,
            source: DetectionSource::StoredScan,
            undecoded: Vec::new(),
        });
        assert_eq!(
            session.scan_summary(),
            "Found 0 black screens. 1 more split comes from a chapter mark. \
             Using the scan from last time."
        );
    }

    #[test]
    fn a_scene_carries_the_title_of_the_chapter_that_begins_with_it() {
        let session = five_scenes_with_chapters();
        let titles: Vec<Option<&str>> = session
            .scenes()
            .into_iter()
            .map(|scene| session.chapter_title(scene))
            .collect();
        assert_eq!(
            titles,
            [
                Some("Opening"),
                None,
                Some("The plan"),
                Some("The heist"),
                None,
                None,
                Some("Credits"),
            ]
        );
    }

    #[test]
    fn a_merged_scene_keeps_the_title_of_its_first_part() {
        let mut session = five_scenes_with_chapters();
        session.apply(ReviewRequest::MergeScenes(scenes(2, 4)), &no_disk);

        let merged = session.scenes()[2];
        assert_eq!(merged.start, 30.0);
        assert_eq!(session.chapter_title(merged), Some("The plan"));
    }

    #[test]
    fn a_video_without_chapters_has_no_titles_and_says_nothing_about_them() {
        let session = test_sessions::five_scenes();
        assert_eq!(session.chapter_split_count(), 0);
        assert!(session
            .scenes()
            .into_iter()
            .all(|scene| session.chapter_title(scene).is_none()));
    }

    #[test]
    fn leaving_out_the_chapter_marks_keeps_what_the_scan_found_and_can_be_undone() {
        let mut session = five_scenes_with_chapters();
        assert_eq!(
            session.apply(ReviewRequest::LeaveOutChapterSplits, &no_disk),
            None
        );
        assert_eq!(session.accepted_cuts().as_slice(), [20.0, 40.0, 60.0, 80.0]);
        assert_eq!(session.chapter_split_count(), 0);

        session.apply(ReviewRequest::Undo, &no_disk);
        assert_eq!(session.chapter_split_count(), 2);
    }

    #[test]
    fn resetting_the_scenes_brings_the_chapter_splits_back() {
        let mut session = five_scenes_with_chapters();
        session.apply(ReviewRequest::LeaveOutChapterSplits, &no_disk);
        session.apply(ReviewRequest::ResetScenes, &no_disk);
        assert_eq!(session.chapter_split_count(), 2);
        assert_eq!(session.scene_count(), 7);
    }

    #[test]
    fn splitting_later_with_nothing_in_the_way_marks_the_video_ready() {
        let mut session = test_sessions::five_scenes();
        assert_eq!(
            session.apply(ReviewRequest::SplitLater, &no_earlier_files),
            Some(FollowUp::MarkReady)
        );
        assert_eq!(session.export_obstacle(), None);
    }

    #[test]
    fn splitting_later_over_earlier_files_asks_first_and_then_marks_ready() {
        let mut session = test_sessions::five_scenes();
        assert_eq!(
            session.apply(ReviewRequest::SplitLater, &three_earlier_files),
            None
        );
        assert!(matches!(
            session.export_obstacle(),
            Some(ExportObstacle::EarlierSceneFiles(_))
        ));

        // Agreeing does what was asked for: wait with the ready videos, not
        // split at once.
        assert_eq!(
            session.apply(replace_in(test_sessions::scene_dir()), &three_earlier_files),
            Some(FollowUp::MarkReady)
        );
        assert_eq!(session.export_obstacle(), None);
    }

    #[test]
    fn agreeing_to_replace_after_asking_to_split_now_still_splits_now() {
        let mut session = test_sessions::five_scenes();
        // First "Split later", then a change of mind.
        session.apply(ReviewRequest::SplitLater, &three_earlier_files);
        session.apply(ReviewRequest::Split, &three_earlier_files);
        assert_eq!(
            session.apply(replace_in(test_sessions::scene_dir()), &three_earlier_files),
            Some(FollowUp::Export)
        );
    }

    #[test]
    fn a_split_may_go_over_nothing_or_over_files_the_person_agreed_to_replace() {
        let mut session = test_sessions::five_scenes();
        assert!(session.may_split_over(&no_earlier_files()));
        assert!(!session.may_split_over(&three_earlier_files()));
        assert!(!session.may_split_over(&unreadable_folder()));

        session.apply(ReviewRequest::SplitLater, &three_earlier_files);
        session.apply(replace_in(test_sessions::scene_dir()), &three_earlier_files);
        assert!(session.may_split_over(&three_earlier_files()));
        // The leave was for that folder and no other.
        assert!(!session.may_split_over(&two_earlier_files_in_the_new_folder()));
        assert!(!session.may_split_over(&unreadable_folder()));
    }

    #[test]
    fn leave_to_replace_files_does_not_outlast_a_change_of_the_scenes() {
        let mut session = test_sessions::five_scenes();
        session.apply(ReviewRequest::SplitLater, &three_earlier_files);
        session.apply(replace_in(test_sessions::scene_dir()), &three_earlier_files);

        session.apply(ReviewRequest::MergeScenes(scenes(0, 1)), &no_disk);
        assert!(!session.may_split_over(&three_earlier_files()));
    }

    #[test]
    fn leave_to_replace_files_does_not_outlast_a_change_of_where_scenes_go() {
        let mut session = test_sessions::five_scenes();
        session.apply(ReviewRequest::SplitLater, &three_earlier_files);
        session.apply(replace_in(test_sessions::scene_dir()), &three_earlier_files);

        session.forget_export_obstacle();
        assert!(!session.may_split_over(&three_earlier_files()));
    }

    #[test]
    fn changes_of_detection_settings_are_left_to_the_app() {
        let mut session = test_sessions::five_scenes();
        let mut detection = DetectionSettings::default();
        detection.scan.criteria.min_seconds = 0.5;

        assert_eq!(
            session.apply(ReviewRequest::ChangeDetection(detection), &no_disk),
            Some(FollowUp::ChangeDetection(detection))
        );
        assert_eq!(
            session.apply(ReviewRequest::RememberForFolder, &no_disk),
            Some(FollowUp::RememberForFolder)
        );
        assert_eq!(
            session.apply(ReviewRequest::UseGeneralSettings, &no_disk),
            Some(FollowUp::UseGeneralSettings)
        );
        // None of them is an edit of the scenes.
        assert!(!session.is_edited());
    }

    #[test]
    fn counts_read_naturally() {
        assert_eq!(counted(1, "scene"), "1 scene");
        assert_eq!(counted(3, "scene"), "3 scenes");
    }
}
