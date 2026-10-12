//! Finds the scene changes of a video: read the packet table, decode the
//! promising stretches, group the black frames, and place a cut on each.
//! On request it also decodes every frame to look for hard cuts.

use std::path::Path;

use anyhow::Result;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::black::{self, BlackCriteria, TimeSpan};
use crate::hardcut::{self, HardCutCandidate, HardCutSettings};
use crate::{probe, scan, scenes, windows};

/// How much of the video to decode while looking for black screens.
#[derive(Debug, Clone, Copy, PartialEq, ValueEnum, Serialize, Deserialize)]
pub enum ScanMode {
    /// Decode only where the packet sizes hint at a black screen. Fastest.
    Suspicious,
    /// Decode every frame. Use this when the black screens are noisy or
    /// grainy, since those do not compress into tiny packets.
    EveryFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScanSettings {
    pub mode: ScanMode,
    pub criteria: BlackCriteria,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            mode: ScanMode::Suspicious,
            criteria: BlackCriteria::default(),
        }
    }
}

/// What a scan looks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScanDepth {
    /// Black screens only. Takes a couple of seconds.
    BlackScreens,
    /// Black screens and hard cuts. Decodes every frame, which takes about
    /// as long as playing the video at 250 times its speed.
    WithHardCuts,
}

/// Everything the scan learned about one video that later steps need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    /// From the first frame appearing to the last frame disappearing.
    pub video: TimeSpan,
    /// The only times a split without re-encoding can cut on.
    pub keyframe_times: Vec<f64>,
    pub frame_count: usize,
    pub black_screens: Vec<TimeSpan>,
    /// One suggested cut per black screen, on the nearest keyframe.
    pub cuts: Vec<f64>,
    pub decoded_frames: usize,
    pub decoded_windows: usize,
    /// The abrupt picture changes in the video, or `None` when the scan did
    /// not look for hard cuts.
    #[serde(default)]
    pub hard_cut_candidates: Option<Vec<HardCutCandidate>>,
}

impl Detection {
    pub fn satisfies(&self, depth: ScanDepth) -> bool {
        match depth {
            ScanDepth::BlackScreens => true,
            ScanDepth::WithHardCuts => self.hard_cut_candidates.is_some(),
        }
    }

    /// The black-screen cuts plus the hard cuts chosen by `settings`. Without
    /// a hard-cut scan this is the black-screen cuts alone.
    pub fn cuts_with_hard_cuts(&self, settings: &HardCutSettings) -> Vec<f64> {
        let Some(candidates) = &self.hard_cut_candidates else {
            return self.cuts.clone();
        };
        hardcut::cuts_with_hard_cuts(
            &self.cuts,
            candidates,
            &self.keyframe_times,
            self.video,
            settings,
        )
    }
}

pub fn detect_scene_changes(
    video: &Path,
    settings: &ScanSettings,
    depth: ScanDepth,
) -> Result<Detection> {
    let index = probe::read_packet_index(video)?;
    let packets = &index.packets;
    let criteria = &settings.criteria;

    let windows = match settings.mode {
        ScanMode::Suspicious => windows::suspicious_windows(packets, criteria.min_seconds),
        ScanMode::EveryFrame => windows::whole_video_windows(packets, rayon::current_num_threads()),
    };
    let frame_is_black = scan::find_black_frames(video, packets, &windows, criteria)?;

    let frame_times: Vec<f64> = packets.iter().map(|packet| packet.time).collect();
    let black_screens = black::black_screens(
        &frame_times,
        &frame_is_black,
        index.video.end,
        criteria.min_seconds,
    );
    let keyframe_times = index.keyframe_times();
    let cuts = scenes::cut_points(&black_screens, &keyframe_times, index.video);

    let hard_cut_candidates = match depth {
        ScanDepth::BlackScreens => None,
        ScanDepth::WithHardCuts => {
            let whole_video = windows::whole_video_windows(packets, rayon::current_num_threads());
            Some(hardcut::find_candidates(video, packets, &whole_video)?)
        }
    };

    Ok(Detection {
        video: index.video,
        keyframe_times,
        frame_count: packets.len(),
        black_screens,
        cuts,
        decoded_frames: windows.iter().map(|window| window.len()).sum(),
        decoded_windows: windows.len(),
        hard_cut_candidates,
    })
}
