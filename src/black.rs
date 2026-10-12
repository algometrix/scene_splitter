//! Decides which frames are black and groups them into black screens.

use serde::{Deserialize, Serialize};

/// A stretch of the video, in seconds from its start. `end` is exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeSpan {
    pub start: f64,
    pub end: f64,
}

impl TimeSpan {
    pub fn seconds(&self) -> f64 {
        self.end - self.start
    }
}

/// What counts as a black screen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BlackCriteria {
    /// Brightest luma (0 to 255) a pixel may have and still count as dark.
    pub max_dark_luma: u8,
    /// Share of the pixels that must be dark for the frame to count as black.
    pub min_dark_fraction: f64,
    /// Shortest black screen that counts as a scene change.
    pub min_seconds: f64,
}

impl Default for BlackCriteria {
    /// Tuned on ordinary films: a quarter second of a frame that is 98%
    /// darker than a dim grey.
    fn default() -> Self {
        Self {
            max_dark_luma: 32,
            min_dark_fraction: 0.98,
            min_seconds: 0.25,
        }
    }
}

impl BlackCriteria {
    /// Says what is wrong with values no scan could work with.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.min_seconds.is_finite() || self.min_seconds <= 0.0 {
            return Err("the shortest black screen must be longer than zero seconds");
        }
        if !(self.min_dark_fraction > 0.0 && self.min_dark_fraction <= 1.0) {
            return Err("the share of a frame that must be black must be above 0 and at most 1");
        }
        Ok(())
    }

    pub fn is_black_frame(&self, luma: &[u8]) -> bool {
        if luma.is_empty() {
            return false;
        }
        let dark_pixels = luma
            .iter()
            .filter(|&&pixel| pixel <= self.max_dark_luma)
            .count();
        dark_pixels as f64 >= luma.len() as f64 * self.min_dark_fraction
    }
}

/// Groups consecutive black frames into spans and keeps the ones that last
/// long enough. A span ends where the next non-black frame begins.
pub fn black_screens(
    frame_times: &[f64],
    frame_is_black: &[bool],
    video_end: f64,
    min_seconds: f64,
) -> Vec<TimeSpan> {
    assert_eq!(frame_times.len(), frame_is_black.len());

    let mut screens = Vec::new();
    let mut frame = 0;
    while frame < frame_times.len() {
        if !frame_is_black[frame] {
            frame += 1;
            continue;
        }
        let first_black = frame;
        while frame < frame_times.len() && frame_is_black[frame] {
            frame += 1;
        }
        let screen = TimeSpan {
            start: frame_times[first_black],
            end: frame_times.get(frame).copied().unwrap_or(video_end),
        };
        if screen.seconds() >= min_seconds {
            screens.push(screen);
        }
    }
    screens
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRITERIA: BlackCriteria = BlackCriteria {
        max_dark_luma: 32,
        min_dark_fraction: 0.98,
        min_seconds: 0.25,
    };

    #[test]
    fn a_dark_frame_is_black() {
        assert!(CRITERIA.is_black_frame(&[16; 100]));
    }

    #[test]
    fn a_small_bright_logo_does_not_stop_a_frame_being_black() {
        let mut luma = [16; 100];
        luma[0] = 235;
        luma[1] = 235;
        assert!(CRITERIA.is_black_frame(&luma));
    }

    #[test]
    fn a_dim_scene_is_not_black() {
        let mut luma = [16; 100];
        luma[..10].fill(80);
        assert!(!CRITERIA.is_black_frame(&luma));
    }

    #[test]
    fn black_frames_group_into_a_screen_that_ends_at_the_next_picture() {
        let times = [0.0, 0.5, 1.0, 1.5, 2.0];
        let is_black = [false, true, true, false, false];
        assert_eq!(
            black_screens(&times, &is_black, 2.5, 0.25),
            vec![TimeSpan {
                start: 0.5,
                end: 1.5
            }]
        );
    }

    #[test]
    fn a_black_screen_shorter_than_the_minimum_is_ignored() {
        let times = [0.0, 0.5, 1.0];
        let is_black = [false, true, false];
        assert!(black_screens(&times, &is_black, 1.5, 1.0).is_empty());
    }

    #[test]
    fn a_black_screen_at_the_end_runs_to_the_end_of_the_video() {
        let times = [0.0, 0.5, 1.0];
        let is_black = [false, true, true];
        assert_eq!(
            black_screens(&times, &is_black, 1.5, 0.25),
            vec![TimeSpan {
                start: 0.5,
                end: 1.5
            }]
        );
    }
}
