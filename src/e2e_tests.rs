//! End-to-end tests: generate a real video, then scan, split and verify it
//! with a real ffmpeg.
//!
//! They are ignored by default because they need `ffmpeg` and `ffprobe` on
//! the PATH. Run them with `cargo test --release -- --include-ignored`.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::black::BlackCriteria;
use crate::detect::{self, Detection, ScanDepth, ScanMode, ScanProgress, ScanSettings, ScanStage};
use crate::export::{self, OriginalOutcome, OriginalPolicy, SplitOrder, Verification};
use crate::fates::{SceneFate, ScenesToWrite};
use crate::ffmpeg::FfmpegError;
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::probe;
use crate::scene_name::SceneNamePattern;
use crate::scenes::CutTimes;
use crate::split::{self, SceneOutcome};
use crate::test_videos::{TestVideo, BLACK_SECONDS, FRAME_RATE, FRAME_SECONDS, SCENE_SECONDS};
use crate::thumbnails::{self, THUMBNAIL_HEIGHT, THUMBNAIL_WIDTH};
use crate::timecode::TimeSpan;
use crate::verify::{self, SceneToVerify};

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
fn the_fast_scan_finds_the_same_black_screen_while_decoding_far_less() {
    let video = TestVideo::two_scenes_with_a_black_screen("fast_scan");
    let detection = scan(&video, ScanMode::Suspicious, ScanDepth::BlackScreens);

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
        OriginalPolicy::Keep,
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
fn verification_fails_when_a_scene_is_missing() {
    let video = TestVideo::two_scenes_with_a_black_screen("missing_scene");
    let scene_files = split_at_the_black_screen(&video);

    let error = verify::verify_scenes(&video.path, &scene_files[..1]).unwrap_err();
    assert!(format!("{error:#}").contains("packets"), "{error:#}");
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn verification_fails_when_the_scenes_are_out_of_order() {
    let video = TestVideo::two_scenes_with_a_black_screen("wrong_order");
    let mut scene_files = split_at_the_black_screen(&video);
    scene_files.reverse();

    assert!(verify::verify_scenes(&video.path, &scene_files).is_err());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn verification_fails_when_a_scene_is_cut_short() {
    let video = TestVideo::two_scenes_with_a_black_screen("cut_short");
    let scene_files = split_at_the_black_screen(&video);

    let last_scene = &scene_files[1];
    let bytes = std::fs::read(last_scene).unwrap();
    std::fs::write(last_scene, &bytes[..bytes.len() / 2]).unwrap();

    assert!(verify::verify_scenes(&video.path, &scene_files).is_err());
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
    assert!(verify::verify_scenes(&video.path, &scene_files).is_ok());
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
            video.folder().join("100% fun_scene_001.mp4"),
            video.folder().join("100% fun_scene_002.mp4"),
        ]
    );
    assert!(verify::verify_scenes(&named_with_percent, &scene_files).is_ok());
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

/// The test video cut into its first scene, its black screen and its second
/// scene, with the black screen dropped.
fn without_the_black_screen() -> ScenesToWrite {
    let fates = vec![SceneFate::Keep, SceneFate::Drop, SceneFate::Keep];
    ScenesToWrite::new(cuts_at(&[BLACK_START, BLACK_END]), fates).unwrap()
}

/// Splits the video without its black screen, and returns every scene the
/// way the check takes it.
fn split_without_the_black_screen(video: &TestVideo) -> Vec<SceneToVerify> {
    split::split_scenes(
        &video.path,
        &without_the_black_screen(),
        video.folder(),
        SceneNamePattern::default(),
        verify::record_dropped_scene,
    )
    .unwrap()
    .into_iter()
    .map(|outcome| match outcome {
        SceneOutcome::Written(scene_file) => SceneToVerify::InFile(scene_file),
        SceneOutcome::Dropped(packets) => SceneToVerify::Dropped(packets),
    })
    .collect()
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_split_with_a_dropped_scene_writes_the_others_and_always_keeps_the_original() {
    let video = TestVideo::two_scenes_with_a_black_screen("dropped_scene");
    // The person chose to have originals moved to the Recycle Bin or Trash.
    let order = SplitOrder::new(
        without_the_black_screen(),
        OriginalPolicy::RecycleOnceVerified,
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
            video.folder().join("video_scene_001.mp4"),
            video.folder().join("video_scene_002.mp4"),
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
        OriginalOutcome::KeptBecauseScenesDropped
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
fn kept_scenes_pass_the_check_only_as_the_right_parts_in_the_right_order() {
    let video = TestVideo::two_scenes_with_a_black_screen("dropped_verify");
    let mut scenes = split_without_the_black_screen(&video);
    verify::verify_scenes_with_dropped(&video.path, &scenes).unwrap();

    // The two scene files change places around the dropped scene.
    scenes.swap(0, 2);
    assert!(verify::verify_scenes_with_dropped(&video.path, &scenes).is_err());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_check_fails_when_a_kept_scene_of_a_split_with_dropped_scenes_is_cut_short() {
    let video = TestVideo::two_scenes_with_a_black_screen("dropped_cut_short");
    let scenes = split_without_the_black_screen(&video);

    let SceneToVerify::InFile(last_scene) = &scenes[2] else {
        panic!("the last scene was kept, so it is in a file");
    };
    let bytes = std::fs::read(last_scene).unwrap();
    std::fs::write(last_scene, &bytes[..bytes.len() / 2]).unwrap();

    assert!(verify::verify_scenes_with_dropped(&video.path, &scenes).is_err());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_kept_scenes_alone_do_not_pass_for_the_whole_video() {
    let video = TestVideo::two_scenes_with_a_black_screen("dropped_not_whole");
    let kept_files: Vec<PathBuf> = split_without_the_black_screen(&video)
        .into_iter()
        .filter_map(|scene| match scene {
            SceneToVerify::InFile(scene_file) => Some(scene_file),
            SceneToVerify::Dropped(_) => None,
        })
        .collect();

    // The check that guards an original before it is recycled.
    let error = verify::verify_scenes(&video.path, &kept_files).unwrap_err();
    assert!(format!("{error:#}").contains("packets"), "{error:#}");
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn scene_files_take_the_name_chosen_and_are_found_again_under_it() {
    let video = TestVideo::two_scenes_with_a_black_screen("scene_name");
    let scene_name = SceneNamePattern::new("{number} of {count} {name} from {start}").unwrap();
    let order = SplitOrder::new(
        ScenesToWrite::every_scene(cuts_at(&[BLACK_START])),
        OriginalPolicy::Keep,
    );

    let report = export::export_scenes(&video.path, &order, video.folder(), scene_name).unwrap();
    assert_eq!(
        report.scene_files,
        vec![
            video.folder().join("001 of 2 video from 00h00m00s.mp4"),
            video.folder().join("002 of 2 video from 00h00m04s.mp4"),
        ]
    );
    assert!(report.is_clean(), "{:?}", report.verification);

    let found =
        split::scene_files_from_earlier_splits(&video.path, video.folder(), scene_name).unwrap();
    assert_eq!(found, report.scene_files);
    // Files named another way are not taken for scenes of this split.
    let found_by_default_name = split::scene_files_from_earlier_splits(
        &video.path,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();
    assert!(found_by_default_name.is_empty());
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
