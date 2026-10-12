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
use crate::exact_cut::CutPrecision;
use crate::exact_fallback::ExactCutLook;
use crate::fates::{NothingToWrite, SceneFate, ScenesToWrite};
use crate::focus::{self, PictureStep, ReviewFocus, SceneStep};
use crate::folder_settings::DetectionSettings;
use crate::plan::{
    AddCutError, CutChoice, CutIndex, MovedCut, NudgeCutError, NudgeDirection, NudgeStep,
    PlanSnapshot, SceneIndex, SceneLayout, SegmentIndex, SplitPlan,
};
use crate::scan::UndecodedFrames;
use crate::scene_list::SceneListFormat;
use crate::scene_name::EarlierSplit;
use crate::scenes::CutTimes;
use crate::selection::{ClickKind, SceneSelection};
use crate::settings::SettingsChange;
use crate::thumbnails;
use crate::timecode::{format_timestamp, parse_timestamp, TimeSpan};

const CHAPTERS_NOT_READ: &str = "The chapter marks of this file could not be read.";

/// What looking for the scene files of an earlier split found.
#[derive(Debug, Clone, PartialEq)]
pub struct EarlierSceneFiles {
    /// The folder the scenes of this video would be written to now.
    pub scene_dir: PathBuf,
    /// The earlier splits of this video that left scene files in it, each
    /// with its files. The person is shown them before any is replaced or
    /// removed. A split named as scenes are named now comes first.
    pub splits: Vec<EarlierSplit>,
}

impl EarlierSceneFiles {
    /// How many scene files of earlier splits are in the folder.
    pub fn file_count(&self) -> usize {
        self.file_names().len()
    }

    /// The names of those files, in name order, each once.
    pub fn file_names(&self) -> Vec<String> {
        let mut file_names: Vec<String> = self
            .splits
            .iter()
            .flat_map(EarlierSplit::file_names)
            .collect();
        file_names.sort();
        file_names.dedup();
        file_names
    }

    /// An earlier split of "no_such_video.mp4" into `count` scenes, named as
    /// the default pattern names them, for a test that only cares how many
    /// files are in the way. No split at all for a count of 0.
    #[cfg(test)]
    pub fn a_split_into(count: usize) -> Vec<EarlierSplit> {
        const VIDEO: &str = "no_such_video.mp4";
        let file_names = crate::scene_name::default_names_for_test(VIDEO, count);
        let file_names: Vec<&str> = file_names.iter().map(String::as_str).collect();
        crate::scene_name::earlier_splits_for_test(VIDEO, &file_names)
    }
}

/// Why the split has not started after the person asked for it.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportObstacle {
    /// Scene files of the same names exist in the folder the scenes go to
    /// and would be replaced. The warning is about that folder only.
    EarlierSceneFiles(EarlierSceneFiles),
    /// The folder the scenes go to could not be looked into.
    CouldNotCheck(String),
    /// Splits at these times were placed or moved onto exact pictures, and
    /// cuts are no longer exact. A copy cannot cut there: each scene would
    /// start at the next point where the video can be cut, and not where it
    /// was reviewed.
    SplitsOffCutPoints(Vec<f64>),
    /// The person asked for such splits to be moved, and those at these
    /// times could not be: the nearest point where the video can be cut is
    /// taken by, or lies beyond, the split beside them.
    SplitsStuckOffCutPoints(Vec<f64>),
}

/// How asking for a new split, or for a split to be moved, went, for the
/// screens to say.
#[derive(Debug, PartialEq)]
pub enum SplitFeedback {
    /// A split was added at this time of the video, the nearest point where
    /// it can be cut as it is.
    Added(f64),
    /// A split was added at the very time that was asked for.
    AddedExactly(f64),
    Refused(AddCutError),
    /// What was typed is not a time.
    NotATime,
    /// A split was moved one step: to the next point where the video can
    /// be cut, or by one picture when cuts are exact.
    Moved(MovedCut),
    MoveRefused(NudgeCutError),
}

impl SplitFeedback {
    /// Whether nothing was added, so the screens can tell it apart from a
    /// confirmation.
    pub fn is_refusal(&self) -> bool {
        match self {
            Self::Added(_) | Self::AddedExactly(_) | Self::Moved(_) => false,
            Self::Refused(_) | Self::NotATime | Self::MoveRefused(_) => true,
        }
    }

    pub fn in_words(&self) -> String {
        match self {
            Self::Added(cut_time) => format!(
                "Added a split at {}, the nearest point where the video can be cut.",
                format_timestamp(*cut_time)
            ),
            Self::AddedExactly(cut_time) => format!(
                "Added a split at exactly {}. A few seconds around it are encoded again when you split.",
                format_timestamp(*cut_time)
            ),
            Self::Refused(why) => why.to_string(),
            Self::NotATime => "Type a time like 1:03:12.".to_owned(),
            Self::Moved(moved) => format!(
                "Moved the split from {} to {}.",
                format_timestamp(moved.from),
                format_timestamp(moved.to)
            ),
            Self::MoveRefused(why) => why.to_string(),
        }
    }
}

/// How saving the scene list went, for the review screen to say.
#[derive(Debug, Clone, PartialEq)]
pub enum SceneListNote {
    Saved(PathBuf),
    /// Why nothing was saved, in words for the person.
    NotSaved(String),
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
    /// Move the split at which this scene starts to the next point, earlier
    /// or later, where the video can be cut.
    NudgeSplit {
        scene: SceneIndex,
        direction: NudgeDirection,
    },
    /// Move the focus to another scene. The selection follows by the rules
    /// of a click of this kind on that scene.
    StepFocus {
        step: SceneStep,
        kind: ClickKind,
    },
    /// Choose another picture of the focused scene to play from.
    StepPicture(PictureStep),
    /// Play the focused scene, from the picture chosen in it or from its
    /// start.
    PlayFocused,
    JoinFocusedWithNext,
    /// Do not write this scene to a file.
    DropScene(SceneIndex),
    /// Write this scene after all.
    KeepScene(SceneIndex),
    /// Drop the selected scenes, or keep them again when every one of them
    /// is dropped already.
    DropOrKeepSelected,
    /// Save the list of scenes to this file, in this format, without
    /// splitting anything.
    SaveSceneList {
        format: SceneListFormat,
        path: PathBuf,
    },
    Undo,
    ResetScenes,
    /// Join the scenes on both sides of every split that comes from a
    /// chapter mark of the file.
    LeaveOutChapterSplits,
    /// Move every split that stands on an exact picture to the nearest
    /// point where the video can be cut as it is.
    MoveSplitsToCutPoints,
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
#[derive(Debug, Clone, PartialEq)]
pub enum FollowUp {
    Export,
    /// Put the video with those that are ready to be split.
    MarkReady,
    Skip,
    Rescan(ScanKind),
    PlayFrom(f64),
    ChangeSettings(SettingsChange),
    /// Write the list of scenes to this file. How that went comes back
    /// through `ReviewSession::note_scene_list`.
    SaveSceneList {
        format: SceneListFormat,
        path: PathBuf,
    },
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
    /// The scene the keys act on, once the person has stepped onto or
    /// clicked one.
    focus: Option<ReviewFocus>,
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
    /// How the last split added or moved on the review screen went, once
    /// one was tried.
    new_split_feedback: Option<SplitFeedback>,
    /// Why the last try to drop scenes changed nothing, until the plan
    /// changes again.
    drop_refusal: Option<NothingToWrite>,
    /// How the last saving of the scene list went, once one was tried.
    scene_list_note: Option<SceneListNote>,
    export_obstacle: Option<ExportObstacle>,
    /// What the person asked for when `export_obstacle` came up, which is
    /// what goes on once they agree to replace the files.
    split_asked_for: SplitTiming,
    /// The earlier scene files the person agreed to replace: the ones they
    /// were shown, in the folder they were shown. Leave to replace those is
    /// no leave to replace others that turn up later. A video that waits to
    /// be split with the others needs this later, when its turn comes and
    /// nobody is there to ask.
    replacing_agreed_for: Option<EarlierSceneFiles>,
    /// A scan the person asked for that would throw away their edits, until
    /// they confirm or cancel it.
    rescan_awaiting_consent: Option<ScanKind>,
    /// The titles the file's chapter marks give to scenes.
    chapter_splits: ChapterSplits,
    /// Where a split the person adds lands. It follows the setting, which
    /// the app passes on, since settings belong to no one review.
    new_split_precision: CutPrecision,
    /// Whether this video can be cut on exact pictures, as far as a look at
    /// its file says. The app looks, since the session reads no files.
    exact_cut_look: ExactCutLook,
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
        let frame_seconds = detection.frame_seconds();
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
            "The scan looked at {} of {} pictures to find them.",
            detection.decoded_frames, detection.frame_count
        );
        let chapter_mark_count = detection.chapter_marks().len();
        let scan_details = match (detection.chapters.problem(), chapter_mark_count) {
            (Some(problem), _) => format!("{decoded} {CHAPTERS_NOT_READ} ({problem})"),
            (None, 0) => decoded,
            (None, marks) => format!("{decoded} The file has {}.", counted(marks, "chapter mark")),
        };
        // Said where it is read without pointing: scenes the file marks
        // may be missing from the list.
        let scan_summary = match detection.chapters.problem() {
            Some(_) => format!("{scan_summary} {CHAPTERS_NOT_READ}"),
            None => scan_summary,
        };
        let mut plan = SplitPlan::from_detected_cuts(&cuts, detection.keyframe_times);
        plan.propose_chapter_cuts(&chapter_splits.cuts);
        Self {
            video,
            plan,
            undo_history: Vec::new(),
            selection: SceneSelection::default(),
            focus: None,
            scan_summary,
            scan_details,
            undecoded,
            frame_seconds,
            new_split_time: String::new(),
            new_split_feedback: None,
            drop_refusal: None,
            scene_list_note: None,
            export_obstacle: None,
            split_asked_for: SplitTiming::Now,
            replacing_agreed_for: None,
            rescan_awaiting_consent: None,
            new_split_precision: CutPrecision::NearestKeyframe,
            exact_cut_look: ExactCutLook::NotLooked,
            chapter_splits,
        }
    }

    pub fn exact_cut_look(&self) -> &ExactCutLook {
        &self.exact_cut_look
    }

    /// The app says how far its look at the video's file has come.
    pub fn note_exact_cut_look(&mut self, look: ExactCutLook) {
        self.exact_cut_look = look;
    }

    /// What to tell the person before a split that cuts with `precision`:
    /// that this video will not be cut on the exact pictures, and why. Only
    /// while exact cuts are asked for and a look at the file found a reason.
    pub fn exact_cut_notice_for(&self, precision: CutPrecision) -> Option<String> {
        match precision {
            CutPrecision::NearestKeyframe => None,
            CutPrecision::ExactFrame => self.exact_cut_look.notice_before_the_split(),
        }
    }

    /// The title of the chapter that begins where this scene begins, when
    /// the file has chapter marks with titles.
    ///
    /// A split the person moved still begins its chapter: the title is
    /// found by where the review began with that split.
    pub fn chapter_title(&self, scene: TimeSpan) -> Option<&str> {
        let began_at = self.plan.time_the_review_began_with(scene.start);
        self.chapter_splits.title_of_scene_starting_at(began_at)
    }

    /// How many splits the review has now only because of a chapter mark.
    pub fn chapter_split_count(&self) -> usize {
        self.plan.chapter_split_count()
    }

    /// Whether a split may go ahead given what was `found` where the scenes
    /// go: nothing in the way, or exactly the files the person already
    /// agreed to replace, in that same folder.
    pub fn may_split_over(&self, found: &Result<EarlierSceneFiles, String>) -> bool {
        match found {
            Ok(earlier_files) => {
                earlier_files.file_count() == 0
                    || self.replacing_agreed_for.as_ref() == Some(earlier_files)
            }
            Err(_could_not_look) => false,
        }
    }

    /// The splits a split of this review cannot make where they stand when
    /// it cuts with `precision`. None when cuts are exact. A copy can only
    /// cut at certain points, and splits placed or moved while cuts were
    /// exact may stand between them.
    pub fn splits_off_cut_points_for(&self, precision: CutPrecision) -> Vec<f64> {
        match precision {
            CutPrecision::NearestKeyframe => self.plan.splits_off_cut_points(),
            CutPrecision::ExactFrame => Vec::new(),
        }
    }

    /// The turn of this video in the run over the ready videos came, and
    /// `obstacle` stood in the way of its split. Nobody is asked in the
    /// middle of a run, so the question is left open here, for the person
    /// to find when they open the video. The video was to be split with the
    /// ready ones, and that is where it goes back to once they have dealt
    /// with the obstacle.
    pub fn hold_back_from_the_ready_run(&mut self, obstacle: ExportObstacle) {
        self.export_obstacle = Some(obstacle);
        self.split_asked_for = SplitTiming::WithTheReadyVideos;
        self.replacing_agreed_for = None;
    }

    /// Says where the splits the person adds from now on land: on the
    /// nearest keyframe, or on the very picture they chose.
    pub fn place_new_splits(&mut self, precision: CutPrecision) {
        self.new_split_precision = precision;
    }

    /// How far "Earlier" and "Later" beside "Move this split" move a split.
    /// It follows where new splits land: a split that copies can only be
    /// cut at certain points, and an exact one on any picture.
    pub fn nudge_step(&self) -> NudgeStep {
        match self.new_split_precision {
            CutPrecision::NearestKeyframe => NudgeStep::ToNextCutPoint,
            CutPrecision::ExactFrame => NudgeStep::OnePicture {
                seconds: self.frame_seconds,
            },
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

    pub fn focus(&self) -> Option<ReviewFocus> {
        self.focus
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

    /// Why the scenes the person last tried to drop are still written.
    pub fn drop_refusal(&self) -> Option<&NothingToWrite> {
        self.drop_refusal.as_ref()
    }

    pub fn scene_list_note(&self) -> Option<&SceneListNote> {
        self.scene_list_note.as_ref()
    }

    /// The app saves the scene list, since the session writes no files, and
    /// tells the session how it went.
    pub fn note_scene_list(&mut self, note: SceneListNote) {
        self.scene_list_note = Some(note);
    }

    /// What the split does with each scene, in scene order.
    pub fn scene_fates(&self) -> Vec<SceneFate> {
        self.plan.scene_fates()
    }

    /// The cuts and the scenes to write, as a split takes them.
    pub fn scenes_to_write(&self) -> ScenesToWrite {
        self.plan.scenes_to_write()
    }

    /// How many scenes the person dropped. A split writes the others.
    pub fn dropped_count(&self) -> usize {
        self.scenes_to_write().dropped_count()
    }

    /// A warning about earlier scene files was given for one folder and one
    /// plan, and so was the person's leave to replace them. Whoever changes
    /// where the scenes go calls this, and so does a review that is taken up
    /// again after its split failed or after it waited to be split.
    pub fn forget_export_obstacle(&mut self) {
        self.export_obstacle = None;
        self.replacing_agreed_for = None;
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

    /// Where a split asked for at `time` would land, or `None` where no
    /// split is possible.
    pub fn split_time_near(&self, time: f64) -> Option<f64> {
        self.plan.cut_time_for(time, self.new_split_precision).ok()
    }

    /// The scenes as they stand now, in order, for the player to show.
    pub fn scenes(&self) -> Vec<TimeSpan> {
        self.plan.scenes()
    }

    #[cfg(test)]
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

    /// Whether "Take merged scenes apart" has a merged scene to take apart.
    pub fn has_merged_scene_selected(&self) -> bool {
        self.layouts_to_take_apart()
            .iter()
            .any(SceneLayout::is_merged)
    }

    /// The moment the focused scene plays from: the picture chosen in it,
    /// or its start when none was.
    pub fn focused_play_time(&self) -> Option<f64> {
        let focus = self.focus?;
        let layout = self.focused_layout()?;
        let Some(picture) = focus.picture() else {
            return Some(layout.span.start);
        };
        let part = *layout.segments.get(focus::part_of(picture))?;
        thumbnails::preview_times(part)
            .get(focus::place_in_part(picture))
            .copied()
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
            ReviewRequest::ClickScene { scene, kind } => {
                self.selection.click(scene, kind);
                self.focus = Some(ReviewFocus::on(scene));
            }
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
            ReviewRequest::NudgeSplit { scene, direction } => self.nudge_split(scene, direction),
            ReviewRequest::StepFocus { step, kind } => self.step_focus(step, kind),
            ReviewRequest::StepPicture(step) => self.step_picture(step),
            ReviewRequest::PlayFocused => {
                return self.focused_play_time().map(FollowUp::PlayFrom);
            }
            ReviewRequest::JoinFocusedWithNext => self.join_focused_with_next(),
            ReviewRequest::DropScene(scene) => self.set_fate_of(vec![scene], SceneFate::Drop),
            ReviewRequest::KeepScene(scene) => self.set_fate_of(vec![scene], SceneFate::Keep),
            ReviewRequest::DropOrKeepSelected => self.drop_or_keep_selected(),
            ReviewRequest::SaveSceneList { format, path } => {
                return Some(FollowUp::SaveSceneList { format, path });
            }
            ReviewRequest::Undo => self.undo(),
            ReviewRequest::ResetScenes => self.edit_plan(SplitPlan::reset_to_detected),
            ReviewRequest::LeaveOutChapterSplits => self.edit_plan(SplitPlan::merge_chapter_cuts),
            ReviewRequest::MoveSplitsToCutPoints => self.move_splits_to_cut_points(),
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
        match (self.add_split_near(time), self.new_split_precision) {
            (Ok(cut_time), CutPrecision::NearestKeyframe) => SplitFeedback::Added(cut_time),
            (Ok(cut_time), CutPrecision::ExactFrame) => SplitFeedback::AddedExactly(cut_time),
            (Err(refusal), _) => SplitFeedback::Refused(refusal),
        }
    }

    /// Splitting replaces files of the same name, so existing scene files
    /// need a second, explicit confirmation. And a split that cannot cut
    /// where the person placed it does not start at all.
    fn ask_to_split(
        &mut self,
        found: Result<EarlierSceneFiles, String>,
        timing: SplitTiming,
    ) -> Option<FollowUp> {
        self.split_asked_for = timing;
        // Leave given earlier was for what was on disk then.
        self.replacing_agreed_for = None;
        self.export_obstacle = self.obstacle_to_a_split(found);
        match self.export_obstacle {
            None => Some(timing.follow_up()),
            Some(_) => None,
        }
    }

    fn obstacle_to_a_split(
        &self,
        found: Result<EarlierSceneFiles, String>,
    ) -> Option<ExportObstacle> {
        // Before the files: moving the splits changes the plan, and leave to
        // replace files is given for one plan.
        let off_cut_points = self.splits_off_cut_points_for(self.new_split_precision);
        if !off_cut_points.is_empty() {
            return Some(ExportObstacle::SplitsOffCutPoints(off_cut_points));
        }
        match found {
            Ok(nothing_in_the_way) if nothing_in_the_way.file_count() == 0 => None,
            Ok(earlier_files) => Some(ExportObstacle::EarlierSceneFiles(earlier_files)),
            Err(problem) => Some(ExportObstacle::CouldNotCheck(problem)),
        }
    }

    /// Moves the splits that stand on exact pictures to where a copy can
    /// cut. It is an edit like any other, so Undo takes it back. Those that
    /// cannot move are named, since the split still cannot start with them.
    fn move_splits_to_cut_points(&mut self) {
        let stuck = self.edit_plan(SplitPlan::move_splits_to_cut_points);
        if !stuck.is_empty() {
            self.export_obstacle = Some(ExportObstacle::SplitsStuckOffCutPoints(stuck));
        }
    }

    /// The person agreed to replace the scene files in `confirmed_dir` that
    /// the warning named. That says nothing about any other folder, nor
    /// about other files. If the scenes would go somewhere else by now,
    /// because the setting for it was changed in between, or other files are
    /// found there than the warning named, what is there is asked about like
    /// on a first try.
    fn replace_earlier_files(
        &mut self,
        confirmed_dir: &Path,
        found: Result<EarlierSceneFiles, String>,
    ) -> Option<FollowUp> {
        let shown = match &self.export_obstacle {
            Some(ExportObstacle::EarlierSceneFiles(shown)) => Some(shown),
            _ => None,
        };
        match found {
            Ok(earlier_files)
                if earlier_files.scene_dir == confirmed_dir && shown == Some(&earlier_files) =>
            {
                self.export_obstacle = None;
                self.replacing_agreed_for = Some(earlier_files);
                Some(self.split_asked_for.follow_up())
            }
            found_elsewhere_or_other_files_or_not_at_all => self.ask_to_split(
                found_elsewhere_or_other_files_or_not_at_all,
                self.split_asked_for,
            ),
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
        let focused_moment = self.focused_moment();
        let outcome = edit(&mut self.plan);
        if !self.plan.is_at(&plan_before) {
            self.undo_history.push(plan_before);
            self.forget_what_depended_on_the_old_plan();
            self.focus_the_scene_showing(focused_moment);
        }
        outcome
    }

    fn undo(&mut self) {
        let Some(earlier_plan) = self.undo_history.pop() else {
            return;
        };
        let focused_moment = self.focused_moment();
        self.plan.restore(earlier_plan);
        self.forget_what_depended_on_the_old_plan();
        self.focus_the_scene_showing(focused_moment);
    }

    /// Where the focused scene starts, to find the focus again by once the
    /// scenes are renumbered.
    fn focused_moment(&self) -> Option<f64> {
        self.focused_layout().map(|layout| layout.span.start)
    }

    /// Keeps the focus on the footage it was on, so a person working down
    /// the list with the keys does not lose their place at every change.
    /// The pictures of that scene may be others now, so none stays chosen.
    fn focus_the_scene_showing(&mut self, moment: Option<f64>) {
        self.focus = moment
            .and_then(|moment| focus::scene_showing(&self.plan.scenes(), moment))
            .map(ReviewFocus::on);
    }

    fn focused_layout(&self) -> Option<SceneLayout> {
        let focused = self.focus?.scene().position();
        self.plan.scene_layouts().into_iter().nth(focused)
    }

    fn step_focus(&mut self, step: SceneStep, kind: ClickKind) {
        let focused = self.focus.map(ReviewFocus::scene);
        let Some(scene) = step.from(focused, self.scene_count()) else {
            return;
        };
        // An edit clears the selection and leaves the focus. A range that
        // is extended from there starts at the scene the keys were on.
        let nothing_selected = self.selection.scenes().next().is_none();
        if let (ClickKind::ExtendRange, true, Some(focused)) = (kind, nothing_selected, focused) {
            self.selection = SceneSelection::only(focused);
        }
        self.selection.click(scene, kind);
        self.focus = Some(ReviewFocus::on(scene));
    }

    fn step_picture(&mut self, step: PictureStep) {
        let (Some(focus), Some(layout)) = (self.focus, self.focused_layout()) else {
            return;
        };
        let picture_count = focus::picture_count(layout.segments.len());
        let picture = step.from(focus.picture(), picture_count);
        self.focus = Some(focus.with_picture(picture));
    }

    fn join_focused_with_next(&mut self) {
        let Some(focus) = self.focus else {
            return;
        };
        let next = focus.scene().next();
        if next.position() >= self.scene_count() {
            return;
        }
        self.merge_scenes(focus.scene()..=next);
    }

    fn nudge_split(&mut self, scene: SceneIndex, direction: NudgeDirection) {
        let step = self.nudge_step();
        let nudged = self.edit_plan(|plan| plan.nudge_cut_before(scene, direction, step));
        self.new_split_feedback = Some(match nudged {
            Ok(moved) => {
                // The scene keeps its number but not its start, which is
                // what the focus is found again by.
                self.focus = Some(ReviewFocus::on(scene));
                SplitFeedback::Moved(moved)
            }
            Err(why) => SplitFeedback::MoveRefused(why),
        });
    }

    /// Scene numbers shift when the plan changes, so a selection made
    /// before would point at other scenes. A warning about existing files
    /// was likewise given for the plan as it was, and so was the word that
    /// a scene could not be dropped or that the scene list was saved.
    fn forget_what_depended_on_the_old_plan(&mut self) {
        self.selection.clear();
        self.forget_export_obstacle();
        self.drop_refusal = None;
        self.scene_list_note = None;
    }

    /// Drops the selected scenes or, with nothing selected, the focused
    /// one. When every one of them is dropped already, the same key brings
    /// them back.
    ///
    /// An edit clears the selection and leaves the focus, so the key goes
    /// on working on the scene the keys are on, as the key that takes a
    /// merged scene apart does.
    fn drop_or_keep_selected(&mut self) {
        let mut selected: Vec<SceneIndex> = self.selection.scenes().collect();
        if selected.is_empty() {
            selected.extend(self.focus.map(ReviewFocus::scene));
        }
        if selected.is_empty() {
            return;
        }
        let fates = self.plan.scene_fates();
        let all_dropped = selected
            .iter()
            .all(|scene| fates.get(scene.position()) == Some(&SceneFate::Drop));
        let wanted = if all_dropped {
            SceneFate::Keep
        } else {
            SceneFate::Drop
        };
        self.set_fate_of(selected, wanted);
    }

    /// Marks all of `scenes` as written or dropped, or none of them: when
    /// that would leave no scene to write, nothing changes and the person
    /// is told why.
    fn set_fate_of(&mut self, scenes: Vec<SceneIndex>, fate: SceneFate) {
        // Dropping a scene moves no scene to another place in the list, so
        // what was selected still is.
        let selection = std::mem::take(&mut self.selection);
        let outcome = self.edit_plan(|plan| {
            let plan_before = plan.snapshot();
            for scene in scenes {
                if let Err(refusal) = plan.set_scene_fate(scene, fate) {
                    plan.restore(plan_before);
                    return Err(refusal);
                }
            }
            Ok(())
        });
        self.selection = selection;
        self.drop_refusal = outcome.err();
    }

    fn merge_scenes(&mut self, scenes: RangeInclusive<SceneIndex>) {
        let merged_scene = *scenes.start();
        self.edit_plan(|plan| plan.merge_scenes(scenes));
        self.selection = SceneSelection::only(merged_scene);
        self.focus = Some(ReviewFocus::on(merged_scene));
    }

    fn take_selected_apart(&mut self) {
        let selected_spans: Vec<TimeSpan> = self
            .layouts_to_take_apart()
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
        match self.new_split_precision {
            CutPrecision::NearestKeyframe => self.edit_plan(|plan| plan.add_cut_near(time)),
            CutPrecision::ExactFrame => self.edit_plan(|plan| plan.add_cut_exactly_at(time)),
        }
    }

    /// The selected scenes or, with nothing selected, the focused one. An
    /// edit clears the selection, and the key that takes a merged scene
    /// apart should still work on the scene the keys are on.
    fn layouts_to_take_apart(&self) -> Vec<SceneLayout> {
        let layouts = self.plan.scene_layouts();
        let selected: Vec<SceneLayout> = self
            .selection
            .scenes()
            .filter_map(|scene| layouts.get(scene.position()).cloned())
            .collect();
        if !selected.is_empty() {
            return selected;
        }
        self.focused_layout().into_iter().collect()
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
            chapters: crate::detect::ChapterMarks::NeverRead,
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
            splits: EarlierSceneFiles::a_split_into(0),
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
            splits: EarlierSceneFiles::a_split_into(3),
        })
    }

    /// What is found once "Put the scenes in a new folder" has been ticked.
    fn new_folder() -> PathBuf {
        PathBuf::from("videos/no_such_video_scenes")
    }

    fn two_earlier_files_in_the_new_folder() -> Result<EarlierSceneFiles, String> {
        Ok(EarlierSceneFiles {
            scene_dir: new_folder(),
            splits: EarlierSceneFiles::a_split_into(2),
        })
    }

    fn an_empty_new_folder() -> Result<EarlierSceneFiles, String> {
        Ok(EarlierSceneFiles {
            scene_dir: new_folder(),
            splits: EarlierSceneFiles::a_split_into(0),
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
            "The scan looked at 0 of 3000 pictures to find them."
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

    fn fates(session: &Session) -> Vec<SceneFate> {
        session.0.scene_fates()
    }

    const KEEP: SceneFate = SceneFate::Keep;
    const DROP: SceneFate = SceneFate::Drop;

    #[test]
    fn a_dropped_scene_is_not_written_and_the_others_are() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropScene(scene(1)));

        assert_eq!(fates(&session), [KEEP, DROP, KEEP, KEEP, KEEP]);
        assert_eq!(session.0.dropped_count(), 1);
        assert_eq!(session.0.scene_count(), 5);
        assert_eq!(session.0.scenes_to_write().kept_scenes().len(), 4);
        assert!(session.0.is_edited());
    }

    #[test]
    fn dropping_a_scene_can_be_undone() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropScene(scene(1)));
        session.ask(ReviewRequest::Undo);

        assert_eq!(session.0.dropped_count(), 0);
        assert!(!session.0.is_edited());
    }

    #[test]
    fn a_dropped_scene_can_be_kept_again() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropScene(scene(1)));
        session.ask(ReviewRequest::KeepScene(scene(1)));
        assert_eq!(session.0.dropped_count(), 0);
        // Two changes were made, and each can be taken back.
        session.ask(ReviewRequest::Undo);
        assert_eq!(session.0.dropped_count(), 1);
    }

    #[test]
    fn keeping_a_scene_that_is_kept_already_is_not_a_change() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::KeepScene(scene(1)));
        assert!(!session.0.can_undo());
    }

    #[test]
    fn dropping_a_scene_leaves_the_selection_as_it_was() {
        let mut session = Session::five_scenes();
        session.ask(click(1, ClickKind::Single));
        session.ask(click(3, ClickKind::Toggle));
        session.ask(ReviewRequest::DropScene(scene(1)));
        assert_eq!(session.selected(), vec![1, 3]);
    }

    #[test]
    fn the_key_drops_the_selected_scenes_and_then_keeps_them_again() {
        let mut session = Session::five_scenes();
        session.ask(click(1, ClickKind::Single));
        session.ask(click(3, ClickKind::Toggle));

        session.ask(ReviewRequest::DropOrKeepSelected);
        assert_eq!(fates(&session), [KEEP, DROP, KEEP, DROP, KEEP]);
        session.ask(ReviewRequest::DropOrKeepSelected);
        assert_eq!(fates(&session), [KEEP, KEEP, KEEP, KEEP, KEEP]);
    }

    #[test]
    fn the_key_drops_the_rest_of_a_selection_that_is_partly_dropped() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropScene(scene(1)));
        session.ask(click(1, ClickKind::Single));
        session.ask(click(2, ClickKind::Toggle));

        session.ask(ReviewRequest::DropOrKeepSelected);
        assert_eq!(fates(&session), [KEEP, DROP, DROP, KEEP, KEEP]);
    }

    #[test]
    fn the_key_does_nothing_without_a_selection() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropOrKeepSelected);
        assert!(!session.0.can_undo());
        assert_eq!(session.0.drop_refusal(), None);
    }

    #[test]
    fn dropping_every_scene_is_refused_and_drops_none_of_them() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::SelectAllScenes);
        session.ask(ReviewRequest::DropOrKeepSelected);

        assert_eq!(session.0.dropped_count(), 0);
        assert!(!session.0.can_undo());
        assert_eq!(
            session.0.drop_refusal(),
            Some(&NothingToWrite::EverySceneDropped)
        );
        assert_eq!(
            session.0.drop_refusal().unwrap().to_string(),
            "At least one scene has to be written."
        );
    }

    #[test]
    fn dropping_the_last_scene_that_is_written_is_refused() {
        let mut session = Session(test_sessions::scenes_of_twenty_seconds(2));
        session.ask(ReviewRequest::DropScene(scene(0)));
        session.ask(ReviewRequest::DropScene(scene(1)));

        assert_eq!(fates(&session), [DROP, KEEP]);
        assert_eq!(
            session.0.drop_refusal(),
            Some(&NothingToWrite::EverySceneDropped)
        );
        // The refusal goes away with the next change.
        session.ask(ReviewRequest::KeepScene(scene(0)));
        assert_eq!(session.0.drop_refusal(), None);
    }

    #[test]
    fn a_dropped_scene_merged_with_a_kept_one_is_written_again() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropScene(scene(1)));
        session.ask(ReviewRequest::MergeScenes(scenes(1, 2)));

        assert_eq!(fates(&session), [KEEP, KEEP, KEEP, KEEP]);
        // Undoing the merge brings the dropped scene back as it was.
        session.ask(ReviewRequest::Undo);
        assert_eq!(fates(&session), [KEEP, DROP, KEEP, KEEP, KEEP]);
    }

    #[test]
    fn dropped_scenes_merged_with_each_other_stay_dropped() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropScene(scene(1)));
        session.ask(ReviewRequest::DropScene(scene(2)));
        session.ask(ReviewRequest::MergeScenes(scenes(1, 2)));

        assert_eq!(fates(&session), [KEEP, DROP, KEEP, KEEP]);
    }

    #[test]
    fn resetting_the_scenes_writes_every_scene_again() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropScene(scene(4)));
        session.ask(ReviewRequest::ResetScenes);
        assert_eq!(session.0.dropped_count(), 0);
    }

    #[test]
    fn a_scan_that_would_forget_dropped_scenes_asks_first() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::DropScene(scene(4)));

        assert_eq!(
            session.ask(ReviewRequest::Rescan(ScanKind::BlackScreens)),
            None
        );
        assert!(session.0.is_asking_before_rescan());
    }

    #[test]
    fn dropping_a_scene_takes_back_the_warning_about_earlier_files() {
        let mut session = Session::five_scenes();
        session.0.apply(ReviewRequest::Split, &three_earlier_files);
        assert!(session.0.export_obstacle().is_some());

        session.ask(ReviewRequest::DropScene(scene(0)));
        assert_eq!(session.0.export_obstacle(), None);
    }

    #[test]
    fn saving_the_scene_list_is_left_to_the_app_and_its_outcome_is_kept() {
        let mut session = Session::five_scenes();
        let save = ReviewRequest::SaveSceneList {
            format: SceneListFormat::Csv,
            path: PathBuf::from("scenes.csv"),
        };
        assert_eq!(
            session.ask(save),
            Some(FollowUp::SaveSceneList {
                format: SceneListFormat::Csv,
                path: PathBuf::from("scenes.csv"),
            })
        );
        assert_eq!(session.0.scene_list_note(), None);

        let saved = SceneListNote::Saved(PathBuf::from("scenes.csv"));
        session.0.note_scene_list(saved.clone());
        assert_eq!(session.0.scene_list_note(), Some(&saved));
        // The list was of the scenes as they were. A change makes it old.
        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));
        assert_eq!(session.0.scene_list_note(), None);
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
            "Added a split at 00:00:50.000, the nearest point where the video can be cut."
        );
        assert_eq!(
            session.scene_ends(),
            vec![20.0, 40.0, 50.0, 60.0, 80.0, 100.0]
        );
    }

    #[test]
    fn with_exact_cuts_a_split_from_the_player_stays_on_the_chosen_picture() {
        let mut session = Session::five_scenes();
        session.0.place_new_splits(CutPrecision::ExactFrame);
        assert_eq!(session.0.split_time_near(47.36), Some(47.36));

        let feedback = session.0.split_near(47.36);

        assert_eq!(feedback, SplitFeedback::AddedExactly(47.36));
        assert!(!feedback.is_refusal());
        assert_eq!(
            feedback.in_words(),
            "Added a split at exactly 00:00:47.360. A few seconds around it are encoded again when you split."
        );
        assert_eq!(
            session.scene_ends(),
            vec![20.0, 40.0, 47.36, 60.0, 80.0, 100.0]
        );
    }

    #[test]
    fn with_exact_cuts_a_typed_split_stays_at_the_time_typed() {
        let mut session = Session::five_scenes();
        session.0.place_new_splits(CutPrecision::ExactFrame);
        session.add_split_at("0:47.5");

        assert_eq!(
            session.0.new_split_feedback(),
            Some(&SplitFeedback::AddedExactly(47.5))
        );
        assert_eq!(
            session.scene_ends(),
            vec![20.0, 40.0, 47.5, 60.0, 80.0, 100.0]
        );
        // Like any other change, it can be taken back.
        session.ask(ReviewRequest::Undo);
        assert_eq!(session.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn turning_exact_cuts_off_again_puts_new_splits_back_on_keyframes() {
        let mut session = Session::five_scenes();
        session.0.place_new_splits(CutPrecision::ExactFrame);
        session.0.place_new_splits(CutPrecision::NearestKeyframe);

        assert_eq!(session.0.split_time_near(47.36), Some(50.0));
        assert_eq!(session.0.split_near(47.36), SplitFeedback::Added(50.0));
    }

    #[test]
    fn a_video_that_cannot_be_cut_exactly_says_so_before_the_split_while_exact_cuts_are_on() {
        let mut session = test_sessions::five_scenes();
        assert_eq!(session.exact_cut_look(), &ExactCutLook::NotLooked);
        assert_eq!(session.exact_cut_notice_for(CutPrecision::ExactFrame), None);

        let encoder_missing = crate::exact_fallback::WhyNotExact::EncoderMissing;
        session.note_exact_cut_look(ExactCutLook::CannotBeCut(encoder_missing.clone()));
        assert_eq!(
            session.exact_cut_notice_for(CutPrecision::ExactFrame),
            Some(encoder_missing.before_the_split_in_plain_words())
        );
        // With exact cuts off nobody asked for one, so there is nothing to say.
        assert_eq!(
            session.exact_cut_notice_for(CutPrecision::NearestKeyframe),
            None
        );
    }

    #[test]
    fn a_video_that_can_be_cut_exactly_has_no_notice() {
        let mut session = test_sessions::five_scenes();
        session.note_exact_cut_look(ExactCutLook::CanBeTried);
        assert_eq!(session.exact_cut_notice_for(CutPrecision::ExactFrame), None);
    }

    /// Five scenes with one split added on an exact picture, at 47.36 s,
    /// between the points at 40 and 50 s where the video can be cut. Cuts
    /// have been turned off exact again since.
    fn session_with_an_exact_split_and_exact_cuts_off() -> ReviewSession {
        let mut session = test_sessions::five_scenes();
        session.place_new_splits(CutPrecision::ExactFrame);
        assert_eq!(
            session.split_near(47.36),
            SplitFeedback::AddedExactly(47.36)
        );
        session.place_new_splits(CutPrecision::NearestKeyframe);
        session
    }

    #[test]
    fn a_copy_does_not_start_over_a_split_placed_on_an_exact_picture() {
        let mut session = session_with_an_exact_split_and_exact_cuts_off();

        // The files are not even looked for: this comes first.
        assert_eq!(session.apply(ReviewRequest::Split, &no_earlier_files), None);
        assert_eq!(
            session.export_obstacle(),
            Some(&ExportObstacle::SplitsOffCutPoints(vec![47.36]))
        );
        assert_eq!(
            session.apply(ReviewRequest::SplitLater, &no_earlier_files),
            None
        );
        assert!(session.export_obstacle().is_some());
    }

    #[test]
    fn with_exact_cuts_on_a_split_on_an_exact_picture_is_no_obstacle() {
        let mut session = session_with_an_exact_split_and_exact_cuts_off();
        session.place_new_splits(CutPrecision::ExactFrame);

        assert_eq!(
            session.apply(ReviewRequest::Split, &no_earlier_files),
            Some(FollowUp::Export)
        );
        assert_eq!(session.export_obstacle(), None);
    }

    #[test]
    fn moving_the_exact_splits_to_cut_points_lets_the_split_start_and_can_be_undone() {
        let mut session = session_with_an_exact_split_and_exact_cuts_off();
        session.apply(ReviewRequest::Split, &no_earlier_files);

        session.apply(ReviewRequest::MoveSplitsToCutPoints, &no_disk);
        assert_eq!(session.export_obstacle(), None);
        assert_eq!(
            session.accepted_cuts().as_slice(),
            [20.0, 40.0, 50.0, 60.0, 80.0]
        );
        assert_eq!(
            session.apply(ReviewRequest::Split, &no_earlier_files),
            Some(FollowUp::Export)
        );

        session.apply(ReviewRequest::Undo, &no_disk);
        assert_eq!(
            session.accepted_cuts().as_slice(),
            [20.0, 40.0, 47.36, 60.0, 80.0]
        );
    }

    #[test]
    fn an_exact_split_whose_cut_point_is_taken_stays_and_is_named() {
        let mut session = test_sessions::five_scenes();
        session.place_new_splits(CutPrecision::ExactFrame);
        // The nearest point where the video can be cut is 40 s, where the
        // scan's own split stands.
        session.split_near(41.0);
        session.place_new_splits(CutPrecision::NearestKeyframe);
        session.apply(ReviewRequest::Split, &no_earlier_files);

        session.apply(ReviewRequest::MoveSplitsToCutPoints, &no_disk);
        assert_eq!(
            session.export_obstacle(),
            Some(&ExportObstacle::SplitsStuckOffCutPoints(vec![41.0]))
        );
        // And the split still does not start.
        assert_eq!(session.apply(ReviewRequest::Split, &no_earlier_files), None);
        assert_eq!(
            session.export_obstacle(),
            Some(&ExportObstacle::SplitsOffCutPoints(vec![41.0]))
        );
    }

    #[test]
    fn which_splits_a_copy_cannot_make_depends_on_how_the_split_cuts() {
        let session = session_with_an_exact_split_and_exact_cuts_off();
        assert_eq!(
            session.splits_off_cut_points_for(CutPrecision::NearestKeyframe),
            vec![47.36]
        );
        assert_eq!(
            session.splits_off_cut_points_for(CutPrecision::ExactFrame),
            Vec::<f64>::new()
        );
    }

    #[test]
    fn a_video_held_back_from_the_ready_run_goes_back_to_the_ready_ones_on_agreeing() {
        let mut session = test_sessions::five_scenes();
        // The person once asked for this video to be split at once, and
        // then marked it ready after all.
        session.apply(ReviewRequest::Split, &three_earlier_files);
        let Ok(found) = three_earlier_files() else {
            panic!("the fixture finds three files");
        };
        let scene_dir = found.scene_dir.clone();

        session.hold_back_from_the_ready_run(ExportObstacle::EarlierSceneFiles(found));
        assert!(matches!(
            session.export_obstacle(),
            Some(ExportObstacle::EarlierSceneFiles(_))
        ));
        // Agreeing does not split this one video by itself, beside a run
        // that may still be going.
        assert_eq!(
            session.apply(
                ReviewRequest::ReplaceEarlierFiles { scene_dir },
                &three_earlier_files
            ),
            Some(FollowUp::MarkReady)
        );
    }

    #[test]
    fn chapter_marks_that_could_not_be_read_are_said_and_the_review_goes_on() {
        let mut detection = test_sessions::scenes_of_twenty_seconds_detected(5);
        detection.chapters =
            crate::detect::ChapterMarks::CouldNotRead("ffprobe ran out of time".to_owned());
        let session = test_sessions::fresh_scan_of(detection);

        assert_eq!(session.scene_count(), 5);
        assert_eq!(
            session.scan_summary(),
            "Found 0 black screens. The chapter marks of this file could not be read."
        );
        assert!(
            session
                .scan_details()
                .ends_with("(ffprobe ran out of time)"),
            "{}",
            session.scan_details()
        );
    }

    #[test]
    fn an_exact_split_outside_the_video_is_refused() {
        let mut session = Session::five_scenes();
        session.0.place_new_splits(CutPrecision::ExactFrame);

        assert_eq!(session.0.split_time_near(100.0), None);
        assert_eq!(
            session.0.split_near(100.0),
            SplitFeedback::Refused(AddCutError::OutsideVideo)
        );
        assert!(!session.0.is_edited());
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
            "There is already a split at 00:00:40.000."
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

    fn nudge(position: usize, direction: NudgeDirection) -> ReviewRequest {
        ReviewRequest::NudgeSplit {
            scene: scene(position),
            direction,
        }
    }

    fn step(step: SceneStep) -> ReviewRequest {
        ReviewRequest::StepFocus {
            step,
            kind: ClickKind::Single,
        }
    }

    fn step_extending(step: SceneStep) -> ReviewRequest {
        ReviewRequest::StepFocus {
            step,
            kind: ClickKind::ExtendRange,
        }
    }

    impl Session {
        fn focused(&self) -> Option<usize> {
            self.0.focus().map(|focus| focus.scene().position())
        }

        fn chosen_picture(&self) -> Option<usize> {
            self.0.focus().and_then(ReviewFocus::picture)
        }
    }

    #[test]
    fn a_split_is_moved_to_the_next_cut_point_and_the_new_time_is_said() {
        let mut session = Session::five_scenes();
        session.ask(nudge(2, NudgeDirection::Later));

        assert_eq!(session.scene_ends(), vec![20.0, 50.0, 60.0, 80.0, 100.0]);
        let feedback = session.0.new_split_feedback().unwrap();
        assert_eq!(
            feedback,
            &SplitFeedback::Moved(MovedCut {
                from: 40.0,
                to: 50.0
            })
        );
        assert!(!feedback.is_refusal());
        assert_eq!(
            feedback.in_words(),
            "Moved the split from 00:00:40.000 to 00:00:50.000."
        );
    }

    #[test]
    fn a_moved_split_can_be_undone() {
        let mut session = Session::five_scenes();
        session.ask(nudge(2, NudgeDirection::Earlier));
        assert_eq!(session.scene_ends(), vec![20.0, 30.0, 60.0, 80.0, 100.0]);
        assert!(session.0.is_edited());

        session.ask(ReviewRequest::Undo);
        assert_eq!(session.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
        assert!(!session.0.is_edited());
    }

    #[test]
    fn a_split_that_cannot_move_stays_and_the_reason_is_said() {
        let mut session = Session::five_scenes();
        session.ask(nudge(2, NudgeDirection::Later));
        // The split is at 50 now, and the next cut point is the split at 60.
        session.ask(nudge(2, NudgeDirection::Later));

        assert_eq!(session.scene_ends(), vec![20.0, 50.0, 60.0, 80.0, 100.0]);
        let feedback = session.0.new_split_feedback().unwrap();
        assert!(feedback.is_refusal());
        assert_eq!(
            feedback.in_words(),
            "The split cannot move further. The split at 00:01:00.000 is in the way."
        );
        // The refused move left nothing more to undo than the first one.
        session.ask(ReviewRequest::Undo);
        assert!(!session.0.can_undo());
    }

    #[test]
    fn the_first_scene_has_no_split_to_move() {
        let mut session = Session::five_scenes();
        session.ask(nudge(0, NudgeDirection::Later));

        assert!(!session.0.can_undo());
        assert_eq!(
            session.0.new_split_feedback(),
            Some(&SplitFeedback::MoveRefused(
                NudgeCutError::NoSplitBeforeScene
            ))
        );
    }

    #[test]
    fn moving_a_split_keeps_the_keys_on_the_scene_it_starts() {
        let mut session = Session::five_scenes();
        session.ask(click(2, ClickKind::Single));

        // Moved later, the moment the scene started at is in the scene
        // before it. The keys stay on the scene whose split it is.
        session.ask(nudge(2, NudgeDirection::Later));
        assert_eq!(session.focused(), Some(2));
        session.ask(nudge(2, NudgeDirection::Earlier));
        session.ask(nudge(2, NudgeDirection::Earlier));
        assert_eq!(session.focused(), Some(2));
        assert_eq!(session.scene_ends(), vec![20.0, 30.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn a_reset_puts_a_moved_split_back() {
        let mut session = Session::five_scenes();
        session.ask(nudge(2, NudgeDirection::Later));

        session.ask(ReviewRequest::ResetScenes);
        assert_eq!(session.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
    }

    #[test]
    fn nothing_is_focused_until_the_person_steps_or_clicks() {
        let mut session = Session::five_scenes();
        assert_eq!(session.focused(), None);
        assert_eq!(session.0.focused_play_time(), None);

        session.ask(step(SceneStep::Next));
        assert_eq!(session.focused(), Some(0));
    }

    #[test]
    fn stepping_moves_the_focus_and_selects_the_scene_it_lands_on() {
        let mut session = Session::five_scenes();
        session.ask(step(SceneStep::Next));
        session.ask(step(SceneStep::Next));
        assert_eq!(session.focused(), Some(1));
        assert_eq!(session.selected(), vec![1]);

        session.ask(step(SceneStep::Last));
        assert_eq!(session.focused(), Some(4));
        session.ask(step(SceneStep::Next));
        assert_eq!(session.focused(), Some(4));

        session.ask(step(SceneStep::First));
        session.ask(step(SceneStep::Previous));
        assert_eq!(session.focused(), Some(0));
        assert_eq!(session.selected(), vec![0]);
    }

    #[test]
    fn stepping_with_shift_extends_the_selection_from_where_it_began() {
        let mut session = Session::five_scenes();
        session.ask(click(1, ClickKind::Single));
        session.ask(step_extending(SceneStep::Next));
        session.ask(step_extending(SceneStep::Next));
        assert_eq!(session.selected(), vec![1, 2, 3]);
        assert_eq!(session.focused(), Some(3));

        session.ask(step_extending(SceneStep::Previous));
        assert_eq!(session.selected(), vec![1, 2]);

        session.ask(ReviewRequest::MergeSelected);
        assert_eq!(session.scene_ends(), vec![20.0, 60.0, 80.0, 100.0]);
        assert_eq!(session.focused(), Some(1));
    }

    #[test]
    fn a_range_extended_after_an_edit_starts_at_the_focused_scene() {
        let mut session = Session::five_scenes();
        session.ask(click(1, ClickKind::Single));
        // The edit clears the selection and leaves the focus where it was.
        session.ask(nudge(1, NudgeDirection::Later));
        assert!(session.selected().is_empty());

        session.ask(step_extending(SceneStep::Next));
        assert_eq!(session.selected(), vec![1, 2]);
    }

    #[test]
    fn a_click_moves_the_focus_to_the_clicked_scene() {
        let mut session = Session::five_scenes();
        session.ask(click(3, ClickKind::Single));
        assert_eq!(session.focused(), Some(3));

        session.ask(ReviewRequest::ClearSelection);
        // Escape clears the selection. The person keeps their place.
        assert_eq!(session.focused(), Some(3));
    }

    #[test]
    fn the_focused_scene_plays_from_its_start() {
        let mut session = Session::five_scenes();
        session.ask(click(2, ClickKind::Single));
        assert_eq!(
            session.ask(ReviewRequest::PlayFocused),
            Some(FollowUp::PlayFrom(40.0))
        );
    }

    #[test]
    fn with_nothing_focused_there_is_nothing_to_play() {
        let mut session = Session::five_scenes();
        assert_eq!(session.ask(ReviewRequest::PlayFocused), None);
    }

    #[test]
    fn a_chosen_picture_is_where_the_focused_scene_plays_from() {
        let mut session = Session::five_scenes();
        session.ask(click(2, ClickKind::Single));
        session.ask(ReviewRequest::StepPicture(PictureStep::Next));
        session.ask(ReviewRequest::StepPicture(PictureStep::Next));
        assert_eq!(session.chosen_picture(), Some(1));

        // The scene runs from 40 to 60 seconds, with a picture every five
        // seconds from 42.5 on.
        assert_eq!(
            session.ask(ReviewRequest::PlayFocused),
            Some(FollowUp::PlayFrom(47.5))
        );

        session.ask(ReviewRequest::StepPicture(PictureStep::Previous));
        session.ask(ReviewRequest::StepPicture(PictureStep::Previous));
        assert_eq!(session.chosen_picture(), None);
    }

    #[test]
    fn the_pictures_of_a_merged_scene_run_through_all_its_parts() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));
        for _ in 0..20 {
            session.ask(ReviewRequest::StepPicture(PictureStep::Next));
        }

        // Two parts of four pictures each, the last one 2.5 seconds before
        // the end of the second part.
        assert_eq!(session.chosen_picture(), Some(7));
        assert_eq!(
            session.ask(ReviewRequest::PlayFocused),
            Some(FollowUp::PlayFrom(37.5))
        );
    }

    #[test]
    fn stepping_to_another_scene_forgets_the_chosen_picture() {
        let mut session = Session::five_scenes();
        session.ask(click(2, ClickKind::Single));
        session.ask(ReviewRequest::StepPicture(PictureStep::Next));

        session.ask(step(SceneStep::Next));
        assert_eq!(session.chosen_picture(), None);
    }

    #[test]
    fn with_nothing_focused_there_is_no_picture_to_choose() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::StepPicture(PictureStep::Next));
        assert_eq!(session.0.focus(), None);
    }

    #[test]
    fn the_focused_scene_is_joined_with_the_next_and_stays_focused() {
        let mut session = Session::five_scenes();
        session.ask(click(1, ClickKind::Single));
        session.ask(ReviewRequest::JoinFocusedWithNext);

        assert_eq!(session.scene_ends(), vec![20.0, 60.0, 80.0, 100.0]);
        assert_eq!(session.focused(), Some(1));
        assert_eq!(session.selected(), vec![1]);
    }

    #[test]
    fn the_last_scene_has_no_next_scene_to_join() {
        let mut session = Session::five_scenes();
        session.ask(click(4, ClickKind::Single));
        session.ask(ReviewRequest::JoinFocusedWithNext);
        assert_eq!(session.0.scene_count(), 5);
        assert!(!session.0.can_undo());

        let mut nothing_focused = Session::five_scenes();
        nothing_focused.ask(ReviewRequest::JoinFocusedWithNext);
        assert_eq!(nothing_focused.0.scene_count(), 5);
    }

    #[test]
    fn the_focus_stays_on_its_footage_when_the_scenes_are_renumbered() {
        let mut session = Session::five_scenes();
        session.ask(click(3, ClickKind::Single));

        // Splitting the first scene in two renumbers every scene after it.
        session.0.split_near(10.0);
        assert_eq!(session.focused(), Some(4));

        session.ask(ReviewRequest::Undo);
        assert_eq!(session.focused(), Some(3));
    }

    #[test]
    fn a_merge_done_with_the_mouse_puts_the_focus_on_the_merged_scene() {
        let mut session = Session::five_scenes();
        session.ask(click(4, ClickKind::Single));
        session.ask(ReviewRequest::MergeScenes(scenes(0, 1)));
        assert_eq!(session.focused(), Some(0));
    }

    #[test]
    fn with_nothing_selected_the_focused_merged_scene_is_taken_apart() {
        let mut session = Session::five_scenes();
        session.ask(ReviewRequest::MergeScenes(scenes(1, 2)));
        session.ask(ReviewRequest::ClearSelection);
        assert!(session.0.has_merged_scene_selected());

        session.ask(ReviewRequest::TakeSelectedApart);
        assert_eq!(session.scene_ends(), vec![20.0, 40.0, 60.0, 80.0, 100.0]);
        assert_eq!(session.focused(), Some(1));
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
                splits: EarlierSceneFiles::a_split_into(3),
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
                splits: EarlierSceneFiles::a_split_into(2),
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
        detection.chapters = crate::detect::ChapterMarks::Read(vec![
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
            "The scan looked at 0 of 3000 pictures to find them. The file has 4 chapter marks."
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
        detection.chapters = crate::detect::ChapterMarks::Read(vec![chapter(31.0, "The plan")]);
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
    fn with_exact_cuts_a_split_moves_by_one_picture_and_keeps_its_chapter_title() {
        let mut session = five_scenes_with_chapters();
        assert_eq!(session.nudge_step(), NudgeStep::ToNextCutPoint);
        session.place_new_splits(CutPrecision::ExactFrame);
        let one_picture = session.frame_seconds();
        assert_eq!(
            session.nudge_step(),
            NudgeStep::OnePicture {
                seconds: one_picture
            }
        );

        // The third scene starts at 30, where the chapter "The plan" does.
        let third_scene = SceneIndex::new(2);
        let moved = ReviewRequest::NudgeSplit {
            scene: third_scene,
            direction: NudgeDirection::Later,
        };
        session.apply(moved, &no_disk);

        let scene = session.scenes()[2];
        assert_eq!(scene.start, 30.0 + one_picture);
        assert_eq!(session.chapter_title(scene), Some("The plan"));
        assert_eq!(session.chapter_split_count(), 2);

        session.apply(ReviewRequest::Undo, &no_disk);
        assert_eq!(session.scenes()[2].start, 30.0);
    }

    #[test]
    fn the_drop_key_works_on_the_focused_scene_when_none_is_selected() {
        let mut session = five_scenes_with_chapters();
        session.apply(
            ReviewRequest::StepFocus {
                step: SceneStep::Next,
                kind: ClickKind::Single,
            },
            &no_disk,
        );
        session.apply(ReviewRequest::ClearSelection, &no_disk);
        assert_eq!(session.selection().scenes().count(), 0);
        assert!(session.focus().is_some());

        session.apply(ReviewRequest::DropOrKeepSelected, &no_disk);
        assert_eq!(session.dropped_count(), 1);
        session.apply(ReviewRequest::DropOrKeepSelected, &no_disk);
        assert_eq!(session.dropped_count(), 0);
    }

    #[test]
    fn a_dropped_scene_keeps_its_chapter_title_and_the_files_are_numbered_without_it() {
        let mut session = five_scenes_with_chapters();
        // The last scene begins at 90, with the chapter "Credits".
        let credits = SceneIndex::new(session.scene_count() - 1);
        session.apply(ReviewRequest::DropScene(credits), &no_disk);

        let scenes = session.scenes();
        assert_eq!(
            session.chapter_title(scenes[credits.position()]),
            Some("Credits")
        );
        let to_write = session.scenes_to_write();
        assert_eq!(to_write.dropped_count(), 1);
        assert_eq!(to_write.kept_scenes().len(), scenes.len() - 1);
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
