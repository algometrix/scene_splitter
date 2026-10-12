//! End-to-end tests of the exact cut: generate a real video, split it on
//! chosen pictures with a real ffmpeg, and count what the scene files hold.
//!
//! Ignored by default, like the other end-to-end tests, because they need
//! `ffmpeg` and `ffprobe` on the PATH.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::exact_cut::CutPrecision;
use crate::exact_split::{self, ExactCutError, Unsupported};
use crate::exact_verify::ExactCutProof;
use crate::export::{
    self, ExportOptions, ExportReport, OriginalAlwaysKept, OriginalOutcome, SplitOrder,
    Verification,
};
use crate::fates::ScenesToWrite;
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
    // Older versions of ffmpeg store the turn asked for here. Newer ones
    // ignore the request, and the test then only shows that nothing is
    // turned that was not.
    let turned = rewritten(
        &video,
        "turned.mp4",
        &["-map", "0", "-c", "copy", "-metadata:s:v:0", "rotate=90"],
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
