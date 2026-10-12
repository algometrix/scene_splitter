//! End-to-end tests: generate a real video, then scan, split and verify it
//! with a real ffmpeg.
//!
//! They are ignored by default because they need `ffmpeg` and `ffprobe` on
//! the PATH. Run them with `cargo test --release -- --include-ignored`.

use std::path::PathBuf;

use crate::black::{BlackCriteria, TimeSpan};
use crate::detect::{self, Detection, ScanDepth, ScanMode, ScanSettings};
use crate::export::{self, OriginalOutcome, OriginalPolicy, Verification};
use crate::hardcut::{HardCutSettings, Sensitivity};
use crate::probe;
use crate::split;
use crate::test_videos::{TestVideo, BLACK_SECONDS, FRAME_RATE, FRAME_SECONDS, SCENE_SECONDS};
use crate::thumbnails::{self, THUMBNAIL_HEIGHT, THUMBNAIL_WIDTH};
use crate::verify;

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
    detect::detect_scene_changes(&video.path, &scan_settings(mode), depth).unwrap()
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
    split::split_at_keyframes(&video.path, &detection.cuts, video.folder()).unwrap()
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

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn splitting_writes_one_file_per_scene_and_they_pass_verification() {
    let video = TestVideo::two_scenes_with_a_black_screen("split");
    let detection = scan(&video, ScanMode::EveryFrame, ScanDepth::BlackScreens);

    let report = export::export_scenes(
        &video.path,
        &detection.cuts,
        video.folder(),
        OriginalPolicy::Keep,
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
    split::split_at_keyframes(&video.path, &[1.0, BLACK_START], video.folder()).unwrap();
    let scene_files =
        split::split_at_keyframes(&video.path, &[BLACK_START], video.folder()).unwrap();

    let found = split::scene_files_from_earlier_splits(&video.path, video.folder()).unwrap();
    assert_eq!(found, scene_files);
    assert!(verify::verify_scenes(&video.path, &scene_files).is_ok());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_preview_picture_can_be_grabbed() {
    let video = TestVideo::two_scenes_with_a_black_screen("preview");
    let thumbnail = thumbnails::grab_thumbnail(&video.path, 2.0).unwrap();

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
    let thumbnail =
        thumbnails::grab_thumbnail(&video.path, BLACK_START + BLACK_SECONDS / 2.0).unwrap();

    assert!(thumbnail.rgb.iter().all(|&value| value < 40));
}
