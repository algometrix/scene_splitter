//! The chapter marks a video file carries, offered as splits beside the ones
//! a scan finds.
//!
//! A chapter mark says where a chapter begins, to the millisecond. A split
//! without re-encoding can only cut where the video allows it, so each mark
//! is moved to the nearest such point. A mark that sits at, or within a
//! second of, a split the scan already found adds no second split there. It
//! only lends that scene its title.

use serde::{Deserialize, Serialize};

use crate::scenes;
use crate::timecode::TimeSpan;

/// A chapter mark this close to a split the scan found is taken to mean the
/// same scene change. A black screen between two scenes lasts about this
/// long, and the mark and the scan rarely pick the same frame of it. Without
/// this, such a pair would leave a scene of a few frames between them.
pub const NEARLY_SAME_SECONDS: f64 = 1.0;

/// One chapter of a video file, as the file names it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    /// Where the chapter begins, in seconds on the clock the scan uses.
    pub start: f64,
    /// `None` for a chapter the file gives no name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// The title of the chapter that begins where a scene begins.
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterTitle {
    /// The start of the scene the title belongs to: the start of the video,
    /// a split the scan found, or a split a chapter mark added.
    pub scene_start: f64,
    pub title: String,
}

/// What the chapter marks of a video add to the splits its scan found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChapterSplits {
    /// The splits that exist only because of a chapter mark: ascending, each
    /// once, strictly inside the video, and none at a time the scan found.
    pub cuts: Vec<f64>,
    /// In ascending order of where their scenes start, one per start.
    pub titles: Vec<ChapterTitle>,
}

impl ChapterSplits {
    /// The title of the chapter that begins exactly where a scene begins.
    pub fn title_of_scene_starting_at(&self, scene_start: f64) -> Option<&str> {
        self.titles
            .iter()
            .find(|title| title.scene_start == scene_start)
            .map(|title| title.title.as_str())
    }
}

/// Where the scene of one chapter begins.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ChapterPlace {
    /// The chapter opens the video, so it needs no split.
    VideoStart,
    /// The scan found a split at, or nearly at, the mark.
    DetectedCut(f64),
    /// The mark gets a split of its own, at this time.
    OwnCut(f64),
    /// The mark is outside the video, or the video cannot be cut anywhere.
    Nowhere,
}

/// Places every chapter mark among the splits the scan found.
///
/// `detected_cuts` are the scan's own splits and `keyframe_times` the points
/// where the video can be cut, both in ascending order.
pub fn chapter_splits(
    chapters: &[Chapter],
    detected_cuts: &[f64],
    keyframe_times: &[f64],
    whole_video: TimeSpan,
) -> ChapterSplits {
    let mut splits = ChapterSplits::default();
    for chapter in chapters {
        let scene_start =
            match place_chapter(chapter.start, detected_cuts, keyframe_times, whole_video) {
                ChapterPlace::VideoStart => whole_video.start,
                ChapterPlace::DetectedCut(cut) => cut,
                ChapterPlace::OwnCut(cut) => {
                    splits.cuts.push(cut);
                    cut
                }
                ChapterPlace::Nowhere => continue,
            };
        if let Some(title) = &chapter.title {
            give_title(&mut splits.titles, scene_start, title);
        }
    }
    splits.cuts.sort_by(f64::total_cmp);
    splits.cuts.dedup();
    splits
        .titles
        .sort_by(|a, b| a.scene_start.total_cmp(&b.scene_start));
    splits
}

fn place_chapter(
    chapter_start: f64,
    detected_cuts: &[f64],
    keyframe_times: &[f64],
    whole_video: TimeSpan,
) -> ChapterPlace {
    if chapter_start.is_nan() || chapter_start >= whole_video.end {
        return ChapterPlace::Nowhere;
    }
    if chapter_start <= whole_video.start {
        return ChapterPlace::VideoStart;
    }
    let mark = TimeSpan {
        start: chapter_start,
        end: chapter_start,
    };
    let Some(cut_point) = scenes::nearest_keyframe(&mark, keyframe_times) else {
        return ChapterPlace::Nowhere;
    };
    if cut_point <= whole_video.start {
        return ChapterPlace::VideoStart;
    }

    // Measured from the mark and from where it would be cut, since either
    // can be the one that sits beside the scan's split.
    let gap_to = |detected: f64| {
        (detected - chapter_start)
            .abs()
            .min((detected - cut_point).abs())
    };
    let nearest_detected = detected_cuts
        .iter()
        .copied()
        .min_by(|&a, &b| gap_to(a).total_cmp(&gap_to(b)));
    match nearest_detected {
        Some(detected) if gap_to(detected) <= NEARLY_SAME_SECONDS => {
            ChapterPlace::DetectedCut(detected)
        }
        _ if whole_video.has_strictly_inside(cut_point) => ChapterPlace::OwnCut(cut_point),
        _ => ChapterPlace::Nowhere,
    }
}

/// Two marks can land on the same point when the video can only be cut far
/// apart. The scene then holds nearly all of the later chapter and next to
/// nothing of the earlier one, so the later title is the one that fits.
fn give_title(titles: &mut Vec<ChapterTitle>, scene_start: f64, title: &str) {
    let title = title.to_owned();
    match titles
        .iter_mut()
        .find(|given| given.scene_start == scene_start)
    {
        Some(given) => given.title = title,
        None => titles.push(ChapterTitle { scene_start, title }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHOLE_VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 100.0,
    };
    /// A point to cut on every ten seconds.
    const KEYFRAMES: [f64; 10] = [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0];
    const NO_DETECTED_CUTS: [f64; 0] = [];

    fn chapter(start: f64, title: &str) -> Chapter {
        Chapter {
            start,
            title: Some(title.to_owned()),
        }
    }

    fn untitled(start: f64) -> Chapter {
        Chapter { start, title: None }
    }

    fn title(scene_start: f64, title: &str) -> ChapterTitle {
        ChapterTitle {
            scene_start,
            title: title.to_owned(),
        }
    }

    fn splits_of(chapters: &[Chapter], detected_cuts: &[f64]) -> ChapterSplits {
        chapter_splits(chapters, detected_cuts, &KEYFRAMES, WHOLE_VIDEO)
    }

    #[test]
    fn a_video_without_chapters_gets_no_splits_from_them() {
        assert_eq!(splits_of(&[], &[40.0]), ChapterSplits::default());
    }

    #[test]
    fn a_chapter_mark_becomes_a_split_where_the_video_can_be_cut() {
        let splits = splits_of(&[untitled(33.0), untitled(58.0)], &NO_DETECTED_CUTS);
        assert_eq!(splits.cuts, vec![30.0, 60.0]);
        assert!(splits.titles.is_empty());
    }

    #[test]
    fn the_chapter_that_opens_the_video_adds_no_split_but_names_the_first_scene() {
        let splits = splits_of(&[chapter(0.0, "Opening")], &NO_DETECTED_CUTS);
        assert!(splits.cuts.is_empty());
        assert_eq!(splits.titles, vec![title(0.0, "Opening")]);
    }

    #[test]
    fn a_mark_nearest_to_the_first_frame_counts_as_the_opening() {
        // Cutting on the first frame would leave an empty first scene.
        let splits = splits_of(&[chapter(3.0, "Opening")], &NO_DETECTED_CUTS);
        assert!(splits.cuts.is_empty());
        assert_eq!(splits.titles, vec![title(0.0, "Opening")]);
    }

    #[test]
    fn a_mark_on_a_detected_split_adds_none_and_names_that_scene() {
        let splits = splits_of(&[chapter(40.0, "The heist")], &[40.0]);
        assert!(splits.cuts.is_empty());
        assert_eq!(splits.titles, vec![title(40.0, "The heist")]);
    }

    #[test]
    fn a_mark_that_would_be_cut_where_the_scan_cut_adds_no_second_split() {
        // The mark is four seconds from the detected split, but both can
        // only be cut at 40.
        let splits = splits_of(&[chapter(44.0, "The heist")], &[40.0]);
        assert!(splits.cuts.is_empty());
        assert_eq!(splits.titles, vec![title(40.0, "The heist")]);
    }

    #[test]
    fn a_mark_within_a_second_of_a_detected_split_joins_it() {
        // Where the video can be cut every half second, the scan cut at the
        // start of a black screen and the mark sits at its end.
        let keyframes = [0.0, 39.5, 40.0, 40.5, 41.0];
        let splits = chapter_splits(
            &[chapter(40.6, "Part two")],
            &[40.0],
            &keyframes,
            WHOLE_VIDEO,
        );
        assert!(splits.cuts.is_empty());
        assert_eq!(splits.titles, vec![title(40.0, "Part two")]);
    }

    #[test]
    fn a_mark_further_than_a_second_from_every_detected_split_gets_its_own() {
        let keyframes = [0.0, 38.0, 40.0, 42.0];
        let just_too_far = 40.0 + NEARLY_SAME_SECONDS + 1.0;
        let splits = chapter_splits(
            &[chapter(just_too_far, "Part two")],
            &[40.0],
            &keyframes,
            WHOLE_VIDEO,
        );
        assert_eq!(splits.cuts, vec![42.0]);
        assert_eq!(splits.titles, vec![title(42.0, "Part two")]);
    }

    #[test]
    fn the_nearest_detected_split_is_the_one_a_mark_joins() {
        let keyframes = [0.0, 39.5, 40.0, 40.5, 41.0];
        let splits = chapter_splits(
            &[chapter(40.4, "Part two")],
            &[39.5, 40.5],
            &keyframes,
            WHOLE_VIDEO,
        );
        assert_eq!(splits.titles, vec![title(40.5, "Part two")]);
    }

    #[test]
    fn two_marks_cut_at_the_same_point_give_one_split_named_by_the_later() {
        let splits = splits_of(
            &[chapter(28.0, "Short one"), chapter(31.0, "Long one")],
            &NO_DETECTED_CUTS,
        );
        assert_eq!(splits.cuts, vec![30.0]);
        assert_eq!(splits.titles, vec![title(30.0, "Long one")]);
    }

    #[test]
    fn an_untitled_mark_does_not_take_a_title_away() {
        let splits = splits_of(&[chapter(28.0, "Named"), untitled(31.0)], &NO_DETECTED_CUTS);
        assert_eq!(splits.titles, vec![title(30.0, "Named")]);
    }

    #[test]
    fn marks_outside_the_video_are_left_out() {
        let outside = [
            chapter(100.0, "At the end"),
            chapter(250.0, "Beyond the end"),
            chapter(f64::NAN, "Nowhere"),
        ];
        assert_eq!(
            splits_of(&outside, &NO_DETECTED_CUTS),
            ChapterSplits::default()
        );
    }

    #[test]
    fn a_mark_before_the_first_frame_counts_as_the_opening() {
        let late_video = TimeSpan {
            start: 5.0,
            end: 100.0,
        };
        let splits = chapter_splits(
            &[chapter(0.0, "Opening")],
            &NO_DETECTED_CUTS,
            &KEYFRAMES,
            late_video,
        );
        assert_eq!(splits.titles, vec![title(5.0, "Opening")]);
    }

    #[test]
    fn a_video_that_cannot_be_cut_gets_no_splits_from_its_chapters() {
        let splits = chapter_splits(
            &[chapter(40.0, "Part two")],
            &NO_DETECTED_CUTS,
            &[],
            WHOLE_VIDEO,
        );
        assert_eq!(splits, ChapterSplits::default());
    }

    #[test]
    fn marks_given_out_of_order_come_out_in_order() {
        let splits = splits_of(
            &[chapter(58.0, "Later"), chapter(33.0, "Earlier")],
            &NO_DETECTED_CUTS,
        );
        assert_eq!(splits.cuts, vec![30.0, 60.0]);
        assert_eq!(
            splits.titles,
            vec![title(30.0, "Earlier"), title(60.0, "Later")]
        );
    }

    #[test]
    fn a_title_is_found_by_the_exact_start_of_its_scene() {
        let splits = splits_of(&[chapter(33.0, "Part two")], &NO_DETECTED_CUTS);
        assert_eq!(splits.title_of_scene_starting_at(30.0), Some("Part two"));
        assert_eq!(splits.title_of_scene_starting_at(0.0), None);
    }

    #[test]
    fn a_chapter_without_a_title_is_stored_without_one() {
        let stored = serde_json::to_string(&untitled(4.0)).unwrap();
        assert_eq!(stored, r#"{"start":4.0}"#);
        assert_eq!(
            serde_json::from_str::<Chapter>(&stored).unwrap(),
            untitled(4.0)
        );
    }
}
