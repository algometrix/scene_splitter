//! Turns black screens into cut points and scenes.

use std::fmt;

use crate::timecode::TimeSpan;

/// The times at which one video is cut: in ascending order, each time once,
/// and each strictly inside the video. Whatever writes scene files takes
/// this, so a cut outside the video cannot reach it.
#[derive(Debug, Clone, PartialEq)]
pub struct CutTimes {
    whole_video: TimeSpan,
    times: Vec<f64>,
}

/// A cut on an edge of the video or beyond it, or at a time that is not a
/// number. It would leave an empty scene, so no list of cuts may hold it.
#[derive(Debug, PartialEq)]
pub struct CutOutsideVideo {
    pub time: f64,
    pub whole_video: TimeSpan,
}

impl fmt::Display for CutOutsideVideo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a cut at {} seconds is not inside the video, which runs from {} to {} seconds",
            self.time, self.whole_video.start, self.whole_video.end
        )
    }
}

impl CutTimes {
    /// Sorts the times and drops repeats. Fails on the first time that is
    /// not strictly inside `whole_video`.
    pub fn inside(whole_video: TimeSpan, mut times: Vec<f64>) -> Result<Self, CutOutsideVideo> {
        let outside = times
            .iter()
            .find(|&&time| !whole_video.has_strictly_inside(time));
        if let Some(&time) = outside {
            return Err(CutOutsideVideo { time, whole_video });
        }
        times.sort_by(f64::total_cmp);
        times.dedup();
        Ok(Self { whole_video, times })
    }

    pub fn whole_video(&self) -> TimeSpan {
        self.whole_video
    }

    /// The scenes these cuts divide the video into.
    pub fn scenes(&self) -> Vec<TimeSpan> {
        scenes_between(&self.times, self.whole_video)
    }

    /// The times in seconds, in ascending order.
    pub fn as_slice(&self) -> &[f64] {
        &self.times
    }

    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
}

/// One cut per black screen, placed on the keyframe nearest to it.
///
/// Splitting without re-encoding can only cut on a keyframe, because every
/// other frame depends on the frames before it. A keyframe inside the black
/// screen gives a clean cut. Otherwise the cut lands slightly before or after
/// it, and one scene keeps a moment of its neighbour.
pub fn cut_points(
    black_screens: &[TimeSpan],
    keyframe_times: &[f64],
    whole_video: TimeSpan,
) -> Vec<f64> {
    let mut cuts: Vec<f64> = black_screens
        .iter()
        // A black opening or closing has footage on one side only, so it
        // separates nothing.
        .filter(|screen| screen.start > whole_video.start && screen.end < whole_video.end)
        .filter_map(|screen| nearest_keyframe(screen, keyframe_times))
        // A cut at the very start or end would only produce an empty scene.
        .filter(|&cut| whole_video.has_strictly_inside(cut))
        .collect();
    cuts.dedup();
    cuts
}

/// The scenes between the cuts. Together they cover the whole video.
pub fn scenes_between(cuts: &[f64], whole_video: TimeSpan) -> Vec<TimeSpan> {
    let starts = std::iter::once(whole_video.start).chain(cuts.iter().copied());
    let ends = cuts.iter().copied().chain(std::iter::once(whole_video.end));
    starts
        .zip(ends)
        .map(|(start, end)| TimeSpan { start, end })
        .collect()
}

/// The keyframe closest to the span, preferring one inside it.
pub fn nearest_keyframe(screen: &TimeSpan, keyframe_times: &[f64]) -> Option<f64> {
    let keyframes_before_end = keyframe_times.partition_point(|&time| time < screen.end);
    // The last keyframe before the screen ends is either inside the screen or
    // the closest one before it. The next keyframe is the closest one after.
    let before_or_inside = keyframes_before_end
        .checked_sub(1)
        .map(|index| keyframe_times[index]);
    let after = keyframe_times.get(keyframes_before_end).copied();

    [before_or_inside, after]
        .into_iter()
        .flatten()
        .min_by(|&a, &b| distance_to(screen, a).total_cmp(&distance_to(screen, b)))
}

/// Zero for a time inside the span, otherwise the gap to its nearer edge.
fn distance_to(span: &TimeSpan, time: f64) -> f64 {
    if time < span.start {
        span.start - time
    } else if time > span.end {
        time - span.end
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYFRAMES: [f64; 5] = [0.0, 2.0, 4.0, 6.0, 8.0];
    const WHOLE_VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 10.0,
    };

    fn black(start: f64, end: f64) -> TimeSpan {
        TimeSpan { start, end }
    }

    fn outside(time: f64) -> Result<CutTimes, CutOutsideVideo> {
        Err(CutOutsideVideo {
            time,
            whole_video: WHOLE_VIDEO,
        })
    }

    #[test]
    fn cut_times_given_out_of_order_come_out_ascending() {
        let cuts = CutTimes::inside(WHOLE_VIDEO, vec![8.0, 2.0, 6.0]).unwrap();
        assert_eq!(cuts.as_slice(), [2.0, 6.0, 8.0]);
    }

    #[test]
    fn a_cut_time_given_twice_is_kept_once() {
        let cuts = CutTimes::inside(WHOLE_VIDEO, vec![4.0, 8.0, 4.0]).unwrap();
        assert_eq!(cuts.as_slice(), [4.0, 8.0]);
        assert_eq!(cuts.len(), 2);
    }

    #[test]
    fn no_cut_times_at_all_are_valid() {
        let cuts = CutTimes::inside(WHOLE_VIDEO, Vec::new()).unwrap();
        assert!(cuts.is_empty());
        assert_eq!(cuts.whole_video(), WHOLE_VIDEO);
    }

    #[test]
    fn a_cut_on_either_edge_of_the_video_is_refused() {
        assert_eq!(
            CutTimes::inside(WHOLE_VIDEO, vec![WHOLE_VIDEO.start]),
            outside(WHOLE_VIDEO.start)
        );
        assert_eq!(
            CutTimes::inside(WHOLE_VIDEO, vec![4.0, WHOLE_VIDEO.end]),
            outside(WHOLE_VIDEO.end)
        );
    }

    #[test]
    fn a_cut_beyond_the_video_is_refused() {
        assert_eq!(
            CutTimes::inside(WHOLE_VIDEO, vec![4.0, 12.0]),
            outside(12.0)
        );
        assert_eq!(CutTimes::inside(WHOLE_VIDEO, vec![-1.0]), outside(-1.0));
    }

    #[test]
    fn a_cut_time_that_is_not_a_number_is_refused() {
        let refused = CutTimes::inside(WHOLE_VIDEO, vec![4.0, f64::NAN]).unwrap_err();
        assert!(refused.time.is_nan());
    }

    #[test]
    fn a_refused_cut_is_explained_with_its_time_and_the_video() {
        let refused = CutTimes::inside(WHOLE_VIDEO, vec![12.5]).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "a cut at 12.5 seconds is not inside the video, which runs from 0 to 10 seconds"
        );
    }

    #[test]
    fn cut_times_divide_their_video_into_scenes() {
        let cuts = CutTimes::inside(LATE_VIDEO, vec![8.0, 4.0]).unwrap();
        assert_eq!(
            cuts.scenes(),
            vec![black(1.5, 4.0), black(4.0, 8.0), black(8.0, 9.5)]
        );
    }

    #[test]
    fn without_cut_times_the_whole_video_is_one_scene() {
        let cuts = CutTimes::inside(WHOLE_VIDEO, Vec::new()).unwrap();
        assert_eq!(cuts.scenes(), vec![WHOLE_VIDEO]);
    }

    #[test]
    fn a_keyframe_inside_the_black_screen_is_the_cut() {
        assert_eq!(
            cut_points(&[black(3.5, 4.5)], &KEYFRAMES, WHOLE_VIDEO),
            vec![4.0]
        );
    }

    #[test]
    fn otherwise_the_cut_is_the_closest_keyframe_before() {
        assert_eq!(
            cut_points(&[black(2.25, 3.0)], &KEYFRAMES, WHOLE_VIDEO),
            vec![2.0]
        );
    }

    #[test]
    fn or_the_closest_keyframe_after() {
        assert_eq!(
            cut_points(&[black(3.0, 3.75)], &KEYFRAMES, WHOLE_VIDEO),
            vec![4.0]
        );
    }

    #[test]
    fn two_black_screens_sharing_a_keyframe_give_one_cut() {
        assert_eq!(
            cut_points(
                &[black(4.25, 4.5), black(4.75, 5.0)],
                &KEYFRAMES,
                WHOLE_VIDEO
            ),
            vec![4.0]
        );
    }

    #[test]
    fn a_black_opening_does_not_create_a_cut_at_time_zero() {
        assert!(cut_points(&[black(0.0, 0.5)], &KEYFRAMES, WHOLE_VIDEO).is_empty());
    }

    #[test]
    fn a_black_closing_does_not_split_off_a_sliver_of_black() {
        assert!(cut_points(&[black(7.5, WHOLE_VIDEO.end)], &KEYFRAMES, WHOLE_VIDEO).is_empty());
    }

    #[test]
    fn scenes_cover_the_whole_video() {
        assert_eq!(
            scenes_between(&[4.0, 8.0], WHOLE_VIDEO),
            vec![black(0.0, 4.0), black(4.0, 8.0), black(8.0, 10.0)]
        );
    }

    #[test]
    fn no_cuts_means_one_scene() {
        assert_eq!(scenes_between(&[], WHOLE_VIDEO), vec![black(0.0, 10.0)]);
    }

    /// A video whose first frame is not at time zero.
    const LATE_VIDEO: TimeSpan = TimeSpan {
        start: 1.5,
        end: 9.5,
    };

    #[test]
    fn no_black_screens_means_no_cuts() {
        assert!(cut_points(&[], &KEYFRAMES, WHOLE_VIDEO).is_empty());
    }

    #[test]
    fn a_video_without_keyframes_cannot_be_cut() {
        assert!(cut_points(&[black(3.5, 4.5)], &[], WHOLE_VIDEO).is_empty());
    }

    #[test]
    fn a_keyframe_on_the_first_frame_after_the_black_screen_is_the_cut() {
        // The keyframe at 2.0 is only a quarter second before the screen,
        // but the one at 4.0 keeps all of the black in the earlier scene.
        assert_eq!(
            cut_points(&[black(2.25, 4.0)], &KEYFRAMES, WHOLE_VIDEO),
            vec![4.0]
        );
    }

    #[test]
    fn keyframes_on_both_edges_of_a_black_screen_cut_at_its_start() {
        assert_eq!(
            cut_points(&[black(2.0, 4.0)], &KEYFRAMES, WHOLE_VIDEO),
            vec![2.0]
        );
    }

    #[test]
    fn with_several_keyframes_inside_the_black_screen_the_last_one_is_the_cut() {
        assert_eq!(
            cut_points(&[black(1.5, 6.5)], &KEYFRAMES, WHOLE_VIDEO),
            vec![6.0]
        );
    }

    #[test]
    fn a_tie_between_two_equally_near_keyframes_goes_to_the_earlier_one() {
        // Half a second after the keyframe at 2.0, half a second before 4.0.
        assert_eq!(nearest_keyframe(&black(2.5, 3.5), &KEYFRAMES), Some(2.0));
    }

    #[test]
    fn a_video_that_is_black_throughout_has_no_cuts() {
        let all_black = [black(WHOLE_VIDEO.start, WHOLE_VIDEO.end)];
        assert!(cut_points(&all_black, &KEYFRAMES, WHOLE_VIDEO).is_empty());
    }

    #[test]
    fn black_at_both_ends_leaves_only_the_cut_in_the_middle() {
        let screens = [
            black(WHOLE_VIDEO.start, 0.5),
            black(3.5, 4.5),
            black(9.0, WHOLE_VIDEO.end),
        ];
        assert_eq!(cut_points(&screens, &KEYFRAMES, WHOLE_VIDEO), vec![4.0]);
    }

    #[test]
    fn a_black_screen_nearest_to_the_first_frame_gives_no_cut() {
        // Cutting on the keyframe at 0.0 would make an empty first scene.
        assert!(cut_points(&[black(0.25, 0.75)], &KEYFRAMES, WHOLE_VIDEO).is_empty());
    }

    #[test]
    fn a_black_opening_of_a_video_that_starts_late_is_not_a_cut() {
        // The keyframe at 2.0 is inside the screen, but there is no footage
        // before it to split off.
        assert!(cut_points(&[black(LATE_VIDEO.start, 2.5)], &KEYFRAMES, LATE_VIDEO).is_empty());
    }

    #[test]
    fn cuts_come_out_in_the_order_of_the_black_screens() {
        let screens = [black(1.5, 2.5), black(3.5, 4.5), black(5.5, 6.5)];
        assert_eq!(
            cut_points(&screens, &KEYFRAMES, WHOLE_VIDEO),
            vec![2.0, 4.0, 6.0]
        );
    }

    #[test]
    fn scenes_start_where_the_video_starts_rather_than_at_zero() {
        assert_eq!(
            scenes_between(&[4.0], LATE_VIDEO),
            vec![black(1.5, 4.0), black(4.0, 9.5)]
        );
    }
}
