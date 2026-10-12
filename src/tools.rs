//! Finds the two programs that do all the video work, ffmpeg and ffprobe,
//! and downloads them the first time the app runs on a machine without them.
//!
//! The download is one fixed release for each operating system, checked
//! against checksums built into this program, so what runs is the same
//! version everywhere.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::ffmpeg::{self, Program, TimeLimit, ToolCommand};
use crate::folders;

pub const FFMPEG_VERSION: &str = "6.1.1";
/// Static builds of ffmpeg and ffprobe for every platform, published as
/// release assets by the ffmpeg-static project. Each asset is one program,
/// gzipped.
const RELEASE_URL: &str = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1";

const APP_FOLDER: &str = "scene_splitter";
const TOOLS_FOLDER: &str = "tools";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the download may stall before it is given up.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_CHUNK_BYTES: usize = 256 * 1024;

/// Printing a version needs no video and no decoding, so a program that
/// takes longer than this to do it is not one that works.
const STARTUP_CHECK_LIMIT: TimeLimit = TimeLimit::WholeRun(Duration::from_secs(10));

/// One gzipped program to fetch, and what it must hash to.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ProgramDownload {
    program: Program,
    asset: &'static str,
    sha256: &'static str,
    bytes: u64,
}

const fn ffmpeg(asset: &'static str, sha256: &'static str, bytes: u64) -> ProgramDownload {
    ProgramDownload {
        program: Program::Ffmpeg,
        asset,
        sha256,
        bytes,
    }
}

const fn ffprobe(asset: &'static str, sha256: &'static str, bytes: u64) -> ProgramDownload {
    ProgramDownload {
        program: Program::Ffprobe,
        asset,
        sha256,
        bytes,
    }
}

/// The two downloads for an operating system and processor, as Rust names
/// them, or `None` where no build is published.
fn downloads_for(os: &str, architecture: &str) -> Option<[ProgramDownload; 2]> {
    let downloads = match (os, architecture) {
        ("windows", "x86_64") => [
            ffmpeg(
                "ffmpeg-win32-x64.gz",
                "8883a3dffbd0a16cf4ef95206ea05283f78908dbfb118f73c83f4951dcc06d77",
                29_581_307,
            ),
            ffprobe(
                "ffprobe-win32-x64.gz",
                "f309e6223ad89d2fe54bccd420a7709b66fd27540674e92309578ed491a43c8d",
                29_521_644,
            ),
        ],
        ("macos", "aarch64") => [
            ffmpeg(
                "ffmpeg-darwin-arm64.gz",
                "8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa",
                19_246_198,
            ),
            ffprobe(
                "ffprobe-darwin-arm64.gz",
                "d986a8ec7b030899fe66a8a288ed809a3543338705a3ce178cfb85869c5d80be",
                19_207_077,
            ),
        ],
        ("macos", "x86_64") => [
            ffmpeg(
                "ffmpeg-darwin-x64.gz",
                "929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106",
                25_296_431,
            ),
            ffprobe(
                "ffprobe-darwin-x64.gz",
                "d4da574d6e2e197bd259b47d69cf262df9e312af24ad960444f6d806d3d4c186",
                25_239_438,
            ),
        ],
        ("linux", "x86_64") => [
            ffmpeg(
                "ffmpeg-linux-x64.gz",
                "bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa",
                29_354_986,
            ),
            ffprobe(
                "ffprobe-linux-x64.gz",
                "25d9b6ccb05e3d9de9e04e31e2506d8dd7f9f0418981965ac6df12e8d3afd067",
                29_276_839,
            ),
        ],
        ("linux", "aarch64") => [
            ffmpeg(
                "ffmpeg-linux-arm64.gz",
                "754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0",
                25_568_691,
            ),
            ffprobe(
                "ffprobe-linux-arm64.gz",
                "2ab6aba60ee84412dff9188720703376cb4e7aaf7e0b5e43aa8249f2acae5bf8",
                25_493_573,
            ),
        ],
        _ => return None,
    };
    Some(downloads)
}

/// Where each program is, once known.
#[derive(Debug)]
struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

static TOOLS: OnceLock<Tools> = OnceLock::new();

/// A run of ffmpeg or ffprobe that ends by itself, with a time limit.
/// Before the tools have been located this runs whatever is on the PATH.
pub fn command(program: Program) -> ToolCommand {
    ToolCommand::at(program, located(program))
}

/// A bare command that runs ffmpeg, for the player. Its decoders run for as
/// long as a video plays, so it starts and stops them itself.
pub fn ffmpeg_command() -> Command {
    let mut command = Command::new(located(Program::Ffmpeg));
    ffmpeg::hide_console_window(&mut command);
    command
}

fn located(program: Program) -> PathBuf {
    let Some(tools) = TOOLS.get() else {
        return PathBuf::from(program.command_name());
    };
    match program {
        Program::Ffmpeg => tools.ffmpeg.clone(),
        Program::Ffprobe => tools.ffprobe.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Availability {
    Ready,
    NeedsDownload,
}

/// Looks for the programs, first in the app's own folder, then on the PATH.
/// The app's own copy comes first because it is the tested version.
///
/// A program only counts when it actually runs. A file cut short by a crash
/// or a power failure would otherwise be found on every start and fail
/// every scan, with no way back to the download.
pub fn find_installed() -> Availability {
    if TOOLS.get().is_some() {
        return Availability::Ready;
    }
    let own_copy = tools_dir().map(|folder| Tools {
        ffmpeg: folder.join(Program::Ffmpeg.file_name()),
        ffprobe: folder.join(Program::Ffprobe.file_name()),
    });
    let on_the_path = Tools {
        ffmpeg: PathBuf::from(Program::Ffmpeg.command_name()),
        ffprobe: PathBuf::from(Program::Ffprobe.command_name()),
    };
    let found = own_copy.into_iter().chain([on_the_path]).find(|tools| {
        runs(Program::Ffmpeg, &tools.ffmpeg) && runs(Program::Ffprobe, &tools.ffprobe)
    });
    match found {
        Some(tools) => {
            remember(tools);
            Availability::Ready
        }
        None => Availability::NeedsDownload,
    }
}

fn remember(tools: Tools) {
    // Two threads may find the tools at once. Both found the same thing, so
    // whichever is stored first is as good as the other.
    let _ = TOOLS.set(tools);
}

fn runs(program: Program, path: &Path) -> bool {
    ToolCommand::at(program, path)
        .arg("-version")
        .run(STARTUP_CHECK_LIMIT)
        .is_ok()
}

/// The app keeps its copy of the programs with its other per-user data.
/// `None` on a system with no per-user folder. A shared folder such as the
/// temporary one will not do, since another user could put a program there.
fn tools_dir() -> Option<PathBuf> {
    Some(
        folders::data_dir()?
            .join(APP_FOLDER)
            .join(TOOLS_FOLDER)
            .join(format!("ffmpeg-{FFMPEG_VERSION}")),
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DownloadProgress {
    pub received_bytes: u64,
    pub total_bytes: u64,
}

impl DownloadProgress {
    pub fn fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.received_bytes as f64 / self.total_bytes as f64).min(1.0) as f32
    }
}

/// The size of the download for this machine, or zero where there is none.
pub fn download_bytes() -> u64 {
    downloads_for(std::env::consts::OS, std::env::consts::ARCH).map_or(0, |downloads| {
        downloads.iter().map(|download| download.bytes).sum()
    })
}

/// Downloads ffmpeg and ffprobe into the app's own folder. Nothing is
/// retried here: the caller reports the failure and the person decides
/// whether to try again.
pub fn download(mut on_progress: impl FnMut(DownloadProgress)) -> Result<()> {
    let (os, architecture) = (std::env::consts::OS, std::env::consts::ARCH);
    let Some(downloads) = downloads_for(os, architecture) else {
        bail!(
            "ffmpeg and ffprobe were not found, and no download exists for {os} on {architecture}. \
             Install ffmpeg yourself and make sure it is on the PATH."
        );
    };
    let Some(folder) = tools_dir() else {
        bail!(
            "ffmpeg and ffprobe were not found, and this system has no per-user folder to download them into. \
             Install ffmpeg yourself and make sure it is on the PATH."
        );
    };
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("could not create {}", folder.display()))?;

    let total_bytes = downloads.iter().map(|download| download.bytes).sum();
    let mut received_before = 0;
    for download in &downloads {
        fetch_program(download, &folder, |received_bytes| {
            on_progress(DownloadProgress {
                received_bytes: received_before + received_bytes,
                total_bytes,
            });
        })?;
        received_before += download.bytes;
    }

    remember(Tools {
        ffmpeg: folder.join(Program::Ffmpeg.file_name()),
        ffprobe: folder.join(Program::Ffprobe.file_name()),
    });
    Ok(())
}

/// Downloads one program, checks it, and puts it in place ready to run.
fn fetch_program(
    download: &ProgramDownload,
    folder: &Path,
    on_received: impl FnMut(u64),
) -> Result<()> {
    let destination = folder.join(download.program.file_name());
    // Named after this process, so two copies of the app downloading at
    // once never write into each other's files.
    let archive_path = destination.with_extension(format!("gz.{}.part", std::process::id()));
    let outcome = download_archive(download, &archive_path, on_received)
        .and_then(|()| unpack_program(&archive_path, &destination));
    // The archive is only needed for unpacking. Failing to remove it wastes
    // disk space but breaks nothing.
    let _ = std::fs::remove_file(&archive_path);
    outcome
}

/// Fetches the gzipped program and makes sure it is the exact file expected.
fn download_archive(
    download: &ProgramDownload,
    archive_path: &Path,
    mut on_received: impl FnMut(u64),
) -> Result<()> {
    let name = download.program.command_name();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(STALL_TIMEOUT)
        .build();
    let response = agent
        .get(&format!("{RELEASE_URL}/{}", download.asset))
        .call()
        .with_context(|| format!("could not reach the {name} download"))?;

    let mut body = response.into_reader();
    let mut archive = File::create(archive_path)
        .with_context(|| format!("could not create {}", archive_path.display()))?;
    let mut checksum = Sha256::new();
    let mut chunk = vec![0u8; DOWNLOAD_CHUNK_BYTES];
    let mut received_bytes = 0u64;
    loop {
        let read = body
            .read(&mut chunk)
            .with_context(|| format!("the {name} download was interrupted"))?;
        if read == 0 {
            break;
        }
        archive
            .write_all(&chunk[..read])
            .with_context(|| format!("could not write {}", archive_path.display()))?;
        checksum.update(&chunk[..read]);
        received_bytes += read as u64;
        on_received(received_bytes);
    }

    if hex(&checksum.finalize()) != download.sha256 {
        bail!("the {name} download does not match its expected checksum, so it was discarded");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Unzips the program under a temporary name, then moves it into place, so
/// a half-written program is never found by a later run.
fn unpack_program(archive_path: &Path, destination: &Path) -> Result<()> {
    let unfinished = destination.with_extension(format!("{}.part", std::process::id()));
    let outcome = write_unpacked(archive_path, &unfinished)
        .and_then(|()| allow_running(&unfinished))
        .and_then(|()| move_into_place(&unfinished, destination));
    if outcome.is_err() {
        // Nothing reads this file again, so leaving it would only waste space.
        let _ = std::fs::remove_file(&unfinished);
    }
    outcome
}

fn write_unpacked(archive_path: &Path, unfinished: &Path) -> Result<()> {
    let archive = File::open(archive_path)
        .with_context(|| format!("could not open {}", archive_path.display()))?;
    let mut output = File::create(unfinished)
        .with_context(|| format!("could not create {}", unfinished.display()))?;
    std::io::copy(&mut GzDecoder::new(archive), &mut output)
        .with_context(|| format!("could not unpack {}", unfinished.display()))?;
    // On disk before it gets its real name, so a power failure cannot leave
    // a program that is there but empty.
    output
        .sync_all()
        .with_context(|| format!("could not finish writing {}", unfinished.display()))
}

fn move_into_place(unfinished: &Path, destination: &Path) -> Result<()> {
    match std::fs::rename(unfinished, destination) {
        Ok(()) => Ok(()),
        // Another copy of the app got there first and may be running the
        // program already, which stops Windows replacing it. Its copy
        // passed the same checksum, so it is as good as this one.
        Err(_) if destination.is_file() => {
            let _ = std::fs::remove_file(unfinished);
            Ok(())
        }
        Err(error) => {
            Err(error).with_context(|| format!("could not write {}", destination.display()))
        }
    }
}

/// Outside Windows a downloaded file is not a program until it is marked as
/// one.
#[cfg(unix)]
fn allow_running(program: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    const OWNER_ALL_OTHERS_READ_AND_RUN: u32 = 0o755;
    let permissions = std::fs::Permissions::from_mode(OWNER_ALL_OTHERS_READ_AND_RUN);
    std::fs::set_permissions(program, permissions)
        .with_context(|| format!("could not make {} runnable", program.display()))
}

#[cfg(not(unix))]
fn allow_running(_program: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED: [(&str, &str); 5] = [
        ("windows", "x86_64"),
        ("macos", "aarch64"),
        ("macos", "x86_64"),
        ("linux", "x86_64"),
        ("linux", "aarch64"),
    ];

    fn is_sha256(text: &str) -> bool {
        text.len() == 64
            && text
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    }

    #[test]
    fn every_supported_platform_has_both_programs() {
        for (os, architecture) in SUPPORTED {
            let downloads = downloads_for(os, architecture).unwrap();
            let programs: Vec<Program> = downloads.iter().map(|d| d.program).collect();
            assert_eq!(programs, vec![Program::Ffmpeg, Program::Ffprobe]);
        }
    }

    #[test]
    fn every_download_names_its_program_and_has_a_real_checksum() {
        for (os, architecture) in SUPPORTED {
            for download in downloads_for(os, architecture).unwrap() {
                assert!(download.asset.starts_with(download.program.command_name()));
                assert!(download.asset.ends_with(".gz"));
                assert!(is_sha256(download.sha256), "{}", download.asset);
                assert!(download.bytes > 1_000_000, "{}", download.asset);
            }
        }
    }

    #[test]
    fn no_two_downloads_share_a_checksum() {
        let mut checksums: Vec<&str> = SUPPORTED
            .iter()
            .flat_map(|(os, architecture)| downloads_for(os, architecture).unwrap())
            .map(|download| download.sha256)
            .collect();
        let count = checksums.len();
        checksums.sort_unstable();
        checksums.dedup();
        assert_eq!(checksums.len(), count);
    }

    #[test]
    fn an_unsupported_platform_has_no_download() {
        assert_eq!(downloads_for("freebsd", "x86_64"), None);
        assert_eq!(downloads_for("windows", "aarch64"), None);
    }

    #[test]
    fn this_machine_is_a_supported_platform() {
        let here = downloads_for(std::env::consts::OS, std::env::consts::ARCH);
        assert!(here.is_some());
    }

    #[test]
    fn a_checksum_is_written_as_lowercase_hex() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn progress_is_a_share_of_the_whole_download() {
        let a_quarter = DownloadProgress {
            received_bytes: 50,
            total_bytes: 200,
        };
        assert_eq!(a_quarter.fraction(), 0.25);

        let unknown_size = DownloadProgress {
            received_bytes: 50,
            total_bytes: 0,
        };
        assert_eq!(unknown_size.fraction(), 0.0);
    }

    #[test]
    fn a_gzipped_program_unpacks_to_its_original_bytes() {
        use flate2::write::GzEncoder;

        let folder =
            std::env::temp_dir().join(format!("scene_splitter_tools_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let archive_path = folder.join("program.gz.part");
        let destination = folder.join("program");

        let mut encoder = GzEncoder::new(
            File::create(&archive_path).unwrap(),
            flate2::Compression::fast(),
        );
        encoder.write_all(b"not really a program").unwrap();
        encoder.finish().unwrap();

        unpack_program(&archive_path, &destination).unwrap();
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"not really a program"
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
