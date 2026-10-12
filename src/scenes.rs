//! Turns black screens into cut points and scenes.

use crate::black::TimeSpan;

/// One cut per black screen, placed on the keyframe nearest to it.
///
/// Splitting without re-encoding can only cut on a keyframe, because every
/// other frame depends on the frames before it. A keyframe inside the black
/// screen gives a clean cut. Otherwise the cut lands slightly before or after
/// it, and one scene keeps a moment of its neighbour.
pub fn cut_points(black_screens: &[TimeSpan], keyframe_times: &[f64], video: TimeSpan) -> Vec<f64> {
    let mut cuts: Vec<f64> = black_screens
        .iter()
        // A black opening or closing has footage on one side only, so it
        // separates nothing.
        .filter(|screen| screen.start > video.start && screen.end < video.end)
        .filter_map(|screen| nearest_keyframe(screen, keyframe_times))
        // A cut at the very start or end would only produce an empty scene.
        .filter(|&cut| cut > video.start && cut < video.end)
        .collect();
    cuts.dedup();
    cuts
}

/// The scenes between the cuts. Together they cover the whole video.
pub fn scenes_between(cuts: &[f64], video: TimeSpan) -> Vec<TimeSpan> {
    let starts = std::iter::once(video.start).chain(cuts.iter().copied());
    let ends = cuts.iter().copied().chain(std::iter::once(video.end));
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
    const VIDEO: TimeSpan = TimeSpan {
        start: 0.0,
        end: 10.0,
    };

    fn black(start: f64, end: f64) -> TimeSpan {
        TimeSpan { start, end }
    }

    #[test]
    fn a_keyframe_inside_the_black_screen_is_the_cut() {
        assert_eq!(cut_points(&[black(3.5, 4.5)], &KEYFRAMES, VIDEO), vec![4.0]);
    }

    #[test]
    fn otherwise_the_cut_is_the_closest_keyframe_before() {
        assert_eq!(
            cut_points(&[black(2.25, 3.0)], &KEYFRAMES, VIDEO),
            vec![2.0]
        );
    }

    #[test]
    fn or_the_closest_keyframe_after() {
        assert_eq!(
            cut_points(&[black(3.0, 3.75)], &KEYFRAMES, VIDEO),
            vec![4.0]
        );
    }

    #[test]
    fn two_black_screens_sharing_a_keyframe_give_one_cut() {
        assert_eq!(
            cut_points(&[black(4.25, 4.5), black(4.75, 5.0)], &KEYFRAMES, VIDEO),
            vec![4.0]
        );
    }

    #[test]
    fn a_black_opening_does_not_create_a_cut_at_time_zero() {
        assert!(cut_points(&[black(0.0, 0.5)], &KEYFRAMES, VIDEO).is_empty());
    }

    #[test]
    fn a_black_closing_does_not_split_off_a_sliver_of_black() {
        assert!(cut_points(&[black(7.5, VIDEO.end)], &KEYFRAMES, VIDEO).is_empty());
    }

    #[test]
    fn scenes_cover_the_whole_video() {
        assert_eq!(
            scenes_between(&[4.0, 8.0], VIDEO),
            vec![black(0.0, 4.0), black(4.0, 8.0), black(8.0, 10.0)]
        );
    }

    #[test]
    fn no_cuts_means_one_scene() {
        assert_eq!(scenes_between(&[], VIDEO), vec![black(0.0, 10.0)]);
    }
}
