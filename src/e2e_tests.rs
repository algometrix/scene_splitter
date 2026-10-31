//! End-to-end tests: generate a real video, then scan, split and verify it
//! with a real ffmpeg.
//!
//! They are ignored by default because they need `ffmpeg` and `ffprobe` on
//! the PATH. Run them with `cargo test --release -- --include-ignored`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::black::BlackCriteria;
use crate::detect::{self, Detection, ScanDepth, ScanMode, ScanProgress, ScanSettings, ScanStage};
use crate::export::{
    self, OriginalAlwaysKept, OriginalOutcome, OriginalPolicy, SplitMethod, SplitOrder, SplitStage,
    Verification,
};
use crate::fates::{SceneFate, ScenesToWrite};
use crate::ffmpeg::{FfmpegError, Program, ShowTimeOption, STEADY_OUTPUT_LIMIT};
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::probe;
use crate::scene_name::{EarlierNaming, SceneNamePattern};
use crate::scenes::CutTimes;
use crate::split::{self, ScenesNotCut};
use crate::test_videos::{
    ChapterContainer, TestChapter, TestVideo, BLACK_SECONDS, FRAME_RATE, FRAME_SECONDS,
    SCENE_SECONDS,
};
use crate::thumbnails::{self, THUMBNAIL_HEIGHT, THUMBNAIL_WIDTH};
use crate::timecode::TimeSpan;
use crate::tools;
use crate::verify;
use crate::windows::FullDecodeBudget;

const BYTES_PER_RGB_PIXEL: usize = 3;
/// Where the black screen of the test video begins and ends.
const BLACK_START: f64 = SCENE_SECONDS;
const BLACK_END: f64 = SCENE_SECONDS + BLACK_SECONDS;

fn scan_settings(mode: ScanMode) -> ScanSettings {
    ScanSettings {
        mode,
        criteria: BlackCriteria::default(),
    }
}

fn scan(video: &TestVideo, mode: ScanMode, depth: ScanDepth) -> Detection {
    detect::scan_for_scene_changes(
        &video.path,
        &scan_settings(mode),
        depth,
        &detect::no_scan_progress,
    )
    .unwrap()
    .detection
}

/// Timestamps come back from the container rounded, so they are compared
/// to within a frame.
fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < FRAME_SECONDS,
        "{actual} is not within a frame of {expected}"
    );
}

fn assert_one_black_screen_between_the_scenes(black_screens: &[TimeSpan]) {
    assert_eq!(black_screens.len(), 1, "{black_screens:?}");
    assert_close(black_screens[0].start, BLACK_START);
    assert_close(black_screens[0].end, BLACK_END);
}

/// Splits the video at its black screen and returns the scene files.
fn split_at_the_black_screen(video: &TestVideo) -> Vec<PathBuf> {
    let detection = scan(video, ScanMode::EveryFrame, ScanDepth::BlackScreens);
    split::split_at_keyframes(&video.path, &detected_cuts(&detection), video.folder()).unwrap()
}

/// The black-screen cuts of a scan, as what a split takes.
/// A split that copies at keyframes, with this choice about the original.
fn copying(original_policy: OriginalPolicy) -> SplitMethod {
    SplitMethod::CopyAtKeyframes { original_policy }
}

fn detected_cuts(detection: &Detection) -> CutTimes {
    CutTimes::inside(detection.video, detection.cuts.clone()).unwrap()
}

/// Cuts at the given times of the test video with a black screen.
fn cuts_at(times: &[f64]) -> CutTimes {
    let whole_video = TimeSpan {
        start: 0.0,
        end: 2.0 * SCENE_SECONDS + BLACK_SECONDS,
    };
    CutTimes::inside(whole_video, times.to_vec()).unwrap()
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_packet_table_lists_every_frame_and_keyframe() {
    let video = TestVideo::two_scenes_with_a_black_screen("packet_table");
    let index = probe::read_packet_index(&video.path).unwrap();

    let whole_seconds = (2.0 * SCENE_SECONDS + BLACK_SECONDS) as usize;
    assert_eq!(index.packets.len(), whole_seconds * FRAME_RATE as usize);
    assert_eq!(index.keyframe_times().len(), whole_seconds);
    assert_close(index.video.start, 0.0);
    assert_close(index.video.end, whole_seconds as f64);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn decoding_every_frame_finds_the_black_screen() {
    let video = TestVideo::two_scenes_with_a_black_screen("every_frame");
    let detection = scan(&video, ScanMode::EveryFrame, ScanDepth::BlackScreens);

    assert_one_black_screen_between_the_scenes(&detection.black_screens);
    assert_eq!(detection.decoded_frames, detection.frame_count);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_too_large_to_decode_in_full_is_scanned_by_its_packet_sizes_alone() {
    // The test video is short, so only a budget of nothing makes it too large.
    const NO_FULL_DECODE: FullDecodeBudget = FullDecodeBudget { pixels: 0 };
    let video = TestVideo::two_scenes_with_a_black_screen("fast_scan");
    let detection = detect::scan_within_budget(
        &video.path,
        &scan_settings(ScanMode::Suspicious),
        ScanDepth::BlackScreens,
        NO_FULL_DECODE,
        &detect::no_scan_progress,
    )
    .unwrap()
    .detection;

    assert_one_black_screen_between_the_scenes(&detection.black_screens);
    assert!(
        detection.decoded_frames < detection.frame_count / 2,
        "decoded {} of {} frames",
        detection.decoded_frames,
        detection.frame_count
    );
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_short_video_is_decoded_in_full_without_being_asked() {
    let video = TestVideo::two_scenes_with_a_black_screen("cheap_full_decode");
    let detection = scan(&video, ScanMode::Suspicious, ScanDepth::BlackScreens);

    assert_one_black_screen_between_the_scenes(&detection.black_screens);
    assert_eq!(detection.decoded_frames, detection.frame_count);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_cut_lands_on_the_keyframe_inside_the_black_screen() {
    let video = TestVideo::two_scenes_with_a_black_screen("cut");
    let detection = scan(&video, ScanMode::EveryFrame, ScanDepth::BlackScreens);

    assert_eq!(detection.cuts.len(), 1);
    assert_close(detection.cuts[0], BLACK_START);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_without_black_has_no_black_screen_cuts() {
    let video = TestVideo::two_scenes_with_a_hard_cut("no_black");
    let detection = scan(&video, ScanMode::EveryFrame, ScanDepth::BlackScreens);

    assert!(detection.black_screens.is_empty());
    assert!(detection.cuts.is_empty());
    assert_eq!(detection.hard_cut_candidates, None);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_hard_cut_scan_finds_the_change_of_scene() {
    let video = TestVideo::two_scenes_with_a_hard_cut("hard_cut");
    let detection = scan(&video, ScanMode::Suspicious, ScanDepth::WithHardCuts);

    let candidates = detection.hard_cut_candidates.as_ref().unwrap();
    assert!(
        candidates
            .iter()
            .any(|candidate| (candidate.time - SCENE_SECONDS).abs() < FRAME_SECONDS),
        "{candidates:?}"
    );

    let settings = HardCutSettings {
        sensitivity: Sensitivity::Medium,
        min_scene_seconds: 2.0,
    };
    let cuts = detection.cuts_with_hard_cuts(&settings);
    assert_eq!(cuts.len(), 1, "{cuts:?}");
    assert_close(cuts[0], SCENE_SECONDS);
}

const OPENING_TITLE: &str = "Opening";
const MIDDLE_TITLE: &str = "Middle";
/// A chapter that begins between two points where the video can be cut,
/// nearer to the one before it, and far from the black screen.
const MIDDLE_CHAPTER_START: f64 = 2.3;
const MIDDLE_CHAPTER_CUT: f64 = 2.0;
/// A chapter that begins inside the black screen, a little after the point
/// the scan cuts at.
const LAST_CHAPTER_START: f64 = BLACK_START + 0.6;

/// Three chapters for the video with a black screen: one that opens it, one
/// in the middle of the first scene, and an unnamed one at the black screen.
fn three_chapters() -> [TestChapter; 3] {
    [
        TestChapter {
            start_seconds: 0.0,
            title: Some(OPENING_TITLE),
        },
        TestChapter {
            start_seconds: MIDDLE_CHAPTER_START,
            title: Some(MIDDLE_TITLE),
        },
        TestChapter {
            start_seconds: LAST_CHAPTER_START,
            title: None,
        },
    ]
}

fn assert_the_three_chapters_were_read(detection: &Detection) {
    let chapters = detection.chapter_marks();
    assert_eq!(chapters.len(), 3, "{chapters:?}");
    assert_close(chapters[0].start, 0.0);
    assert_close(chapters[1].start, MIDDLE_CHAPTER_START);
    assert_close(chapters[2].start, LAST_CHAPTER_START);
    let titles: Vec<Option<&str>> = chapters
        .iter()
        .map(|chapter| chapter.title.as_deref())
        .collect();
    assert_eq!(titles, [Some(OPENING_TITLE), Some(MIDDLE_TITLE), None]);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_without_chapters_is_scanned_as_having_none() {
    let video = TestVideo::two_scenes_with_a_black_screen("no_chapters");
    let detection = scan(&video, ScanMode::Suspicious, ScanDepth::BlackScreens);

    // Looked for and not found, which is not the same as never looked for.
    assert_eq!(
        detection.chapters,
        crate::detect::ChapterMarks::Read(Vec::new())
    );
    assert!(detection.chapter_splits(&detection.cuts).cuts.is_empty());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn chapter_marks_are_read_and_offered_as_splits_beside_the_black_screen() {
    let video = TestVideo::two_scenes_with_chapters(
        "chapters",
        ChapterContainer::Matroska,
        &three_chapters(),
    );
    let detection = scan(&video, ScanMode::EveryFrame, ScanDepth::BlackScreens);
    assert_the_three_chapters_were_read(&detection);
    assert_eq!(detection.cuts.len(), 1, "{:?}", detection.cuts);
    let black_screen_cut = detection.cuts[0];

    let chapter_splits = detection.chapter_splits(&detection.cuts);
    // The middle chapter gets a split of its own where the video can be
    // cut. The last one sits at the black screen and adds none.
    assert_eq!(chapter_splits.cuts.len(), 1, "{chapter_splits:?}");
    assert_close(chapter_splits.cuts[0], MIDDLE_CHAPTER_CUT);
    assert_eq!(
        chapter_splits.title_of_scene_starting_at(detection.video.start),
        Some(OPENING_TITLE)
    );
    assert_eq!(
        chapter_splits.title_of_scene_starting_at(chapter_splits.cuts[0]),
        Some(MIDDLE_TITLE)
    );
    assert_eq!(
        chapter_splits.title_of_scene_starting_at(black_screen_cut),
        None
    );

    // Split at the black screen and at the chapter, the scenes hold the
    // whole video.
    let every_cut = vec![chapter_splits.cuts[0], black_screen_cut];
    let cuts = CutTimes::inside(detection.video, every_cut).unwrap();
    let order = SplitOrder::new(
        ScenesToWrite::every_scene(cuts),
        copying(OriginalPolicy::Keep),
    );
    let report = export::export_scenes(
        &video.path,
        &order,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();
    assert_eq!(report.scene_files.len(), 3);
    assert!(report.is_clean(), "{:?}", report.verification);
    assert!(video.path.is_file());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn an_mp4_keeps_its_chapters_in_a_stream_the_check_refuses_to_lose() {
    let video = TestVideo::two_scenes_with_chapters(
        "mp4_chapters",
        ChapterContainer::Mp4,
        &three_chapters(),
    );
    let detection = scan(&video, ScanMode::EveryFrame, ScanDepth::BlackScreens);
    assert_the_three_chapters_were_read(&detection);

    // The scenes are written, but they do not carry the chapter stream, so
    // the original is not one the check lets go.
    let order = SplitOrder::new(
        ScenesToWrite::every_scene(detected_cuts(&detection)),
        copying(OriginalPolicy::Keep),
    );
    let report = export::export_scenes(
        &video.path,
        &order,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();
    assert_eq!(report.scene_files.len(), 2);
    let Verification::Failed(reason) = &report.verification else {
        panic!("the chapter stream of the original was let go unnoticed");
    };
    assert!(reason.contains("data stream"), "{reason}");
    assert!(video.path.is_file());
}

/// Scans the video and returns every progress report in the order given.
fn progress_of_a_scan(video: &TestVideo, depth: ScanDepth) -> Vec<ScanProgress> {
    let reports = Mutex::new(Vec::new());
    detect::scan_for_scene_changes(
        &video.path,
        &scan_settings(ScanMode::EveryFrame),
        depth,
        &|progress| reports.lock().unwrap().push(progress),
    )
    .unwrap();
    reports.into_inner().unwrap()
}

fn largest_fraction(reports: &[ScanProgress], stage: ScanStage) -> f32 {
    reports
        .iter()
        .filter(|progress| progress.stage == stage)
        .map(|progress| progress.fraction)
        .fold(0.0, f32::max)
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_scan_for_black_screens_reports_progress_up_to_the_end() {
    let video = TestVideo::two_scenes_with_a_black_screen("progress");
    let reports = progress_of_a_scan(&video, ScanDepth::BlackScreens);

    assert!(reports.len() > 10, "{} reports", reports.len());
    assert!(reports
        .iter()
        .all(|progress| progress.stage == ScanStage::BlackScreens
            && (0.0..=1.0).contains(&progress.fraction)));
    assert_eq!(largest_fraction(&reports, ScanStage::BlackScreens), 1.0);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_hard_cut_scan_reports_progress_through_both_stages() {
    let video = TestVideo::two_scenes_with_a_hard_cut("progress_two_stages");
    let reports = progress_of_a_scan(&video, ScanDepth::WithHardCuts);

    // Both stages decode every frame here, so each is half of the scan.
    assert_eq!(largest_fraction(&reports, ScanStage::BlackScreens), 0.5);
    assert_eq!(largest_fraction(&reports, ScanStage::HardCuts), 1.0);
    let first_of_the_second_stage = reports
        .iter()
        .find(|progress| progress.stage == ScanStage::HardCuts)
        .unwrap();
    assert!(first_of_the_second_stage.fraction > 0.5);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_healthy_video_has_no_stretches_that_could_not_be_decoded() {
    let video = TestVideo::two_scenes_with_a_hard_cut("nothing_undecoded");
    let outcome = detect::scan_for_scene_changes(
        &video.path,
        &scan_settings(ScanMode::EveryFrame),
        ScanDepth::WithHardCuts,
        &detect::no_scan_progress,
    )
    .unwrap();
    assert!(outcome.undecoded.is_empty(), "{:?}", outcome.undecoded);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_file_that_is_not_a_video_fails_with_what_ffprobe_said() {
    let video = TestVideo::two_scenes_with_a_black_screen("not_a_video");
    let not_a_video = video.folder().join("notes.mp4");
    std::fs::write(&not_a_video, b"these are not pictures").unwrap();

    let error = probe::read_packet_index(&not_a_video).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<FfmpegError>(),
            Some(FfmpegError::Failed { stderr_tail, .. }) if !stderr_tail.is_empty()
        ),
        "{error:#}"
    );
    assert!(format!("{error:#}").contains("notes.mp4"), "{error:#}");
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_picture_past_the_end_of_the_video_is_not_understood() {
    let video = TestVideo::two_scenes_with_a_black_screen("past_the_end");
    let past_the_end = 10.0 * (2.0 * SCENE_SECONDS + BLACK_SECONDS);

    let Err(error) =
        thumbnails::grab_thumbnail(&video.path, past_the_end, thumbnails::SMALLEST_THUMBNAIL)
    else {
        panic!("a picture was returned from past the end of the video");
    };
    assert!(
        matches!(
            error.downcast_ref::<FfmpegError>(),
            Some(FfmpegError::OutputNotUnderstood { .. })
        ),
        "{error:#}"
    );
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn splitting_writes_one_file_per_scene_and_they_pass_verification() {
    let video = TestVideo::two_scenes_with_a_black_screen("split");
    let detection = scan(&video, ScanMode::EveryFrame, ScanDepth::BlackScreens);

    let order = SplitOrder::new(
        ScenesToWrite::every_scene(detected_cuts(&detection)),
        copying(OriginalPolicy::Keep),
    );
    let report = export::export_scenes(
        &video.path,
        &order,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();

    assert_eq!(report.scene_files.len(), 2);
    assert!(
        matches!(report.verification, Verification::Passed),
        "{:?}",
        report.verification
    );
    assert!(matches!(report.original, OriginalOutcome::Kept));
    assert!(video.path.is_file());
    assert!(report.is_clean());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn each_scene_file_holds_its_own_part_of_the_video() {
    let video = TestVideo::two_scenes_with_a_black_screen("scene_lengths");
    let scene_files = split_at_the_black_screen(&video);

    let lengths: Vec<f64> = scene_files
        .iter()
        .map(|scene| probe::read_packet_index(scene).unwrap().video.seconds())
        .collect();
    assert_close(lengths[0], SCENE_SECONDS);
    assert_close(lengths[1], BLACK_SECONDS + SCENE_SECONDS);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_check_says_how_far_it_is_and_ends_near_the_whole() {
    let video = TestVideo::two_scenes_with_a_black_screen("check_progress");
    let scene_files = split_at_the_black_screen(&video);

    let shares = std::sync::Mutex::new(Vec::new());
    verify::verify_scenes(&video.path, &scene_files, &|share| {
        shares.lock().unwrap().push(share)
    })
    .unwrap();

    let shares = shares.into_inner().unwrap();
    let furthest = shares.iter().copied().fold(0.0, f32::max);
    assert!(shares.iter().all(|share| (0.0..=1.0).contains(share)));
    // The packets are nearly all of a scene file.
    assert!(furthest > 0.8, "the check only got to {furthest}");
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_split_says_how_far_it_is_through_writing_and_checking() {
    let video = TestVideo::two_scenes_with_a_black_screen("split_progress");
    let detection = scan(&video, ScanMode::EveryFrame, ScanDepth::BlackScreens);
    let order = SplitOrder::new(
        ScenesToWrite::every_scene(detected_cuts(&detection)),
        copying(OriginalPolicy::Keep),
    );

    let reports = std::sync::Mutex::new(Vec::new());
    export::export_scenes_reporting(
        &video.path,
        &order,
        video.folder(),
        SceneNamePattern::default(),
        &|progress| reports.lock().unwrap().push(progress),
    )
    .unwrap();

    let reports = reports.into_inner().unwrap();
    let stages: Vec<SplitStage> = reports.iter().map(|progress| progress.stage).collect();
    assert!(stages.contains(&SplitStage::WritingScenes), "{stages:?}");
    assert!(stages.contains(&SplitStage::CheckingScenes), "{stages:?}");
    let first_check = stages
        .iter()
        .position(|stage| *stage == SplitStage::CheckingScenes)
        .unwrap();
    assert!(stages[first_check..]
        .iter()
        .all(|stage| *stage == SplitStage::CheckingScenes));
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn verification_fails_when_a_scene_is_cut_short() {
    let video = TestVideo::two_scenes_with_a_black_screen("cut_short");
    let scene_files = split_at_the_black_screen(&video);

    let last_scene = &scene_files[1];
    let bytes = std::fs::read(last_scene).unwrap();
    std::fs::write(last_scene, &bytes[..bytes.len() / 2]).unwrap();

    assert!(verify::verify_scenes(&video.path, &scene_files, &verify::no_share_checked).is_err());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn splitting_again_into_fewer_scenes_leaves_no_old_files_behind() {
    let video = TestVideo::two_scenes_with_a_black_screen("resplit");
    split_at_the_black_screen(&video);

    // One second into the first scene, then again with a single cut later:
    // the first split writes three files, the second only two.
    split::split_at_keyframes(&video.path, &cuts_at(&[1.0, BLACK_START]), video.folder()).unwrap();
    let scene_files =
        split::split_at_keyframes(&video.path, &cuts_at(&[BLACK_START]), video.folder()).unwrap();

    let found = split::scene_files_from_earlier_splits(
        &video.path,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();
    assert_eq!(found, scene_files);
    assert!(verify::verify_scenes(&video.path, &scene_files, &verify::no_share_checked).is_ok());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_with_a_percent_sign_in_its_name_is_split() {
    let video = TestVideo::two_scenes_with_a_black_screen("percent_sign");
    // ffmpeg reads the name it is given for the scene files as a pattern,
    // in which a percent sign means something.
    let named_with_percent = video.folder().join("100% fun.mp4");
    std::fs::copy(&video.path, &named_with_percent).unwrap();

    let scene_files = split::split_at_keyframes(
        &named_with_percent,
        &cuts_at(&[BLACK_START]),
        video.folder(),
    )
    .unwrap();
    assert_eq!(
        scene_files,
        vec![
            video.folder().join("100% fun_scene_1_of_2.mp4"),
            video.folder().join("100% fun_scene_2_of_2.mp4"),
        ]
    );
    assert!(
        verify::verify_scenes(&named_with_percent, &scene_files, &verify::no_share_checked).is_ok()
    );
    assert_no_folder_is_left_in(video.folder());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_split_that_fails_leaves_the_scenes_of_an_earlier_split_as_they_were() {
    let video = TestVideo::two_scenes_with_a_black_screen("failed_split");
    let earlier_scenes = split_at_the_black_screen(&video);
    let earlier_bytes: Vec<Vec<u8>> = earlier_scenes
        .iter()
        .map(|scene| std::fs::read(scene).unwrap())
        .collect();

    // The video is replaced by something ffmpeg cannot read, so the next
    // split of it fails.
    std::fs::write(&video.path, b"this is not a video").unwrap();
    let error = split::split_at_keyframes(&video.path, &cuts_at(&[1.0, 2.0, 3.0]), video.folder())
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("could not split"),
        "{error:#}"
    );

    let scenes_now = split::scene_files_from_earlier_splits(
        &video.path,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();
    assert_eq!(scenes_now, earlier_scenes);
    for (scene, bytes) in earlier_scenes.iter().zip(&earlier_bytes) {
        assert_eq!(&std::fs::read(scene).unwrap(), bytes, "{}", scene.display());
    }
    // Nothing half-written stays behind, in a folder or otherwise.
    assert_no_folder_is_left_in(video.folder());
}

/// Cuts at keyframes of a video, and what a split at them has to give.
struct KeyframeCuts {
    cuts: CutTimes,
    /// How many pictures each scene holds, in scene order.
    pictures_per_scene: Vec<usize>,
}

/// Cuts at the keyframes of `video` with the given numbers, counted from
/// zero, at the times the scan reads for them.
fn cuts_at_keyframes(video: &TestVideo, keyframe_numbers: &[usize]) -> KeyframeCuts {
    let index = probe::read_packet_index(&video.path).unwrap();
    let keyframe_positions: Vec<usize> = index
        .packets
        .iter()
        .enumerate()
        .filter(|(_position, packet)| packet.is_keyframe)
        .map(|(position, _packet)| position)
        .collect();
    let cut_positions: Vec<usize> = keyframe_numbers
        .iter()
        .map(|&number| keyframe_positions[number])
        .collect();

    let cut_times = cut_positions
        .iter()
        .map(|&position| index.packets[position].time)
        .collect();
    let scene_starts = std::iter::once(0).chain(cut_positions.iter().copied());
    let scene_ends = cut_positions
        .iter()
        .copied()
        .chain(std::iter::once(index.packets.len()));
    KeyframeCuts {
        cuts: CutTimes::inside(index.video, cut_times).unwrap(),
        pictures_per_scene: scene_starts
            .zip(scene_ends)
            .map(|(start, end)| end - start)
            .collect(),
    }
}

fn pictures_in_each(scene_files: &[PathBuf]) -> Vec<usize> {
    scene_files
        .iter()
        .map(|scene| probe::read_packet_index(scene).unwrap().packets.len())
        .collect()
}

/// Makes sure the AVI test video is of the kind that came out as one file:
/// ffprobe lists its keyframes with "N/A" where the show time goes. An
/// ffmpeg that one day fills those times in would let the tests below pass
/// without testing anything.
fn assert_keyframes_state_no_show_time(video: &TestVideo) {
    let packets = tools::command(Program::Ffprobe)
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "packet=pts_time,flags", "-of", "csv=p=0"])
        .arg(&video.path)
        .run_for_text(STEADY_OUTPUT_LIMIT)
        .unwrap();
    let keyframes: Vec<&str> = packets
        .lines()
        .filter(|line| {
            line.split(',')
                .nth(1)
                .is_some_and(|flags| flags.starts_with('K'))
        })
        .collect();
    assert!(keyframes.len() > 6, "{keyframes:?}");
    assert!(
        keyframes.iter().all(|line| line.starts_with("N/A,")),
        "{keyframes:?}"
    );
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn an_avi_that_states_no_show_time_for_its_keyframes_is_split_at_every_cut() {
    let video = TestVideo::avi_without_show_times("avi");
    assert_keyframes_state_no_show_time(&video);
    let planned = cuts_at_keyframes(&video, &[3, 6]);

    let order = SplitOrder::new(
        ScenesToWrite::every_scene(planned.cuts),
        copying(OriginalPolicy::Keep),
    );
    let report = export::export_scenes(
        &video.path,
        &order,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();

    // Every scene starts on the keyframe its cut was planned on.
    assert_eq!(
        pictures_in_each(&report.scene_files),
        planned.pictures_per_scene
    );
    assert!(
        matches!(report.verification, Verification::Passed),
        "{:?}",
        report.verification
    );
    assert!(video.path.is_file());
    assert!(report.is_clean());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn an_avi_is_split_at_every_cut_by_an_ffmpeg_without_the_time_filter_too() {
    let video = TestVideo::avi_without_show_times("avi_old_ffmpeg");
    let planned = cuts_at_keyframes(&video, &[3, 6]);

    let scene_files = split::split_at_keyframes_timing_pictures(
        &video.path,
        &planned.cuts,
        video.folder(),
        ShowTimeOption::WorkedOutByFfmpeg,
    )
    .unwrap();

    assert_eq!(pictures_in_each(&scene_files), planned.pictures_per_scene);
    verify::verify_scenes(&video.path, &scene_files, &verify::no_share_checked).unwrap();
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_scene_cut_out_of_an_avi_is_split_at_every_cut_again() {
    let video = TestVideo::avi_without_show_times("avi_scene");
    let first_split = cuts_at_keyframes(&video, &[3]);
    let scene_files =
        split::split_at_keyframes(&video.path, &first_split.cuts, video.folder()).unwrap();

    // The second scene opens with a keyframe and the pictures stored after
    // it that are shown before it, which the whole video does not.
    let scene = TestVideo::copy_of(&scene_files[1], "avi_scene_again");
    let planned = cuts_at_keyframes(&scene, &[1, 4]);
    let scenes_of_the_scene =
        split::split_at_keyframes(&scene.path, &planned.cuts, scene.folder()).unwrap();

    assert_eq!(
        pictures_in_each(&scenes_of_the_scene),
        planned.pictures_per_scene
    );
    verify::verify_scenes(&scene.path, &scenes_of_the_scene, &verify::no_share_checked).unwrap();
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_cut_ffmpeg_passes_over_stops_the_split_and_says_how_many_scenes_were_written() {
    let video = TestVideo::two_scenes_with_a_black_screen("cut_passed_over");
    // The last keyframe is a second before the end, so no keyframe is left
    // for a cut half a second before it.
    let after_the_last_keyframe = 2.0 * SCENE_SECONDS + BLACK_SECONDS - 0.5;

    let error = split::split_at_keyframes(
        &video.path,
        &cuts_at(&[BLACK_START, after_the_last_keyframe]),
        video.folder(),
    )
    .unwrap_err();

    assert!(error.downcast_ref::<ScenesNotCut>().is_some(), "{error:#}");
    assert!(
        format!("{error:#}").contains("ffmpeg wrote 2 of 3 scenes"),
        "{error:#}"
    );
    // The two scenes that were written are gone with their folder.
    assert_no_folder_is_left_in(video.folder());
    let files_left = std::fs::read_dir(video.folder()).unwrap().count();
    assert_eq!(files_left, 1);
}

/// The test video cut into its first scene, its black screen and its second
/// scene, with the black screen dropped.
fn without_the_black_screen() -> ScenesToWrite {
    let fates = vec![SceneFate::Keep, SceneFate::Drop, SceneFate::Keep];
    ScenesToWrite::new(cuts_at(&[BLACK_START, BLACK_END]), fates).unwrap()
}

/// Splits the video without its black screen, and returns the scene files
/// that were kept.
fn split_without_the_black_screen(video: &TestVideo) -> Vec<PathBuf> {
    let nothing_to_note = |_dropped_scene: &Path| Ok(());
    let outcomes = split::split_scenes(
        &video.path,
        &without_the_black_screen(),
        video.folder(),
        SceneNamePattern::default(),
        nothing_to_note,
        &split::no_share_written,
    )
    .unwrap();
    split::written_files(&outcomes)
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_split_with_a_dropped_scene_writes_the_others_and_always_keeps_the_original() {
    let video = TestVideo::two_scenes_with_a_black_screen("dropped_scene");
    // The person chose to have originals moved to the Recycle Bin or Trash.
    let order = SplitOrder::new(
        without_the_black_screen(),
        copying(OriginalPolicy::RecycleOnceVerified),
    );

    let report = export::export_scenes(
        &video.path,
        &order,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();

    // The kept scenes are numbered without a gap where the dropped one was.
    assert_eq!(
        report.scene_files,
        vec![
            video.folder().join("video_scene_1_of_2.mp4"),
            video.folder().join("video_scene_2_of_2.mp4"),
        ]
    );
    assert_eq!(report.dropped_scenes, 1);
    assert!(
        matches!(report.verification, Verification::Passed),
        "{:?}",
        report.verification
    );
    assert!(matches!(
        report.original,
        OriginalOutcome::AlwaysKept(OriginalAlwaysKept::ScenesDropped)
    ));
    assert!(video.path.is_file());
    assert!(report.is_clean());

    // Neither file holds the black screen.
    for scene in &report.scene_files {
        let length = probe::read_packet_index(scene).unwrap().video.seconds();
        assert_close(length, SCENE_SECONDS);
    }
    // The dropped scene is gone with the folder it was written into.
    assert_no_folder_is_left_in(video.folder());
    let files_left = std::fs::read_dir(video.folder()).unwrap().count();
    assert_eq!(files_left, 3);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_kept_scenes_of_a_split_with_a_dropped_scene_pass_the_check() {
    let video = TestVideo::two_scenes_with_a_black_screen("dropped_verify");
    let kept_files = split_without_the_black_screen(&video);

    assert_eq!(kept_files.len(), 2);
    verify::verify_scenes(&video.path, &kept_files, &verify::no_share_checked).unwrap();
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_check_fails_when_a_kept_scene_of_a_split_with_dropped_scenes_is_cut_short() {
    let video = TestVideo::two_scenes_with_a_black_screen("dropped_cut_short");
    let kept_files = split_without_the_black_screen(&video);

    let last_scene = &kept_files[1];
    let bytes = std::fs::read(last_scene).unwrap();
    std::fs::write(last_scene, &bytes[..bytes.len() / 2]).unwrap();

    assert!(verify::verify_scenes(&video.path, &kept_files, &verify::no_share_checked).is_err());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn scene_files_take_the_name_chosen_and_are_found_again_under_it() {
    let video = TestVideo::two_scenes_with_a_black_screen("scene_name");
    let scene_name = SceneNamePattern::new("{number} of {count} {name} from {start}").unwrap();
    let order = SplitOrder::new(
        ScenesToWrite::every_scene(cuts_at(&[BLACK_START])),
        copying(OriginalPolicy::Keep),
    );

    let report = export::export_scenes(&video.path, &order, video.folder(), scene_name).unwrap();
    assert_eq!(
        report.scene_files,
        vec![
            video.folder().join("1 of 2 video from 00h00m00s.mp4"),
            video.folder().join("2 of 2 video from 00h00m04s.mp4"),
        ]
    );
    assert!(report.is_clean(), "{:?}", report.verification);

    let found =
        split::scene_files_from_earlier_splits(&video.path, video.folder(), scene_name).unwrap();
    assert_eq!(found, report.scene_files);
    // Someone who never used that name does not take them for scenes.
    let found_by_default_name = split::scene_files_from_earlier_splits(
        &video.path,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();
    assert!(found_by_default_name.is_empty());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_split_under_a_new_name_removes_the_scenes_of_the_split_under_the_old_name() {
    let video = TestVideo::two_scenes_with_a_black_screen("renamed_scenes");
    let old_default = SceneNamePattern::default_before_the_count();
    let every_scene = || {
        SplitOrder::new(
            ScenesToWrite::every_scene(cuts_at(&[BLACK_START])),
            copying(OriginalPolicy::Keep),
        )
    };
    // A split from before the default name said the count.
    let earlier =
        export::export_scenes(&video.path, &every_scene(), video.folder(), old_default).unwrap();
    assert_eq!(
        earlier.scene_files,
        vec![
            video.folder().join("video_scene_001.mp4"),
            video.folder().join("video_scene_002.mp4"),
        ]
    );
    // Other videos in the folder, whose names only look like scene names.
    let neighbours = [
        video.folder().join("video_scene_1_of_20_extended.mp4"),
        video.folder().join("video_scene_2.mp4"),
        video.folder().join("video_scene_3_of_2.mp4"),
        video.folder().join("video two_scene_001.mp4"),
    ];
    for neighbour in &neighbours {
        std::fs::copy(&video.path, neighbour).unwrap();
    }

    // The earlier split is found, and said to be named another way.
    let found =
        split::earlier_splits_in(&video.path, video.folder(), SceneNamePattern::default()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].naming(), EarlierNaming::AnotherWay);
    assert_eq!(found[0].files(), earlier.scene_files);

    let report = export::export_scenes(
        &video.path,
        &every_scene(),
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();
    assert_eq!(
        report.scene_files,
        vec![
            video.folder().join("video_scene_1_of_2.mp4"),
            video.folder().join("video_scene_2_of_2.mp4"),
        ]
    );
    assert!(report.is_clean(), "{:?}", report.verification);

    // The scenes of the earlier split are gone, and nothing else is.
    assert!(earlier.scene_files.iter().all(|scene| !scene.exists()));
    assert!(neighbours.iter().all(|neighbour| neighbour.is_file()));
    assert!(video.path.is_file());
    assert_no_folder_is_left_in(video.folder());
}

/// The folder scenes are written into before they get their names is gone
/// once a split is over, however it ended.
fn assert_no_folder_is_left_in(folder: &std::path::Path) {
    let folders_left: Vec<PathBuf> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    assert!(folders_left.is_empty(), "{folders_left:?}");
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_preview_picture_can_be_grabbed() {
    let video = TestVideo::two_scenes_with_a_black_screen("preview");
    let thumbnail =
        thumbnails::grab_thumbnail(&video.path, 2.0, thumbnails::SMALLEST_THUMBNAIL).unwrap();

    assert_eq!(
        thumbnail.rgb.len(),
        THUMBNAIL_WIDTH * THUMBNAIL_HEIGHT * BYTES_PER_RGB_PIXEL
    );
    // A test pattern is not a blank picture.
    assert!(thumbnail.rgb.iter().any(|&value| value > 64));
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_preview_inside_the_black_screen_is_dark() {
    let video = TestVideo::two_scenes_with_a_black_screen("dark_preview");
    let thumbnail = thumbnails::grab_thumbnail(
        &video.path,
        BLACK_START + BLACK_SECONDS / 2.0,
        thumbnails::SMALLEST_THUMBNAIL,
    )
    .unwrap();

    assert!(thumbnail.rgb.iter().all(|&value| value < 40));
}
