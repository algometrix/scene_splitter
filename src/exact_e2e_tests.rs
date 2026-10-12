//! End-to-end tests of the exact cut: generate a real video, split it on
//! chosen pictures with a real ffmpeg, and count what the scene files hold.
//!
//! Ignored by default, like the other end-to-end tests, because they need
//! `ffmpeg` and `ffprobe` on the PATH.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::exact_cut::CutPrecision;
use crate::exact_fallback::{CannotMoveSplits, ExactCutFallback, NoWayToCut, WhyNotExact};
use crate::exact_split::{self, Encoder, ExactCutError, Unsupported};
use crate::exact_verify::{self, ExactCutProof, NotProven, VerifyError, WrittenScenes};
use crate::export::{
    self, CutOutcome, ExportOptions, ExportReport, OriginalAlwaysKept, OriginalOutcome, SplitOrder,
    Verification,
};
use crate::fates::{SceneFate, ScenesToWrite};
use crate::ffmpeg::{Program, TimeLimit};
use crate::probe;
use crate::scene_name::SceneNamePattern;
use crate::scenes::CutTimes;
use crate::split;
use crate::test_videos::{
    TestVideo, FRAME_RATE, FRAME_SECONDS, REORDERED_KEYFRAME_EVERY, REORDERED_VIDEO_SECONDS,
    SCENE_SECONDS,
};
use crate::timecode::TimeSpan;
use crate::tools;

const REORDERED_FRAMES: usize = (REORDERED_VIDEO_SECONDS as usize) * FRAME_RATE as usize;
/// A picture between two keyframes of the reordered video, far from both.
const CUT_FRAME: usize = 73;
/// Times of a file are compared to within a millisecond, which is as fine
/// as a Matroska file keeps them.
const MILLISECOND: f64 = 0.001;
/// How much of a video the test of a damaged one overwrites, and with what.
const DAMAGED_BYTES: usize = 20_000;
const DAMAGE_FILL: u8 = 0x5a;
/// Writing a few seconds of small pictures anew takes about a second.
const REWRITE_LIMIT: TimeLimit = TimeLimit::WholeRun(Duration::from_secs(120));

fn time_of(frame: usize) -> f64 {
    frame as f64 * FRAME_SECONDS
}

fn cuts_at(video_seconds: f64, times: &[f64]) -> CutTimes {
    let whole_video = TimeSpan {
        start: 0.0,
        end: video_seconds,
    };
    CutTimes::inside(whole_video, times.to_vec()).unwrap()
}

/// The options of someone who turned exact cuts on and left everything else
/// alone, which includes moving originals to the Recycle Bin or Trash.
fn exact_cut_options() -> ExportOptions {
    ExportOptions {
        cut_precision: CutPrecision::ExactFrame,
        ..ExportOptions::default()
    }
}

fn split_exactly(video: &Path, out_dir: &Path, cuts: &CutTimes) -> ExportReport {
    let order = SplitOrder::new(
        ScenesToWrite::every_scene(cuts.clone()),
        exact_cut_options().split_method(),
    );
    export::export_scenes(video, &order, out_dir, SceneNamePattern::default()).unwrap()
}

fn proof_of(report: &ExportReport) -> &ExactCutProof {
    match &report.verification {
        Verification::PassedForExactCut(proof) => proof,
        other => panic!("not the result of an exact cut: {other:?}"),
    }
}

/// How many pictures each file holds, counted by ffprobe and not by the
/// check under test.
fn picture_counts(files: &[PathBuf]) -> Vec<usize> {
    files
        .iter()
        .map(|file| probe::read_packet_index(file).unwrap().packets.len())
        .collect()
}

/// Asserts that the pictures of the scene files, one file after another, are
/// shown at the pace of the original's pictures: each as long after the
/// first picture of its scene as in the original. A join that left a gap or
/// doubled a time would show here. Times are read by ffprobe and not by the
/// check under test.
fn assert_pictures_keep_their_pace(original: &Path, scene_files: &[PathBuf]) {
    let original_times = picture_times(original);
    let mut pictures_before = 0;
    for scene in scene_files {
        let scene_times = picture_times(scene);
        let in_original = &original_times[pictures_before..pictures_before + scene_times.len()];
        for (place, (in_scene, in_original_at)) in scene_times.iter().zip(in_original).enumerate() {
            let since_scene_start = in_scene - scene_times[0];
            let since_start_in_original = in_original_at - in_original[0];
            assert!(
                (since_scene_start - since_start_in_original).abs() <= 2.0 * MILLISECOND,
                "{}: picture {} is shown {since_scene_start} s into the scene, \
                 but {since_start_in_original} s in the original",
                scene.display(),
                place + 1
            );
        }
        pictures_before += scene_times.len();
    }
    assert_eq!(pictures_before, original_times.len());
}

fn picture_times(video: &Path) -> Vec<f64> {
    let index = probe::read_packet_index(video).unwrap();
    index.packets.iter().map(|packet| packet.time).collect()
}

/// Writes `video` anew as `file_name` in the same folder, with the given
/// ffmpeg arguments between input and output.
fn rewritten(video: &TestVideo, file_name: &str, how: &[&str]) -> PathBuf {
    let rewritten = video.folder().join(file_name);
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(&video.path)
        .args(how)
        .arg(&rewritten)
        .run(REWRITE_LIMIT)
        .unwrap();
    rewritten
}

fn folders_in(folder: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect()
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn an_exact_split_leaves_out_a_dropped_scene_and_names_the_rest_by_the_pattern() {
    let video = TestVideo::with_reordered_pictures("exact_with_a_dropped_scene");
    let original_bytes = std::fs::read(&video.path).unwrap();
    let second_cut_frame = CUT_FRAME + 2 * REORDERED_KEYFRAME_EVERY;
    let cuts = cuts_at(
        REORDERED_VIDEO_SECONDS,
        &[time_of(CUT_FRAME), time_of(second_cut_frame)],
    );
    let fates = vec![SceneFate::Keep, SceneFate::Drop, SceneFate::Keep];
    let scenes = ScenesToWrite::new(cuts, fates).unwrap();
    let order = SplitOrder::new(scenes, exact_cut_options().split_method());
    let scene_name = SceneNamePattern::new("{number} of {count} {name}").unwrap();

    let report = export::export_scenes(&video.path, &order, video.folder(), scene_name).unwrap();

    // The kept scenes are numbered without a gap where the dropped one was.
    assert_eq!(
        report.scene_files,
        vec![
            video.folder().join("1 of 2 video.mp4"),
            video.folder().join("2 of 2 video.mp4"),
        ]
    );
    assert_eq!(report.dropped_scenes, 1);
    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, REORDERED_FRAMES - second_cut_frame]
    );
    // The check counted every picture of the video before the dropped
    // scene was left out.
    assert_eq!(proof_of(&report).pictures, REORDERED_FRAMES);
    assert!(matches!(
        report.original,
        OriginalOutcome::AlwaysKept(OriginalAlwaysKept::ScenesDroppedAndCutIsExact)
    ));
    assert!(report.is_clean());
    assert!(report
        .plain_summary()
        .contains("1 scene was dropped and is in no file."));
    assert_eq!(std::fs::read(&video.path).unwrap(), original_bytes);
    // Nothing of the dropped scene stays behind.
    assert!(folders_in(video.folder()).is_empty());
    let videos_left = std::fs::read_dir(video.folder()).unwrap().count();
    assert_eq!(videos_left, 3);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_cut_between_keyframes_lands_on_the_chosen_picture() {
    let video = TestVideo::with_reordered_pictures("exact_between_keyframes");
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);

    let report = split_exactly(&video.path, video.folder(), &cuts);

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, REORDERED_FRAMES - CUT_FRAME]
    );
    let proof = proof_of(&report);
    assert_eq!(proof.pictures, REORDERED_FRAMES);
    // Only the pictures between the keyframes on either side of the cut.
    assert_eq!(proof.pictures_encoded_again, REORDERED_KEYFRAME_EVERY);
    assert!(proof.sound_packets > 0);
    assert_eq!(proof.cut_landings.len(), 1);
    assert!((proof.cut_landings[0].landed_at - time_of(CUT_FRAME)).abs() < MILLISECOND);
    assert!(proof.largest_sound_shift_seconds <= 2.0 * MILLISECOND);
    assert_pictures_keep_their_pace(&video.path, &report.scene_files);
    assert!(report.is_clean());
    assert!(folders_in(video.folder()).is_empty());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn the_original_is_kept_after_an_exact_cut_whatever_the_setting_says() {
    let video = TestVideo::with_reordered_pictures("exact_keeps_original");
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);
    let original_bytes = std::fs::read(&video.path).unwrap();
    // The setting for originals says to recycle them.
    assert_eq!(
        exact_cut_options().original_policy,
        export::OriginalPolicy::RecycleOnceVerified
    );

    let report = split_exactly(&video.path, video.folder(), &cuts);

    assert!(matches!(
        report.original,
        OriginalOutcome::AlwaysKept(OriginalAlwaysKept::CutIsExact)
    ));
    assert_eq!(std::fs::read(&video.path).unwrap(), original_bytes);
    assert!(report.plain_summary().contains("always kept"));
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn several_cuts_each_start_their_scene_on_the_first_picture_at_or_after_them() {
    let video = TestVideo::with_reordered_pictures("exact_several_cuts");
    // Between two pictures, on a picture between keyframes, on a keyframe,
    // and two cuts with no keyframe between them.
    let between_pictures = time_of(33) - FRAME_SECONDS / 2.0;
    let cut_times = [
        between_pictures,
        time_of(125),
        time_of(150),
        time_of(160),
        time_of(170),
    ];
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &cut_times);

    let report = split_exactly(&video.path, video.folder(), &cuts);

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![33, 92, 25, 10, 10, REORDERED_FRAMES - 170]
    );
    let landings = &proof_of(&report).cut_landings;
    let expected_landings = [33, 125, 150, 160, 170].map(time_of);
    for ((landing, asked_for), expected) in landings.iter().zip(cut_times).zip(expected_landings) {
        assert_eq!(landing.asked_for, asked_for);
        assert!(
            (landing.landed_at - expected).abs() < MILLISECOND,
            "{landing:?} instead of {expected}"
        );
        // Never before the moment asked for, and less than a picture after.
        let late_by = landing.landed_at - asked_for;
        assert!(
            (-MILLISECOND..FRAME_SECONDS).contains(&late_by),
            "{late_by}"
        );
    }
    assert_pictures_keep_their_pace(&video.path, &report.scene_files);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_cut_on_a_keyframe_encodes_nothing_again() {
    let video = TestVideo::with_reordered_pictures("exact_on_keyframe");
    let keyframe = 2 * REORDERED_KEYFRAME_EVERY;
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(keyframe)]);

    let report = split_exactly(&video.path, video.folder(), &cuts);

    assert_eq!(proof_of(&report).pictures_encoded_again, 0);
    assert_eq!(
        picture_counts(&report.scene_files),
        vec![keyframe, REORDERED_FRAMES - keyframe]
    );
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_matroska_video_is_cut_on_the_chosen_picture() {
    let video = TestVideo::with_reordered_pictures("exact_matroska");
    let matroska = rewritten(&video, "video.mkv", &["-map", "0", "-c", "copy"]);
    // A Matroska file may count its pictures from a few milliseconds after
    // zero, so the picture is looked up and not worked out.
    let index = probe::read_packet_index(&matroska).unwrap();
    let cuts = CutTimes::inside(index.video, vec![index.packets[CUT_FRAME].time]).unwrap();

    let report = split_exactly(&matroska, video.folder(), &cuts);

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, REORDERED_FRAMES - CUT_FRAME]
    );
    assert_eq!(
        proof_of(&report).pictures_encoded_again,
        REORDERED_KEYFRAME_EVERY
    );
    assert!(report.scene_files[0]
        .extension()
        .is_some_and(|extension| extension == "mkv"));
    assert_pictures_keep_their_pace(&matroska, &report.scene_files);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_without_sound_is_cut_on_the_chosen_picture() {
    let video = TestVideo::with_reordered_pictures("exact_no_sound");
    let silent = rewritten(&video, "silent.mp4", &["-map", "0:v", "-c", "copy"]);
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);

    let report = split_exactly(&silent, video.folder(), &cuts);

    assert_eq!(proof_of(&report).sound_packets, 0);
    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, REORDERED_FRAMES - CUT_FRAME]
    );
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_with_pictures_stored_in_order_is_cut_on_the_chosen_picture() {
    let video = TestVideo::two_scenes_with_a_hard_cut("exact_plain");
    let video_seconds = 2.0 * SCENE_SECONDS;
    let frames = (video_seconds as usize) * FRAME_RATE as usize;
    // Half a second before the change of scene, and so between keyframes.
    let cuts = cuts_at(video_seconds, &[time_of(88)]);

    let report = split_exactly(&video.path, video.folder(), &cuts);

    assert_eq!(picture_counts(&report.scene_files), vec![88, frames - 88]);
    assert_pictures_keep_their_pace(&video.path, &report.scene_files);
    // Keyframes are a second apart: 13 pictures before the cut, 12 after.
    assert_eq!(
        proof_of(&report).pictures_encoded_again,
        FRAME_RATE as usize
    );
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_whose_later_keyframes_are_no_clean_starts_is_cut_on_the_chosen_picture() {
    let video = TestVideo::with_open_picture_groups("exact_open_groups");
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);

    let report = split_exactly(&video.path, video.folder(), &cuts);

    // A copy that started on one of the later keyframes would not decode to
    // the original's pictures, and the check would have thrown it away.
    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, REORDERED_FRAMES - CUT_FRAME]
    );
    assert_eq!(proof_of(&report).pictures, REORDERED_FRAMES);
    assert_pictures_keep_their_pace(&video.path, &report.scene_files);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_stored_turned_on_its_side_stays_turned_the_same_way() {
    let video = TestVideo::with_reordered_pictures("exact_turned");
    let turned = turned_on_its_side(&video);
    assert!(
        size_and_turn(&turned)
            .iter()
            .any(|property| property.contains("rotat")),
        "the test video is not marked as turned: {:?}",
        size_and_turn(&turned)
    );
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);

    let report = split_exactly(&turned, video.folder(), &cuts);

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, REORDERED_FRAMES - CUT_FRAME]
    );
    for scene in &report.scene_files {
        assert_eq!(
            size_and_turn(scene),
            size_and_turn(&turned),
            "{}",
            scene.display()
        );
    }
}

/// The video with a mark that says to show its pictures turned by a quarter,
/// as a phone held upright stores what it films. Newer versions of ffmpeg
/// set the mark as they read the file. Older ones do not know that option
/// and store the turn as a tag instead, which newer ones ignore.
fn turned_on_its_side(video: &TestVideo) -> PathBuf {
    let turned = video.folder().join("turned.mp4");
    let the_newer_way = tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-y"])
        .args(["-display_rotation", "90", "-i"])
        .arg(&video.path)
        .args(["-map", "0", "-c", "copy"])
        .arg(&turned)
        .run(REWRITE_LIMIT);
    match the_newer_way {
        Ok(_) => turned,
        Err(_an_older_ffmpeg) => rewritten(
            video,
            "turned.mp4",
            &["-map", "0", "-c", "copy", "-metadata:s:v:0", "rotate=90"],
        ),
    }
}

/// What ffprobe says about the size of the stored pictures and how they are
/// to be turned for showing.
fn size_and_turn(video: &Path) -> Vec<String> {
    let streams = tools::command(Program::Ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_streams"])
        .arg(video)
        .run_for_text(REWRITE_LIMIT)
        .unwrap();
    streams
        .lines()
        .filter(|line| {
            ["width=", "height=", "rotate", "rotation"]
                .iter()
                .any(|property| line.contains(property))
        })
        .map(str::to_owned)
        .collect()
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_that_is_not_h264_is_refused_and_nothing_is_touched() {
    let video = TestVideo::with_reordered_pictures("exact_refused");
    let other_codec = rewritten(
        &video,
        "other.mp4",
        &["-map", "0", "-c:v", "mpeg4", "-c:a", "copy"],
    );
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);
    // Scene files of an earlier, normal split are in the folder.
    let earlier_scenes = split::split_at_keyframes(&other_codec, &cuts, video.folder()).unwrap();
    let earlier_bytes: Vec<Vec<u8>> = earlier_scenes
        .iter()
        .map(|scene| std::fs::read(scene).unwrap())
        .collect();

    let error = exact_split::split_on_exact_frames(
        &other_codec,
        &ScenesToWrite::every_scene(cuts),
        video.folder(),
        SceneNamePattern::default(),
        Encoder::LIBX264,
    )
    .unwrap_err();

    let refusal = error.downcast_ref::<ExactCutError>();
    assert!(
        matches!(
            refusal,
            Some(ExactCutError::Unsupported(Unsupported::Codec { name })) if name == "mpeg4"
        ),
        "{error:#}"
    );
    let words = refusal.unwrap().in_plain_words();
    assert!(words.contains("The original was not changed."), "{words}");
    assert!(other_codec.is_file());
    for (scene, bytes) in earlier_scenes.iter().zip(&earlier_bytes) {
        assert_eq!(&std::fs::read(scene).unwrap(), bytes, "{}", scene.display());
    }
    assert!(folders_in(video.folder()).is_empty());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn an_exact_split_of_a_damaged_video_leaves_no_scene_files() {
    let video = TestVideo::with_reordered_pictures("exact_damaged");
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);
    // A stretch in the last third of the file is overwritten. The file's
    // index sits at its end and still lists every picture, so the damage
    // only shows when the pictures stored there are decoded.
    let damaged = video.folder().join("damaged.mp4");
    let mut bytes = std::fs::read(&video.path).unwrap();
    let damage_start = bytes.len() * 2 / 3;
    bytes[damage_start..damage_start + DAMAGED_BYTES].fill(DAMAGE_FILL);
    std::fs::write(&damaged, bytes).unwrap();

    let error = exact_split::split_on_exact_frames(
        &damaged,
        &ScenesToWrite::every_scene(cuts),
        video.folder(),
        SceneNamePattern::default(),
        Encoder::LIBX264,
    )
    .unwrap_err();

    // The pictures in the damaged stretch are copied, and copying does not
    // look inside them. It is the check that finds them.
    assert!(
        matches!(
            error.downcast_ref::<ExactCutError>(),
            Some(ExactCutError::NotProven(_))
        ),
        "{error:#}"
    );
    let scenes_left = split::scene_files_from_earlier_splits(
        &damaged,
        video.folder(),
        SceneNamePattern::default(),
    )
    .unwrap();
    assert!(scenes_left.is_empty(), "{scenes_left:?}");
    assert!(folders_in(video.folder()).is_empty());
}

/// The picture nearest to `CUT_FRAME` where the reordered video can be cut
/// by copying: the keyframe two seconds in.
const NEAREST_CUT_POINT_FRAME: usize = REORDERED_KEYFRAME_EVERY;

/// The reordered video with its pictures stored as MPEG-4 part 2, which no
/// exact cut is made for, and a point where it can be cut every two seconds
/// as before.
fn in_another_codec(video: &TestVideo) -> PathBuf {
    let keyframe_every = REORDERED_KEYFRAME_EVERY.to_string();
    rewritten(
        video,
        "other.mp4",
        &[
            "-map",
            "0",
            "-c:v",
            "mpeg4",
            "-g",
            &keyframe_every,
            "-c:a",
            "copy",
        ],
    )
}

fn exact_order(cuts: &CutTimes) -> SplitOrder {
    SplitOrder::new(
        ScenesToWrite::every_scene(cuts.clone()),
        exact_cut_options().split_method(),
    )
}

fn fallback_of(report: &ExportReport) -> &ExactCutFallback {
    match &report.cut {
        CutOutcome::AtTheNearestPointsInstead(fallback) => fallback,
        CutOutcome::AsAskedFor => panic!("the video was cut as asked: {report:?}"),
    }
}

/// Asserts what holds after every split that asked for the exact picture
/// `CUT_FRAME` and was cut at the nearest point instead: the one split
/// moved there and is reported, the scenes are copies that hold the whole
/// video, and the original is where and what it was.
fn assert_cut_at_the_nearest_point_instead(
    report: &ExportReport,
    original: &Path,
    original_bytes: &[u8],
) {
    let fallback = fallback_of(report);
    assert_eq!(fallback.splits.len(), 1);
    let split = fallback.splits[0];
    assert_eq!(split.asked_for, time_of(CUT_FRAME));
    assert!(
        (split.cut_at - time_of(NEAREST_CUT_POINT_FRAME)).abs() < MILLISECOND,
        "{split:?}"
    );
    let frames_moved = CUT_FRAME - NEAREST_CUT_POINT_FRAME;
    assert!(
        (split.seconds_moved() - time_of(frames_moved)).abs() < MILLISECOND,
        "{split:?}"
    );
    assert_eq!(fallback.farthest_move(), Some(split));

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![
            NEAREST_CUT_POINT_FRAME,
            REORDERED_FRAMES - NEAREST_CUT_POINT_FRAME
        ]
    );
    // The check of a normal split ran: every packet of the original is in
    // the scenes, unchanged.
    assert!(
        matches!(report.verification, Verification::Passed),
        "{:?}",
        report.verification
    );
    // The setting says to recycle, the split asked for exact pictures, and
    // so the original stays.
    assert!(
        matches!(
            report.original,
            OriginalOutcome::AlwaysKept(OriginalAlwaysKept::CutIsExact)
        ),
        "{:?}",
        report.original
    );
    assert_eq!(std::fs::read(original).unwrap(), original_bytes);
    // Not what was asked for, so the person is sent to look at the result.
    assert!(!report.is_clean());
    let summary = report.plain_summary();
    assert!(
        summary.contains("It was cut at the nearest points where it can be cut instead."),
        "{summary}"
    );
    assert!(summary.contains("0.920 seconds earlier"), "{summary}");
    assert!(folders_in(report.out_dir.as_path()).is_empty());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_in_another_codec_is_cut_at_the_nearest_points_and_the_result_says_so() {
    let video = TestVideo::with_reordered_pictures("fallback_other_codec");
    let other_codec = in_another_codec(&video);
    let original_bytes = std::fs::read(&other_codec).unwrap();
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);
    let out_dir = video.folder().join("scenes");

    // What the person is told before the split.
    let known_before = exact_split::why_no_exact_cut_at_a_glance(&other_codec).unwrap();
    let not_h264 = WhyNotExact::KindOfVideo(Unsupported::Codec {
        name: "mpeg4".to_owned(),
    });
    assert_eq!(known_before, Some(not_h264.clone()));

    let report = export::export_scenes(
        &other_codec,
        &exact_order(&cuts),
        &out_dir,
        SceneNamePattern::default(),
    )
    .unwrap();

    assert_eq!(fallback_of(&report).why, not_h264);
    assert_cut_at_the_nearest_point_instead(&report, &other_codec, &original_bytes);
    assert!(report
        .plain_summary()
        .contains("because its pictures are stored as mpeg4"));
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_that_can_be_cut_exactly_has_nothing_against_it_at_a_glance() {
    let video = TestVideo::with_reordered_pictures("exact_at_a_glance");
    assert_eq!(
        exact_split::why_no_exact_cut_at_a_glance(&video.path).unwrap(),
        None
    );
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn with_an_ffmpeg_that_lacks_the_encoder_the_video_is_cut_at_the_nearest_points() {
    let video = TestVideo::with_reordered_pictures("fallback_no_encoder");
    let original_bytes = std::fs::read(&video.path).unwrap();
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);
    let out_dir = video.folder().join("scenes");

    let report = export::export_scenes_encoding_with(
        &video.path,
        &exact_order(&cuts),
        &out_dir,
        SceneNamePattern::default(),
        Encoder::ONE_NO_FFMPEG_HAS,
    )
    .unwrap();

    assert_eq!(fallback_of(&report).why, WhyNotExact::EncoderMissing);
    assert_cut_at_the_nearest_point_instead(&report, &video.path, &original_bytes);
    assert!(report
        .plain_summary()
        .contains("because this copy of ffmpeg lacks the encoder that exact cuts need"));
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn an_exact_cut_that_fails_its_check_is_thrown_away_and_the_video_cut_at_the_nearest_points() {
    let video = TestVideo::with_reordered_pictures("fallback_not_proven");
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);
    // Damaged as in the test of a damaged video: only decoding shows it.
    let damaged = video.folder().join("damaged.mp4");
    let mut bytes = std::fs::read(&video.path).unwrap();
    let damage_start = bytes.len() * 2 / 3;
    bytes[damage_start..damage_start + DAMAGED_BYTES].fill(DAMAGE_FILL);
    std::fs::write(&damaged, &bytes).unwrap();
    let out_dir = video.folder().join("scenes");

    let report = export::export_scenes(
        &damaged,
        &exact_order(&cuts),
        &out_dir,
        SceneNamePattern::default(),
    )
    .unwrap();

    // The exact cut was made, did not pass, and nothing of it is left.
    let why = &fallback_of(&report).why;
    assert!(
        matches!(why, WhyNotExact::DidNotPassTheCheck { .. }),
        "{why:?}"
    );
    assert!(report
        .plain_summary()
        .contains("did not pass the check and were thrown away"));
    assert!(report.details().is_some());
    // What is there instead are copies, which hold the damage as the
    // original does and so pass the packet for packet check.
    assert_cut_at_the_nearest_point_instead(&report, &damaged, &bytes);
    let scene_files =
        split::scene_files_from_earlier_splits(&damaged, &out_dir, SceneNamePattern::default())
            .unwrap();
    assert_eq!(scene_files, report.scene_files);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_trimmed_video_with_pictures_that_are_never_shown_is_cut_at_the_nearest_points() {
    let video = TestVideo::with_reordered_pictures("fallback_trimmed");
    // Trimmed by a second without encoding. The file still stores the
    // pictures of that second, since the ones after them are built on them,
    // and marks them as never shown.
    let trimmed = video.folder().join("trimmed.mp4");
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-ss", "1", "-i"])
        .arg(&video.path)
        .args(["-map", "0", "-c", "copy"])
        .arg(&trimmed)
        .run(REWRITE_LIMIT)
        .unwrap();
    let index = probe::read_packet_index(&trimmed).unwrap();
    let cuts = CutTimes::inside(index.video, vec![index.video.start + time_of(CUT_FRAME)]).unwrap();

    let report = export::export_scenes(
        &trimmed,
        &exact_order(&cuts),
        &video.folder().join("scenes"),
        SceneNamePattern::default(),
    )
    .unwrap();

    let why = &fallback_of(&report).why;
    assert!(
        matches!(
            why,
            WhyNotExact::KindOfVideo(Unsupported::PicturesNeverShown { count }) if *count > 0
        ),
        "{why:?}"
    );
    assert!(matches!(
        report.original,
        OriginalOutcome::AlwaysKept(OriginalAlwaysKept::CutIsExact)
    ));
    assert!(trimmed.is_file());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn splits_that_would_meet_at_one_point_leave_nothing_written() {
    let video = TestVideo::with_reordered_pictures("fallback_splits_meet");
    let other_codec = in_another_codec(&video);
    let original_bytes = std::fs::read(&other_codec).unwrap();
    // Two pictures before and two after the same point where the video can
    // be cut.
    let before = time_of(NEAREST_CUT_POINT_FRAME - 2);
    let after = time_of(NEAREST_CUT_POINT_FRAME + 2);
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[before, after]);
    let out_dir = video.folder().join("scenes");

    let error = export::export_scenes(
        &other_codec,
        &exact_order(&cuts),
        &out_dir,
        SceneNamePattern::default(),
    )
    .unwrap_err();

    let no_way = error.downcast_ref::<NoWayToCut>();
    assert!(
        matches!(
            no_way,
            Some(NoWayToCut {
                obstacle: CannotMoveSplits::SplitsMeet { first, second, .. },
                ..
            }) if *first == before && *second == after
        ),
        "{error:#}"
    );
    let words = no_way.unwrap().in_plain_words();
    assert!(
        words.contains("Nothing was written, and the original was not changed."),
        "{words}"
    );
    assert_eq!(std::fs::read(&other_codec).unwrap(), original_bytes);
    assert!(!out_dir.exists(), "{}", out_dir.display());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn cuts_right_beside_a_point_where_the_video_can_be_cut_land_on_the_chosen_pictures() {
    let video = TestVideo::with_reordered_pictures("exact_beside_keyframes");
    // One picture before a keyframe, two before the next, one after it.
    // Here a part that is encoded again begins or ends within the pictures
    // that are stored out of order around the keyframe.
    let cut_frames = [
        REORDERED_KEYFRAME_EVERY - 1,
        2 * REORDERED_KEYFRAME_EVERY - 2,
        2 * REORDERED_KEYFRAME_EVERY + 1,
    ];
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &cut_frames.map(time_of));

    let report = split_exactly(&video.path, video.folder(), &cuts);

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![49, 49, 3, REORDERED_FRAMES - 101]
    );
    // The check compared every picture that was encoded again with its own
    // in the original and with the ones beside it.
    let proof = proof_of(&report);
    assert!(proof.pictures_encoded_again > 0);
    assert!(
        proof.largest_picture_difference < 1.0,
        "{}",
        proof.largest_picture_difference
    );
    assert_pictures_keep_their_pace(&video.path, &report.scene_files);
    assert!(report.is_clean());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_scene_encoded_from_the_picture_before_its_first_is_not_proven() {
    // Every scene of this video is encoded again as a whole, so a scene of
    // one split can stand in for a scene of another.
    let video = TestVideo::with_open_picture_groups("exact_wrong_pictures");
    let one_picture_early = video.folder().join("early");
    let as_chosen = video.folder().join("chosen");
    let early_cut = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME - 1)]);
    let chosen_cut = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);
    let early = split_exactly(&video.path, &one_picture_early, &early_cut);
    let chosen = split_exactly(&video.path, &as_chosen, &chosen_cut);

    // The second scene of the early split without its last picture: as many
    // pictures as the chosen split's second scene, shown at the same pace,
    // and each of them the picture before the one that belongs there.
    let second_scene_pictures = (REORDERED_FRAMES - CUT_FRAME).to_string();
    let shifted = video.folder().join("shifted.mp4");
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(&early.scene_files[1])
        .args([
            "-map",
            "0",
            "-c",
            "copy",
            "-frames:v",
            &second_scene_pictures,
        ])
        .arg(&shifted)
        .run(REWRITE_LIMIT)
        .unwrap();
    assert_eq!(
        picture_counts(std::slice::from_ref(&shifted)),
        vec![REORDERED_FRAMES - CUT_FRAME]
    );

    let plan = exact_split::plan_of(&video.path, &chosen_cut).unwrap();
    let scratch_folder = video.folder().join("scratch");
    std::fs::create_dir_all(&scratch_folder).unwrap();
    let verify = |second_scene: &Path| {
        let files = [chosen.scene_files[0].clone(), second_scene.to_path_buf()];
        let written = WrittenScenes {
            files: &files,
            scratch_folder: &scratch_folder,
        };
        exact_verify::verify_exact_split(&video.path, written, &plan, chosen_cut.as_slice())
    };

    let not_proven = verify(&shifted).unwrap_err();
    assert!(
        matches!(
            not_proven,
            VerifyError::NotProven(NotProven::EncodedPictureIsAnother {
                scene: 2,
                picture: 1
            })
        ),
        "{not_proven}"
    );
    // The scene that was written for this cut passes the same check.
    assert!(verify(&chosen.scene_files[1]).is_ok());
}

/// Splits `video` on the picture that is `CUT_FRAME` pictures in, wherever
/// the file says that picture is shown, and returns the report with how
/// many pictures the video has.
fn split_on_the_cut_frame_looked_up(video: &Path, out_dir: &Path) -> (ExportReport, usize) {
    let index = probe::read_packet_index(video).unwrap();
    let cuts = CutTimes::inside(index.video, vec![index.packets[CUT_FRAME].time]).unwrap();
    (split_exactly(video, out_dir, &cuts), index.packets.len())
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_with_pictures_shown_at_an_uneven_pace_is_cut_on_the_chosen_picture() {
    let video = TestVideo::with_reordered_pictures("exact_uneven_pace");
    // Every fifth picture is left out and the others keep their times, so
    // some pictures are shown twice as long as the rest.
    let keyframe_every = REORDERED_KEYFRAME_EVERY.to_string();
    let mut how = vec!["-map", "0", "-vf", "select='not(eq(mod(n,5),3))'"];
    how.extend(tools::picture_times_option().arguments());
    how.extend(["-c:v", "libx264", "-preset", "veryfast", "-bf", "2"]);
    how.extend(["-g", &keyframe_every, "-sc_threshold", "0", "-c:a", "copy"]);
    let uneven = rewritten(&video, "uneven.mp4", &how);
    let times = picture_times(&uneven);
    let longest_gap = times
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .fold(0.0, f64::max);
    assert!(
        (longest_gap - 2.0 * FRAME_SECONDS).abs() < MILLISECOND,
        "the test video is not uneven: {longest_gap}"
    );

    let (report, frames) = split_on_the_cut_frame_looked_up(&uneven, &video.folder().join("out"));

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, frames - CUT_FRAME]
    );
    assert!((proof_of(&report).cut_landings[0].landed_at - times[CUT_FRAME]).abs() < MILLISECOND);
    assert_pictures_keep_their_pace(&uneven, &report.scene_files);
    assert!(report.is_clean());
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_whose_pictures_begin_after_its_sound_is_cut_on_the_chosen_picture() {
    let video = TestVideo::with_reordered_pictures("exact_late_pictures");
    // The same pictures, shown from 0.4 seconds on, under sound that begins
    // at zero.
    let late = video.folder().join("late.mp4");
    tools::command(Program::Ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(&video.path)
        .args(["-itsoffset", "0.4", "-i"])
        .arg(&video.path)
        .args(["-map", "1:v", "-map", "0:a", "-c", "copy"])
        .arg(&late)
        .run(REWRITE_LIMIT)
        .unwrap();
    let first_picture_at = picture_times(&late)[0];
    assert!(
        (first_picture_at - 0.4).abs() < MILLISECOND,
        "the pictures of the test video begin at {first_picture_at}"
    );

    let (report, frames) = split_on_the_cut_frame_looked_up(&late, &video.folder().join("out"));

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, frames - CUT_FRAME]
    );
    let proof = proof_of(&report);
    assert!(proof.sound_packets > 0);
    assert!(proof.largest_sound_shift_seconds <= 2.0 * MILLISECOND);
    // The first scene still begins with sound alone, as the original does.
    let first_scene_begins_at = picture_times(&report.scene_files[0])[0];
    assert!(
        (first_scene_begins_at - first_picture_at).abs() <= 2.0 * MILLISECOND,
        "{first_scene_begins_at}"
    );
    assert_pictures_keep_their_pace(&late, &report.scene_files);
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn sound_of_other_kinds_and_in_several_streams_is_kept_packet_for_packet() {
    let video = TestVideo::with_reordered_pictures("exact_other_sound");
    // The sound twice: as it is, and as AC-3, whose packets are longer.
    let two_streams = rewritten(
        &video,
        "two_streams.mkv",
        &[
            "-map", "0:v", "-map", "0:a", "-map", "0:a", "-c:v", "copy", "-c:a:0", "copy",
            "-c:a:1", "ac3",
        ],
    );

    let (report, frames) =
        split_on_the_cut_frame_looked_up(&two_streams, &video.folder().join("out"));

    assert_eq!(
        picture_counts(&report.scene_files),
        vec![CUT_FRAME, frames - CUT_FRAME]
    );
    let proof = proof_of(&report);
    assert!(proof.largest_sound_shift_seconds <= 2.0 * MILLISECOND);
    for scene in &report.scene_files {
        assert_eq!(sound_stream_count(scene), 2, "{}", scene.display());
    }
}

fn sound_stream_count(video: &Path) -> usize {
    tools::command(Program::Ffprobe)
        .args(["-v", "error", "-select_streams", "a"])
        .args(["-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(video)
        .run_for_text(REWRITE_LIMIT)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

/// The reordered video encoded anew with `colours` stated in it, cut on a
/// picture between two keyframes. The second scene then begins with
/// pictures that were encoded again, so what ffprobe says of its colours is
/// what the encoder of the exact cut wrote. Returns what the original states.
fn split_a_video_that_states_its_colours(test_name: &str, colours: &[&str]) -> Vec<String> {
    let video = TestVideo::with_reordered_pictures(test_name);
    let keyframe_every = REORDERED_KEYFRAME_EVERY.to_string();
    let mut how = vec![
        "-map", "0", "-c:v", "libx264", "-preset", "veryfast", "-bf", "2",
    ];
    how.extend(["-g", &keyframe_every, "-sc_threshold", "0", "-c:a", "copy"]);
    how.extend(colours);
    let coloured = rewritten(&video, "coloured.mp4", &how);
    let cuts = cuts_at(REORDERED_VIDEO_SECONDS, &[time_of(CUT_FRAME)]);

    let report = split_exactly(&coloured, &video.folder().join("out"), &cuts);

    assert_eq!(
        proof_of(&report).pictures_encoded_again,
        REORDERED_KEYFRAME_EVERY
    );
    let stated_in_the_original = colours_of(&coloured);
    for scene in &report.scene_files {
        assert_eq!(
            colours_of(scene),
            stated_in_the_original,
            "{}",
            scene.display()
        );
    }
    stated_in_the_original
}

/// What ffprobe says about the colours of the pictures.
fn colours_of(video: &Path) -> Vec<String> {
    let streams = tools::command(Program::Ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_streams"])
        .arg(video)
        .run_for_text(REWRITE_LIMIT)
        .unwrap();
    streams
        .lines()
        .filter(|line| line.starts_with("color_"))
        .map(str::to_owned)
        .collect()
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_that_states_its_colours_keeps_them_in_every_scene() {
    let stated = split_a_video_that_states_its_colours(
        "exact_colours",
        &[
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
        ],
    );
    assert!(
        stated.contains(&"color_space=bt709".to_owned()),
        "the test video does not state its colours: {stated:?}"
    );
}

#[test]
#[ignore = "needs ffmpeg on the PATH"]
fn a_video_with_the_full_range_of_brightness_keeps_it_in_every_scene() {
    // As phones and cameras record: black is 0 and white 255, where films
    // keep to 16 and 235. Read the wrong way, the pictures around a cut
    // would be paler or harsher than the rest.
    let stated = split_a_video_that_states_its_colours(
        "exact_full_range",
        &[
            "-vf",
            "scale=out_range=full",
            "-color_range",
            "pc",
            "-colorspace",
            "bt709",
        ],
    );
    assert!(
        stated.contains(&"color_range=pc".to_owned()),
        "the test video does not have the full range: {stated:?}"
    );
}
