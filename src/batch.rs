//! One run of splitting every video that is ready: which video's turn it
//! is, how each one went, and what to tell the person at the end.
//!
//! The videos are split one after another. A split is limited by the drive
//! it reads from and writes to, so two at once would only make both slower.
//! One video going wrong never stops the rest. Nothing here writes a file or
//! starts a thread: the window asks whose turn it is and says how it went.

use std::collections::VecDeque;

use crate::queue::JobId;

/// How the turn of one video ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitOutcome {
    /// Written and checked, and the original dealt with as asked.
    Clean,
    /// Written, but the check did not pass or the original could not be
    /// moved. The page of the video says which.
    NeedsALook,
    /// Nothing usable was written. The video keeps its review.
    Failed,
    /// Scene files of an earlier split are where the scenes would go, and
    /// the person never agreed to replace those. The video is back in
    /// review with that question open.
    EarlierFilesInTheWay,
    /// The person took the video back into review before its turn came.
    NoLongerReady,
}

/// How far a run has come.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchProgress {
    /// The videos whose turn is over, however it ended.
    pub done: usize,
    pub video_count: usize,
}

impl BatchProgress {
    /// The share of the videos that are done, from 0.0 to 1.0.
    pub fn fraction(self) -> f32 {
        if self.video_count == 0 {
            return 1.0;
        }
        self.done as f32 / self.video_count as f32
    }
}

#[derive(Debug, PartialEq)]
pub struct SplitBatch {
    /// The videos whose turn is still to come, in the order of the list.
    waiting: VecDeque<JobId>,
    /// The video being split now.
    splitting: Option<JobId>,
    /// How each finished turn ended, in the order they were taken.
    outcomes: Vec<SplitOutcome>,
    video_count: usize,
}

impl SplitBatch {
    /// A run over the videos that are ready now, or `None` when none is. A
    /// video that becomes ready later is not part of this run.
    pub fn of(ready_videos: Vec<JobId>) -> Option<Self> {
        if ready_videos.is_empty() {
            return None;
        }
        Some(Self {
            video_count: ready_videos.len(),
            waiting: ready_videos.into(),
            splitting: None,
            outcomes: Vec::new(),
        })
    }

    /// The video whose turn begins now, or `None` while another is being
    /// split or when every video has had its turn.
    pub fn take_next(&mut self) -> Option<JobId> {
        if self.splitting.is_some() {
            return None;
        }
        self.splitting = self.waiting.pop_front();
        self.splitting
    }

    /// The turn of `video` is over. Ignored for a video whose turn it is
    /// not, such as one the person chose to split by itself during the run.
    pub fn turn_ended(&mut self, video: JobId, outcome: SplitOutcome) {
        if self.splitting != Some(video) {
            return;
        }
        self.splitting = None;
        self.outcomes.push(outcome);
    }

    pub fn is_over(&self) -> bool {
        self.splitting.is_none() && self.waiting.is_empty()
    }

    pub fn progress(&self) -> BatchProgress {
        BatchProgress {
            done: self.outcomes.len(),
            video_count: self.video_count,
        }
    }

    fn count(&self, wanted: SplitOutcome) -> usize {
        self.outcomes
            .iter()
            .filter(|&&outcome| outcome == wanted)
            .count()
    }

    /// What the run came to, for the person to read once it is over: what
    /// worked first, then each thing that did not, with what to do about it.
    pub fn summary(&self) -> String {
        let clean = self.count(SplitOutcome::Clean);
        let mut sentences = vec![if clean == self.video_count {
            all_split_in_words(clean)
        } else {
            format!(
                "Split and checked: {clean} of {}.",
                counted_videos(self.video_count)
            )
        }];
        let troubles = [
            (
                SplitOutcome::NeedsALook,
                "split, but the result needs a look. Open it to see why.",
            ),
            (
                SplitOutcome::Failed,
                "not split. The original was not changed. Open it to go back to its review.",
            ),
            (
                SplitOutcome::EarlierFilesInTheWay,
                "not split, because scene files from an earlier split are in the way. \
                 Open it to decide.",
            ),
            (
                SplitOutcome::NoLongerReady,
                "left out, because it was taken back into review.",
            ),
        ];
        for (outcome, what_happened) in troubles {
            match self.count(outcome) {
                0 => {}
                1 => sentences.push(format!("1 was {what_happened}")),
                several => sentences.push(format!("{several} were {what_happened}")),
            }
        }
        sentences.join(" ")
    }
}

fn all_split_in_words(video_count: usize) -> String {
    match video_count {
        1 => "The video was split and checked.".to_owned(),
        2 => "Both videos were split and checked.".to_owned(),
        several => format!("All {several} videos were split and checked."),
    }
}

fn counted_videos(video_count: usize) -> String {
    if video_count == 1 {
        "1 video".to_owned()
    } else {
        format!("{video_count} videos")
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::queue::{Added, Queue};

    /// The names of `video_count` videos, as a queue gives them out.
    fn videos(video_count: usize) -> Vec<JobId> {
        let mut queue = Queue::default();
        (0..video_count)
            .map(|number| {
                let Added::New(id) = queue.add(Path::new(&format!("video_{number}.mp4"))) else {
                    panic!("each of these videos is new to the queue");
                };
                id
            })
            .collect()
    }

    /// A run over `outcomes.len()` videos in which each ended as given.
    fn run_ending(outcomes: &[SplitOutcome]) -> SplitBatch {
        let mut batch = SplitBatch::of(videos(outcomes.len())).unwrap();
        for &outcome in outcomes {
            let video = batch.take_next().unwrap();
            batch.turn_ended(video, outcome);
        }
        assert!(batch.is_over());
        batch
    }

    #[test]
    fn there_is_no_run_without_a_ready_video() {
        assert_eq!(SplitBatch::of(Vec::new()), None);
    }

    #[test]
    fn videos_take_their_turn_one_after_another_in_the_order_of_the_list() {
        let ids = videos(3);
        let mut batch = SplitBatch::of(ids.clone()).unwrap();

        assert_eq!(batch.take_next(), Some(ids[0]));
        // The second waits until the first is done.
        assert_eq!(batch.take_next(), None);
        assert!(!batch.is_over());

        batch.turn_ended(ids[0], SplitOutcome::Clean);
        assert_eq!(batch.take_next(), Some(ids[1]));
        batch.turn_ended(ids[1], SplitOutcome::Clean);
        assert_eq!(batch.take_next(), Some(ids[2]));
        assert!(!batch.is_over());

        batch.turn_ended(ids[2], SplitOutcome::Clean);
        assert_eq!(batch.take_next(), None);
        assert!(batch.is_over());
    }

    #[test]
    fn a_failure_does_not_stop_the_videos_after_it() {
        let ids = videos(3);
        let mut batch = SplitBatch::of(ids.clone()).unwrap();
        let first = batch.take_next().unwrap();
        batch.turn_ended(first, SplitOutcome::Failed);

        assert_eq!(batch.take_next(), Some(ids[1]));
        assert!(!batch.is_over());
    }

    #[test]
    fn progress_counts_every_turn_that_is_over() {
        let ids = videos(4);
        let mut batch = SplitBatch::of(ids).unwrap();
        assert_eq!(
            batch.progress(),
            BatchProgress {
                done: 0,
                video_count: 4
            }
        );

        let first = batch.take_next().unwrap();
        batch.turn_ended(first, SplitOutcome::Failed);
        let second = batch.take_next().unwrap();
        assert_eq!(batch.progress().done, 1);
        assert_eq!(batch.progress().fraction(), 0.25);

        batch.turn_ended(second, SplitOutcome::Clean);
        assert_eq!(batch.progress().fraction(), 0.5);
    }

    #[test]
    fn the_end_of_a_split_that_is_not_the_runs_turn_is_ignored() {
        let ids = videos(3);
        let mut batch = SplitBatch::of(vec![ids[0], ids[1]]).unwrap();
        let first = batch.take_next().unwrap();

        // The third video was split by itself while the run was going.
        batch.turn_ended(ids[2], SplitOutcome::Clean);
        // And the second has not had its turn yet.
        batch.turn_ended(ids[1], SplitOutcome::Clean);
        assert_eq!(batch.progress().done, 0);
        assert_eq!(batch.take_next(), None);

        batch.turn_ended(first, SplitOutcome::Clean);
        assert_eq!(batch.progress().done, 1);
    }

    #[test]
    fn a_run_in_which_everything_worked_says_so_in_one_sentence() {
        let clean = SplitOutcome::Clean;
        assert_eq!(
            run_ending(&[clean]).summary(),
            "The video was split and checked."
        );
        assert_eq!(
            run_ending(&[clean, clean]).summary(),
            "Both videos were split and checked."
        );
        assert_eq!(
            run_ending(&[clean, clean, clean]).summary(),
            "All 3 videos were split and checked."
        );
    }

    #[test]
    fn a_run_with_trouble_says_what_worked_and_then_each_thing_that_did_not() {
        let batch = run_ending(&[
            SplitOutcome::Clean,
            SplitOutcome::Failed,
            SplitOutcome::Clean,
            SplitOutcome::EarlierFilesInTheWay,
            SplitOutcome::Failed,
        ]);
        assert_eq!(
            batch.summary(),
            "Split and checked: 2 of 5 videos. \
             2 were not split. The original was not changed. \
             Open it to go back to its review. \
             1 was not split, because scene files from an earlier split are in the way. \
             Open it to decide."
        );
    }

    #[test]
    fn every_kind_of_trouble_has_a_sentence_of_its_own() {
        let batch = run_ending(&[
            SplitOutcome::NeedsALook,
            SplitOutcome::Failed,
            SplitOutcome::EarlierFilesInTheWay,
            SplitOutcome::NoLongerReady,
        ]);
        let summary = batch.summary();
        assert!(
            summary.starts_with("Split and checked: 0 of 4 videos."),
            "{summary}"
        );
        for said in [
            "the result needs a look",
            "The original was not changed",
            "are in the way",
            "taken back into review",
        ] {
            assert!(summary.contains(said), "{summary}");
        }
    }

    #[test]
    fn a_run_of_one_video_that_failed_says_so() {
        assert_eq!(
            run_ending(&[SplitOutcome::Failed]).summary(),
            "Split and checked: 0 of 1 video. 1 was not split. \
             The original was not changed. Open it to go back to its review."
        );
    }
}
