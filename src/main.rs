//! Splits a video into scenes at stretches of black screen.
//!
//! Run without arguments to open the window and drag videos in. Run with a
//! video to split it straight from the command line, or with `--scan-only`
//! to scan videos ahead of time and review them in the window later.

// Windows opens a console window for every console program it starts, so a
// double click in Explorer would show an empty black window behind the app.
// Built as a window program, it gets none, and `main` connects it to the
// terminal it was started from so that the command line still prints. The
// tests stay a console program, or their results would not show.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod app;
mod batch;
mod black;
mod cache;
mod chapters;
mod detect;
#[cfg(test)]
mod e2e_tests;
mod exact_cut;
#[cfg(test)]
mod exact_e2e_tests;
mod exact_fallback;
mod exact_split;
mod exact_verify;
mod export;
mod fates;
mod ffmpeg;
mod focus;
mod folder_settings;
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

use chapters::ChapterSplits;
use detect::{Detection, ScanDepth, ScanKind, ScanMode};
use exact_cut::CutPrecision;
use exact_fallback::NoWayToCut;
use exact_split::ExactCutError;
use export::{ExistingListFile, OriginalPolicy, SplitMethod, SplitOrder};
use fates::ScenesToWrite;
use folder_settings::{DetectionInUse, SettingsScope};
use hardcut::HardCutSettings;
use scene_list::{FrameRate, SceneList, SceneListFormat};
use scene_name::{EarlierNaming, EarlierSplit, SceneNamePattern};
use scenes::CutTimes;
use settings::Settings;
use timecode::{format_timestamp, TimeSpan};

/// How often the download of ffmpeg reports its progress on the console.
const PROGRESS_STEP_PERCENT: u32 = 10;

/// Detection and output options left out here come from the saved settings,
/// which the window edits: those remembered for the video's folder when
/// there are some, the general ones otherwise.
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
    /// "{name}_scene_{number}_of_{count}", which names the first of five
    /// scenes "<video name>_scene_1_of_5".
    #[arg(long, requires = "video", value_name = "PATTERN")]
    scene_name: Option<SceneNamePattern>,

    /// Write the list of scenes to this file: their numbers, times, the
    /// names their files get and their chapter titles. Works with
    /// --list-only, which then writes the list and no scene files.
    #[arg(long, requires = "video", value_name = "FILE")]
    save_list: Option<PathBuf>,

    /// The format of the scene list: a table (csv), chapter marks to add
    /// to a video with ffmpeg (chapters) or a CMX3600 edit list (edl). Left
    /// out, it is taken from the ending of the file's name: .csv, .txt (or
    /// .ffmetadata) or .edl.
    #[arg(long, value_enum, requires = "save_list", value_name = "FORMAT")]
    list_format: Option<SceneListFormat>,

    /// Keep the original video, whatever the saved setting says. Unless
    /// kept, the original is moved to the Recycle Bin (the Trash on macOS
    /// and Linux), but only after checking that the scene files hold every
    /// packet of it, unchanged.
    #[arg(long, requires = "video", conflicts_with = "list_only")]
    keep_original: bool,

    /// Cut on the exact picture where each scene changes, and not on the
    /// nearest point where the video can be cut as it is. A few seconds
    /// around each cut are encoded again, so this is slower. The original
    /// video is always kept. A video that cannot be cut this way is cut at
    /// the nearest points where it can be cut instead. The run then says
    /// so, lists how far each split moved, and ends with an error code.
    #[arg(long, requires = "video", conflicts_with_all = ["scan_only", "review"])]
    exact_cuts: bool,

    /// Also look for hard cuts: scene changes with no black screen. This
    /// decodes every frame, so it takes far longer than the normal scan.
    #[arg(long, requires = "video")]
    hard_cuts: bool,

    /// Do not split at the chapter marks of the file. Without this, a video
    /// with chapters is also split where each chapter begins.
    #[arg(long, requires = "video", conflicts_with_all = ["review", "scan_only"])]
    no_chapters: bool,

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
    // First, so that the help text and a refused argument are printed too.
    print_to_the_starting_terminal();
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
        || arguments.save_list.is_some()
        || arguments.no_chapters;
    if path.is_dir() && !arguments.scan_only && for_one_video_only {
        bail!(
            "{} is a folder. --list-only, --out-dir, --overwrite, --save-list and --no-chapters \
             work on a single video. Use --scan-only to scan a folder, or give the folder alone \
             to open it in the window.",
            path.display()
        );
    }

    let detection = detection_at(&loaded.settings, path);
    // The window gets the tools itself, with a progress bar.
    if !arguments.scan_only && (arguments.review || path.is_dir()) {
        refuse_overrides_for_the_window(&arguments)?;
        let first_scan = scan_kind(&arguments, detection.detection.hard_cuts);
        return app::run(loaded, Some(path.clone()), first_scan);
    }
    if let SettingsScope::Folder(folder) = &detection.scope {
        println!(
            "Using the scan settings remembered for {}.",
            folder.display()
        );
    }
    let settings = settings_for_this_run(loaded.settings, &detection, &arguments)?;

    get_tools_if_missing()?;
    if arguments.scan_only {
        return scan_ahead(path, &arguments, &settings);
    }
    split_from_command_line(path, &arguments, &settings)
}

/// Connects the program to the terminal that started it. A window program
/// on Windows starts with no console, so without this nothing it prints
/// would be seen. Output sent to a file or a pipe needs no console and is
/// not affected.
#[cfg(windows)]
fn print_to_the_starting_terminal() {
    /// Stands for "the process that started this one" in `AttachConsole`.
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }

    // SAFETY: the call takes one number and no pointers.
    // It fails when there is no terminal to connect to, which is the normal
    // case for a start from Explorer. There is then nobody to print to, so
    // the result is not looked at.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Elsewhere a program started from a terminal already prints to it.
#[cfg(not(windows))]
fn print_to_the_starting_terminal() {}

/// Downloads ffmpeg and ffprobe on a machine that has neither, printing
/// progress as it goes.
fn get_tools_if_missing() -> Result<()> {
    if tools::find_installed() == tools::Availability::Ready {
        return Ok(());
    }
    println!("ffmpeg was not found. Downloading ffmpeg (this happens once).");
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
        || arguments.scene_name.is_some()
        || arguments.exact_cuts;
    if overrides_given {
        bail!(
            "--scan, --min-black, --max-luma, --black-fraction, --keep-original, --scene-name \
             and --exact-cuts apply to a command line run. For the window, change them in its \
             Settings section."
        );
    }
    Ok(())
}

/// The detection settings saved for what the command line names: a video,
/// or a folder whose videos are all directly inside it and so share its
/// settings.
fn detection_at(settings: &Settings, video_or_folder: &Path) -> DetectionInUse {
    if !video_or_folder.is_dir() {
        return settings.detection_for(video_or_folder);
    }
    match std::path::absolute(video_or_folder) {
        Ok(folder) => settings.detection_for_folder(&folder),
        // A folder that cannot be written out in full matches nothing that
        // was remembered, which leaves the general settings.
        Err(_) => settings.detection_for_folder(video_or_folder),
    }
}

/// The settings of one command line run: the saved ones, with the detection
/// settings in force for the video or folder, and on top of those whatever
/// the command line overrides.
fn settings_for_this_run(
    mut settings: Settings,
    saved_detection: &DetectionInUse,
    arguments: &Arguments,
) -> Result<Settings> {
    settings.detection = saved_detection.detection;
    if let Some(mode) = arguments.scan {
        settings.detection.scan.mode = mode;
    }
    if let Some(min_black) = arguments.min_black {
        settings.detection.scan.criteria.min_seconds = min_black;
    }
    if let Some(max_luma) = arguments.max_luma {
        settings.detection.scan.criteria.max_dark_luma = max_luma;
    }
    if let Some(black_fraction) = arguments.black_fraction {
        settings.detection.scan.criteria.min_dark_fraction = black_fraction;
    }
    if arguments.keep_original {
        settings.export.original_policy = OriginalPolicy::Keep;
    }
    if let Some(scene_name) = arguments.scene_name {
        settings.export.scene_name = scene_name;
    }
    if arguments.exact_cuts {
        settings.export.cut_precision = CutPrecision::ExactFrame;
    }
    settings
        .validate()
        .context("the options given cannot be used")?;
    Ok(settings)
}

/// The scan the arguments ask for. A hard-cut scan thins its cuts with the
/// saved settings given.
fn scan_kind(arguments: &Arguments, hard_cuts: HardCutSettings) -> ScanKind {
    if arguments.hard_cuts {
        ScanKind::HardCuts(hard_cuts)
    } else {
        ScanKind::BlackScreens
    }
}

fn scan_depth(arguments: &Arguments, settings: &Settings) -> ScanDepth {
    scan_kind(arguments, settings.detection.hard_cuts).depth()
}

fn scan_ahead(path: &Path, arguments: &Arguments, settings: &Settings) -> Result<()> {
    let depth = scan_depth(arguments, settings);
    let summary = prescan::scan_ahead(
        path,
        settings.scene_names(),
        &settings.detection.scan,
        depth,
        |video_done| {
            println!("{video_done}");
            for stretch in &video_done.undecoded {
                eprintln!("Warning: in {}, {stretch}", video_done.video.display());
            }
        },
    )?;
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
        &settings.detection.scan,
        scan_depth(arguments, settings),
        &detect::no_scan_progress,
    )?;
    let detection_seconds = started.elapsed().as_secs_f64();
    for stretch in &scan.undecoded {
        eprintln!("Warning: in {}, {stretch}", video.display());
    }
    let detection = scan.detection;
    let method = settings.export.split_method();
    let scan_kind = scan_kind(arguments, settings.detection.hard_cuts);
    let detected_cuts = match settings.export.cut_precision {
        CutPrecision::NearestKeyframe => detection.cuts_for(scan_kind),
        // Nobody reviews a split from the command line, so the cuts go
        // where the scan saw the scene change.
        CutPrecision::ExactFrame => exact_cut::exact_cut_times(&detection, scan_kind),
    };
    let chapter_splits = if arguments.no_chapters {
        ChapterSplits::default()
    } else {
        detection.chapter_splits(&detected_cuts)
    };
    let every_cut = detected_cuts
        .iter()
        .chain(&chapter_splits.cuts)
        .copied()
        .collect();
    // Checked once, here, so that neither the listing nor the split can be
    // handed a cut that is not inside the video.
    let cuts = CutTimes::inside(detection.video, every_cut).map_err(|outside| {
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
            detected_cuts.len().saturating_sub(detection.cuts.len())
        );
    }
    let chapter_mark_count = detection.chapter_marks().len();
    if chapter_mark_count > 0 && arguments.no_chapters {
        println!("Chapter marks: {chapter_mark_count}, left out as asked.");
    } else if chapter_mark_count > 0 {
        println!(
            "Chapter marks: {chapter_mark_count}. Cuts they add: {}.",
            chapter_splits.cuts.len()
        );
    }
    print_scenes(&cuts.scenes(), &chapter_splits);
    println!("Detection took {detection_seconds:.2}s.");

    // The command line has no review, so it has no way to drop a scene.
    let scenes = ScenesToWrite::every_scene(cuts);
    if let Some(list_file) = &arguments.save_list {
        let listed = ScenesToList {
            video,
            detection: &detection,
            scenes: &scenes,
            chapter_splits: &chapter_splits,
        };
        save_scene_list(&listed, arguments, settings, list_file)?;
        println!("Saved the scene list to {}.", list_file.display());
    }

    if arguments.list_only {
        return Ok(());
    }
    if scenes.cuts().is_empty() {
        println!("No scene changes found, nothing to split.");
        return Ok(());
    }

    // Splitting replaces the scene files of earlier splits and removes
    // those it has no use for, so it only goes ahead over them when the
    // user asked for that.
    let scene_names = settings.scene_names();
    let earlier_splits = split::earlier_splits_in(video, &out_dir, scene_names)?;
    if !arguments.overwrite && !earlier_splits.is_empty() {
        bail!(earlier_scenes_in_the_way(&earlier_splits, &out_dir));
    }
    if !earlier_splits.is_empty() {
        println!(
            "{}",
            earlier_scenes_being_replaced(&earlier_splits, &out_dir)
        );
    }

    let started = Instant::now();
    let order = SplitOrder::new(scenes, method);
    if method == SplitMethod::ExactFrames {
        println!("{}", notice_before_an_exact_cut(video));
    }
    let report = export::export_scenes(video, &order, &out_dir, scene_names)
        .map_err(with_the_way_back_to_a_normal_cut)?;
    // So that these scenes are still known as scenes of this video after
    // the name for scene files is changed. The split itself is done, so a
    // settings file that cannot be written is told and not a failure.
    if let Err(error) = settings::remember_scene_name_used_for_a_split(scene_names.in_use()) {
        eprintln!(
            "Warning: the name of these scene files could not be noted in the settings: {error:#}"
        );
    }
    println!("{}", report.summary());
    if let Some(measurements) = report.exact_cut_measurements() {
        println!("{measurements}");
    }
    if let Some(moved_splits) = report.fallback_measurements() {
        println!("{moved_splits}");
    }
    println!(
        "Writing and verifying took {:.2}s.",
        started.elapsed().as_secs_f64()
    );
    // A script that goes on after this must be able to tell that the scenes
    // were not verified, that the original is still there, or that the video
    // was not cut on the exact pictures it asked for.
    if !report.is_clean() {
        bail!("the scenes were written, but see the note above before relying on them");
    }
    Ok(())
}

/// What a scene list is made from: the video, its scan, the scenes a split
/// would write, and the titles its chapter marks give to scenes.
#[derive(Clone, Copy)]
struct ScenesToList<'run> {
    video: &'run Path,
    detection: &'run Detection,
    scenes: &'run ScenesToWrite,
    chapter_splits: &'run ChapterSplits,
}

/// Writes the scenes with their times and the names their files get to
/// `list_file`, in the format asked for or the one its name stands for.
fn save_scene_list(
    listed: &ScenesToList,
    arguments: &Arguments,
    settings: &Settings,
    list_file: &Path,
) -> Result<()> {
    let ScenesToList {
        video,
        detection,
        scenes,
        chapter_splits,
    } = *listed;
    let format = arguments
        .list_format
        .or_else(|| SceneListFormat::for_file(list_file))
        .with_context(|| {
            format!(
                "the name {} does not say which format the scene list should have. \
                 End it in .csv, .txt (or .ffmetadata) or .edl, or pass --list-format",
                list_file.display()
            )
        })?;
    let frame_seconds = detection.frame_seconds();
    let frame_rate = FrameRate::from_frame_seconds(frame_seconds).map_err(|not_a_rate| {
        anyhow::anyhow!(
            "the frame rate of {} is not known: {not_a_rate}",
            video.display()
        )
    })?;
    let list = SceneList::of_split(video, scenes, frame_rate, settings.export.scene_name)?
        .with_chapter_titles(|scene| {
            chapter_splits
                .title_of_scene_starting_at(scene.start)
                .map(str::to_owned)
        });
    let if_exists = if arguments.overwrite {
        ExistingListFile::Replace
    } else {
        ExistingListFile::Refuse
    };
    export::save_scene_list(&list, format, list_file, if_exists)
}

/// Why the split does not start over the scene files of earlier ones.
fn earlier_scenes_in_the_way(earlier_splits: &[EarlierSplit], out_dir: &Path) -> String {
    format!(
        "{}\nPass --overwrite to replace them, or choose another --out-dir. \
         With --overwrite, those the new split has no use for go to the {}.",
        earlier_scenes_listed(earlier_splits, out_dir),
        export::TRASH_NAME
    )
}

/// What a split with --overwrite says before it starts: nobody is asked,
/// so the files that are about to go are at least named.
fn earlier_scenes_being_replaced(earlier_splits: &[EarlierSplit], out_dir: &Path) -> String {
    format!(
        "{}\nThey are replaced, as --overwrite says. Those the new split has no use for go to \
         the {}.",
        earlier_scenes_listed(earlier_splits, out_dir),
        export::TRASH_NAME
    )
}

/// The scene files of earlier splits, every one named, split by split. A
/// file is taken for an earlier scene by its name alone, and the person
/// should see each one that --overwrite replaces or moves to the Recycle
/// Bin or Trash. A split whose files are named another way than scenes are
/// named now is said to be so, since none of its files is written over and
/// all of them go.
fn earlier_scenes_listed(earlier_splits: &[EarlierSplit], out_dir: &Path) -> String {
    let count = match split::files_of(earlier_splits).len() {
        1 => "1 scene file from an earlier split is".to_owned(),
        several => format!("{several} scene files from an earlier split are"),
    };
    let mut lines = vec![format!("{count} in {}:", out_dir.display())];
    for earlier_split in earlier_splits {
        if earlier_split.naming() == EarlierNaming::AnotherWay {
            lines.push(NAMED_ANOTHER_WAY_HEADING.to_owned());
        }
        lines.extend(
            earlier_split
                .file_names()
                .into_iter()
                .map(|name| format!("  {name}")),
        );
    }
    lines.join("\n")
}

/// Stands above the files of an earlier split in that list, when they are
/// not named as scenes are named now.
const NAMED_ANOTHER_WAY_HEADING: &str = "Named another way than scenes are named now:";

/// Said before an exact split starts, since it takes longer than a copy and
/// does not do what the saved setting for originals may say.
const EXACT_CUT_NOTICE: &str =
    "Cutting on the exact pictures. A few seconds around each cut are encoded again, \
     and the original is kept.";

/// What is said before a split that asked for exact pictures starts. A
/// video that cannot be cut that way is known by a look at its file, and
/// then the notice says how it will be cut instead.
fn notice_before_an_exact_cut(video: &Path) -> String {
    match exact_split::why_no_exact_cut_at_a_glance(video) {
        Ok(Some(why_not_exact)) => why_not_exact.before_the_split_in_plain_words(),
        // A video that could not be looked at fails in the split itself,
        // which says why.
        Ok(None) | Err(_) => EXACT_CUT_NOTICE.to_owned(),
    }
}

/// A failed exact cut is told with what the person can do instead. A video
/// that can be cut neither way says so itself.
fn with_the_way_back_to_a_normal_cut(error: anyhow::Error) -> anyhow::Error {
    if let Some(no_way) = error.downcast_ref::<NoWayToCut>() {
        let in_plain_words = no_way.in_plain_words();
        return error.context(in_plain_words);
    }
    if error.downcast_ref::<ExactCutError>().is_none() {
        return error;
    }
    error.context(
        "the exact cut did not work. The original was not changed. \
         Run again without --exact-cuts to split the normal way",
    )
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

/// Lists the scenes, each with the title of the chapter that begins where
/// it begins, if the file names one.
fn print_scenes(scenes: &[TimeSpan], chapter_splits: &ChapterSplits) {
    println!("Scenes: {}", scenes.len());
    for (number, scene) in scenes.iter().enumerate() {
        let title = chapter_splits
            .title_of_scene_starting_at(scene.start)
            .map(|title| format!("  {title}"))
            .unwrap_or_default();
        println!(
            "  {:03}  {} to {}{title}",
            number + 1,
            format_timestamp(scene.start),
            format_timestamp(scene.end)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_to_replace_earlier_scenes_names_every_one_of_them() {
        let out_dir = Path::new("videos");
        let two = scene_name::earlier_splits_for_test(
            "Rocky.mp4",
            &["Rocky_scene_1_of_2.mp4", "Rocky_scene_2_of_2.mp4"],
        );
        let refusal = earlier_scenes_in_the_way(&two, out_dir);
        assert!(
            refusal.starts_with("2 scene files from an earlier split are in videos:"),
            "{refusal}"
        );
        assert!(
            refusal.contains("\n  Rocky_scene_1_of_2.mp4\n  Rocky_scene_2_of_2.mp4\n"),
            "{refusal}"
        );
        assert!(!refusal.contains(NAMED_ANOTHER_WAY_HEADING), "{refusal}");
        assert!(refusal.contains("--overwrite"), "{refusal}");
        assert!(refusal.contains(export::TRASH_NAME), "{refusal}");

        let one = scene_name::earlier_splits_for_test("Rocky.mp4", &["Rocky_scene_2_of_3.mp4"]);
        let one = earlier_scenes_in_the_way(&one, out_dir);
        assert!(
            one.starts_with("1 scene file from an earlier split is in videos:"),
            "{one}"
        );
    }

    #[test]
    fn the_refusal_says_which_earlier_scenes_are_named_another_way() {
        let out_dir = Path::new("videos");
        let found = scene_name::earlier_splits_for_test(
            "Rocky.mp4",
            &[
                "Rocky_scene_001.mp4",
                "Rocky_scene_002.mp4",
                "Rocky_scene_1_of_2.mp4",
            ],
        );
        let refusal = earlier_scenes_in_the_way(&found, out_dir);
        assert!(
            refusal.starts_with("3 scene files from an earlier split are in videos:"),
            "{refusal}"
        );
        assert!(
            refusal.contains(&format!(
                "\n  Rocky_scene_1_of_2.mp4\n{NAMED_ANOTHER_WAY_HEADING}\n  \
                 Rocky_scene_001.mp4\n  Rocky_scene_002.mp4\n"
            )),
            "{refusal}"
        );

        // With --overwrite nobody is asked, so the same files are named.
        let notice = earlier_scenes_being_replaced(&found, out_dir);
        assert!(
            notice.starts_with(&earlier_scenes_listed(&found, out_dir)),
            "{notice}"
        );
        assert!(notice.contains(export::TRASH_NAME), "{notice}");
    }

    #[test]
    fn the_help_for_scene_names_shows_the_default() {
        use clap::CommandFactory;
        let help = Arguments::command().render_long_help().to_string();
        assert!(
            help.contains(scene_name::DEFAULT_PATTERN),
            "the help does not name {}",
            scene_name::DEFAULT_PATTERN
        );
    }
}
