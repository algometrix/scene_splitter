//! Splits a video into scenes at stretches of black screen.
//!
//! Run without arguments to open the window and drag videos in. Run with a
//! video to split it straight from the command line, or with `--scan-only`
//! to scan videos ahead of time and review them in the window later.

mod app;
mod black;
mod cache;
mod detect;
#[cfg(test)]
mod e2e_tests;
mod export;
mod fates;
mod ffmpeg;
mod focus;
mod folders;
mod hardcut;
mod keyboard;
mod library;
mod plan;
mod player;
mod prescan;
mod probe;
mod queue;
mod review;
mod review_session;
mod scan;
mod scene_list;
mod scene_name;
mod scenes;
mod selection;
mod settings;
mod settings_controls;
mod split;
#[cfg(test)]
mod test_videos;
mod theme;
mod thumbnails;
mod timecode;
mod tools;
mod verify;
mod windows;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Parser;

use detect::{Detection, ScanDepth, ScanKind, ScanMode};
use export::{ExistingListFile, OriginalPolicy, SplitOrder};
use fates::ScenesToWrite;
use scene_list::{FrameRate, SceneList, SceneListFormat};
use scene_name::SceneNamePattern;
use scenes::CutTimes;
use settings::Settings;
use timecode::{format_timestamp, TimeSpan};

/// How often the download of ffmpeg reports its progress on the console.
const PROGRESS_STEP_PERCENT: u32 = 10;

/// Detection and output options left out here come from the saved settings,
/// which the window's Settings section edits.
#[derive(Parser)]
#[command(version, about)]
struct Arguments {
    /// The video to split. Leave it out to open the window, then drag
    /// videos or a folder onto it. A folder here opens the window with every
    /// video in it queued, or scans them all with --scan-only.
    video: Option<PathBuf>,

    /// Scan the video, or every video directly inside the folder, and store
    /// the results without splitting anything. The window then opens those
    /// videos ready for review. Detection options given here must match the
    /// window's settings for the stored scans to be reused.
    #[arg(long, requires = "video", conflicts_with_all = ["review", "list_only", "out_dir", "overwrite", "keep_original", "scene_name", "save_list"])]
    scan_only: bool,

    /// Open the given video in the window instead of splitting it at once.
    /// With --hard-cuts the window starts with the hard-cut scan.
    #[arg(long, requires = "video", conflicts_with_all = ["list_only", "out_dir", "overwrite", "keep_original", "scene_name", "save_list"])]
    review: bool,

    /// Where to write the scene files. Defaults to the saved setting, which
    /// is the video's own folder unless changed.
    #[arg(long, requires = "video")]
    out_dir: Option<PathBuf>,

    /// Print the scenes without writing any files.
    #[arg(long, requires = "video")]
    list_only: bool,

    /// Replace scene files left in the output folder by an earlier run, and
    /// a scene list that --save-list finds in its place.
    #[arg(long, requires = "video")]
    overwrite: bool,

    /// How to name the scene files, whatever the saved setting says. Use
    /// {name} for the video's name and {number} for the scene's number,
    /// which both have to be there, and if you like {count}, {start} and
    /// {length}. The video's extension is added. The default is
    /// "{name}_scene_{number}".
    #[arg(long, requires = "video", value_name = "PATTERN")]
    scene_name: Option<SceneNamePattern>,

    /// Write the list of scenes to this file: their numbers, times and the
    /// names their files get. Works with --list-only, which then writes
    /// the list and no scene files.
    #[arg(long, requires = "video", value_name = "FILE")]
    save_list: Option<PathBuf>,

    /// The format of the scene list: a table (csv), chapter marks for
    /// ffmpeg (chapters) or a CMX3600 edit list (edl). Left out, it is
    /// taken from the ending of the file's name: .csv, .txt or .edl.
    #[arg(long, value_enum, requires = "save_list", value_name = "FORMAT")]
    list_format: Option<SceneListFormat>,

    /// Keep the original video, whatever the saved setting says. Unless
    /// kept, the original is moved to the Recycle Bin (the Trash on macOS
    /// and Linux), but only after checking that the scene files hold every
    /// packet of it, unchanged.
    #[arg(long, requires = "video", conflicts_with = "list_only")]
    keep_original: bool,

    /// Also look for hard cuts: scene changes with no black screen. This
    /// decodes every frame, so it takes far longer than the normal scan.
    #[arg(long, requires = "video")]
    hard_cuts: bool,

    /// How much of the video to decode while looking for black screens.
    #[arg(long, value_enum)]
    scan: Option<ScanMode>,

    /// Shortest black screen, in seconds, that counts as a scene change.
    #[arg(long)]
    min_black: Option<f64>,

    /// Brightest pixel value (0 to 255) that still counts as black.
    #[arg(long)]
    max_luma: Option<u8>,

    /// Share of a frame's pixels that must be black, from 0 to 1.
    #[arg(long)]
    black_fraction: Option<f64>,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let loaded = Settings::load();
    if let Some(problem) = &loaded.problem {
        eprintln!("{problem}");
    }

    let Some(path) = &arguments.video else {
        refuse_overrides_for_the_window(&arguments)?;
        return app::run(loaded, None, ScanKind::BlackScreens);
    };
    if !path.exists() {
        bail!("{} does not exist", path.display());
    }
    let for_one_video_only = arguments.list_only
        || arguments.overwrite
        || arguments.out_dir.is_some()
        || arguments.save_list.is_some();
    if path.is_dir() && !arguments.scan_only && for_one_video_only {
        bail!(
            "{} is a folder. --list-only, --out-dir, --overwrite and --save-list work on a \
             single video. Use --scan-only to scan a folder, or give the folder alone to open \
             it in the window.",
            path.display()
        );
    }

    // The window gets the tools itself, with a progress bar.
    if !arguments.scan_only && (arguments.review || path.is_dir()) {
        refuse_overrides_for_the_window(&arguments)?;
        let first_scan = scan_kind(&arguments, &loaded.settings);
        return app::run(loaded, Some(path.clone()), first_scan);
    }
    let settings = settings_for_this_run(loaded.settings, &arguments)?;

    get_tools_if_missing()?;
    if arguments.scan_only {
        return scan_ahead(path, &arguments, &settings);
    }
    split_from_command_line(path, &arguments, &settings)
}

/// Downloads ffmpeg and ffprobe on a machine that has neither, printing
/// progress as it goes.
fn get_tools_if_missing() -> Result<()> {
    if tools::find_installed() == tools::Availability::Ready {
        return Ok(());
    }
    println!(
        "ffmpeg was not found. Downloading ffmpeg {} (this happens once).",
        tools::FFMPEG_VERSION
    );
    let mut percent_reported = 0;
    tools::download(|progress| {
        let percent = (progress.fraction() * 100.0) as u32;
        if percent >= percent_reported + PROGRESS_STEP_PERCENT {
            percent_reported = percent;
            println!("  {percent}%");
        }
    })
    .context("ffmpeg could not be downloaded")?;
    println!("ffmpeg is ready.");
    Ok(())
}

/// The window saves its settings whenever one changes. An option given for
/// one run would be saved along with them, so the window takes none.
fn refuse_overrides_for_the_window(arguments: &Arguments) -> Result<()> {
    let overrides_given = arguments.scan.is_some()
        || arguments.min_black.is_some()
        || arguments.max_luma.is_some()
        || arguments.black_fraction.is_some()
        || arguments.keep_original
        || arguments.scene_name.is_some();
    if overrides_given {
        bail!(
            "--scan, --min-black, --max-luma, --black-fraction, --keep-original and --scene-name \
             apply to a command line run. For the window, change them in its Settings section."
        );
    }
    Ok(())
}

/// The saved settings, with whatever the command line overrides.
fn settings_for_this_run(mut settings: Settings, arguments: &Arguments) -> Result<Settings> {
    if let Some(mode) = arguments.scan {
        settings.scan.mode = mode;
    }
    if let Some(min_black) = arguments.min_black {
        settings.scan.criteria.min_seconds = min_black;
    }
    if let Some(max_luma) = arguments.max_luma {
        settings.scan.criteria.max_dark_luma = max_luma;
    }
    if let Some(black_fraction) = arguments.black_fraction {
        settings.scan.criteria.min_dark_fraction = black_fraction;
    }
    if arguments.keep_original {
        settings.export.original_policy = OriginalPolicy::Keep;
    }
    if let Some(scene_name) = arguments.scene_name {
        settings.export.scene_name = scene_name;
    }
    settings
        .validate()
        .context("the options given cannot be used")?;
    Ok(settings)
}

/// The scan the arguments ask for. A hard-cut scan thins its cuts with the
/// saved settings.
fn scan_kind(arguments: &Arguments, settings: &Settings) -> ScanKind {
    if arguments.hard_cuts {
        ScanKind::HardCuts(settings.hard_cuts)
    } else {
        ScanKind::BlackScreens
    }
}

fn scan_depth(arguments: &Arguments, settings: &Settings) -> ScanDepth {
    scan_kind(arguments, settings).depth()
}

fn scan_ahead(path: &Path, arguments: &Arguments, settings: &Settings) -> Result<()> {
    let depth = scan_depth(arguments, settings);
    let summary = prescan::scan_ahead(path, &settings.scan, depth, |video_done| {
        println!("{video_done}");
        for stretch in &video_done.undecoded {
            eprintln!("Warning: in {}, {stretch}", video_done.video.display());
        }
    })?;
    if summary.is_empty() {
        println!("No videos were found in {}.", path.display());
        return Ok(());
    }
    println!("{}", summary.describe());
    if !summary.failed.is_empty() {
        for failed in &summary.failed {
            eprintln!("{}: {}", failed.video.display(), failed.reason);
        }
        bail!("{} videos could not be scanned", summary.failed.len());
    }
    Ok(())
}

fn split_from_command_line(video: &Path, arguments: &Arguments, settings: &Settings) -> Result<()> {
    let out_dir = match &arguments.out_dir {
        Some(out_dir) => out_dir.clone(),
        None => split::scene_dir(video, settings.export.scene_location)?,
    };

    let started = Instant::now();
    // A run from the command line shows no progress bar.
    let scan = detect::scan_for_scene_changes(
        video,
        &settings.scan,
        scan_depth(arguments, settings),
        &detect::no_scan_progress,
    )?;
    let detection_seconds = started.elapsed().as_secs_f64();
    for stretch in &scan.undecoded {
        eprintln!("Warning: in {}, {stretch}", video.display());
    }
    let detection = scan.detection;
    // Checked once, here, so that neither the listing nor the split can be
    // handed a cut that is not inside the video.
    let cuts = CutTimes::inside(
        detection.video,
        detection.cuts_for(scan_kind(arguments, settings)),
    )
    .map_err(|outside| {
        anyhow::anyhow!("the scan of {} made no sense: {outside}", video.display())
    })?;

    println!(
        "Decoded {} of {} frames in {} windows.",
        detection.decoded_frames, detection.frame_count, detection.decoded_windows
    );
    print_black_screens(&detection.black_screens);
    if let Some(candidates) = &detection.hard_cut_candidates {
        println!(
            "Sudden picture changes: {}, of which {} became cuts.",
            candidates.len(),
            cuts.len() - detection.cuts.len()
        );
    }
    print_scenes(&cuts.scenes());
    println!("Detection took {detection_seconds:.2}s.");

    // The command line has no review, so it has no way to drop a scene.
    let scenes = ScenesToWrite::every_scene(cuts);
    if let Some(list_file) = &arguments.save_list {
        save_scene_list(video, &detection, &scenes, arguments, settings, list_file)?;
        println!("Saved the scene list to {}.", list_file.display());
    }

    if arguments.list_only {
        return Ok(());
    }
    if scenes.cuts().is_empty() {
        println!("No scene changes found, nothing to split.");
        return Ok(());
    }

    // Splitting replaces files of the same name, so an earlier run's scenes
    // are only overwritten when the user asked for that.
    let scene_name = settings.export.scene_name;
    let existing_scenes = split::scene_files_from_earlier_splits(video, &out_dir, scene_name)?;
    if !arguments.overwrite && !existing_scenes.is_empty() {
        bail!(
            "{} scene files from an earlier split are in {}. Pass --overwrite to replace them, or choose another --out-dir.",
            existing_scenes.len(),
            out_dir.display()
        );
    }

    let started = Instant::now();
    let order = SplitOrder::new(scenes, settings.export.original_policy);
    let report = export::export_scenes(video, &order, &out_dir, scene_name)?;
    println!("{}", report.summary());
    println!(
        "Writing and verifying took {:.2}s.",
        started.elapsed().as_secs_f64()
    );
    // A script that goes on after this must be able to tell that the scenes
    // were not verified, or that the original is still there.
    if !report.is_clean() {
        bail!("the scenes were written, but see the note above before relying on them");
    }
    Ok(())
}

/// Writes the scenes with their times and the names their files get to
/// `list_file`, in the format asked for or the one its name stands for.
fn save_scene_list(
    video: &Path,
    detection: &Detection,
    scenes: &ScenesToWrite,
    arguments: &Arguments,
    settings: &Settings,
    list_file: &Path,
) -> Result<()> {
    let format = arguments
        .list_format
        .or_else(|| SceneListFormat::for_file(list_file))
        .with_context(|| {
            format!(
                "the name {} does not say which format the scene list should have. \
                 End it in .csv, .txt or .edl, or pass --list-format",
                list_file.display()
            )
        })?;
    let frame_seconds = detection.video.seconds() / detection.frame_count.max(1) as f64;
    let frame_rate = FrameRate::from_frame_seconds(frame_seconds).map_err(|not_a_rate| {
        anyhow::anyhow!(
            "the frame rate of {} is not known: {not_a_rate}",
            video.display()
        )
    })?;
    let list = SceneList::of_split(video, scenes, frame_rate, settings.export.scene_name)?;
    let if_exists = if arguments.overwrite {
        ExistingListFile::Replace
    } else {
        ExistingListFile::Refuse
    };
    export::save_scene_list(&list, format, list_file, if_exists)
}

fn print_black_screens(black_screens: &[TimeSpan]) {
    println!("Black screens: {}", black_screens.len());
    for screen in black_screens {
        println!(
            "  {} to {} ({:.2}s)",
            format_timestamp(screen.start),
            format_timestamp(screen.end),
            screen.seconds()
        );
    }
}

fn print_scenes(scenes: &[TimeSpan]) {
    println!("Scenes: {}", scenes.len());
    for (number, scene) in scenes.iter().enumerate() {
        println!(
            "  {:03}  {} to {}",
            number + 1,
            format_timestamp(scene.start),
            format_timestamp(scene.end)
        );
    }
}
