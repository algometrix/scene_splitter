# Scene Splitter

[![CI](https://github.com/algometrix/scene_splitter/actions/workflows/ci.yml/badge.svg)](https://github.com/algometrix/scene_splitter/actions/workflows/ci.yml)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-4F46C8)
![Rust](https://img.shields.io/badge/rust-1.88%2B-FF7A50?logo=rust&logoColor=white)
![ffmpeg](https://img.shields.io/badge/ffmpeg-downloaded%20on%20first%20run-1FA97C)
![No re-encoding](https://img.shields.io/badge/splits-without%20re--encoding-2E90D9)

Scene Splitter cuts a long video into its scenes. It finds the stretches of black screen between scenes, shows the proposed scenes in a review window, and writes one file per scene. It copies the compressed streams and re-encodes nothing, so a split takes seconds and loses no quality.

![The review screen: a timeline and one colour-coded card per scene](docs/review.png)

It is a desktop app and a command line tool in one program, for Windows, macOS and Linux, written in Rust with egui/eframe. It runs `ffmpeg` and `ffprobe` for all video work, and downloads them itself the first time it needs them.

## What it does

- **Fast scan.** A two hour 480p video is scanned in about two seconds, because black frames show up in the packet sizes before anything is decoded.
- **Hard cuts.** An optional slower scan finds scene changes that have no black screen.
- **Review before writing.** Drag one scene onto another to merge them. Undo, reset, add a split by time, or move a strip back out of a merged scene.
- **Built-in player.** Click a preview to play from there, step frame by frame with the mouse wheel, and split the scene at the current position.
- **Safe by default.** After a split, every packet of the scene files is checked against the original. Only then is the original moved to the Recycle Bin or Trash.
- **Scan ahead.** Scan a whole folder from the command line, and review the results in the window later.
- **Nothing to install.** ffmpeg is fetched on first run and checked against a built-in checksum.

![The player, with frame stepping and a split button](docs/player.png)

## Requirements

- A Rust toolchain (stable, with Cargo).
- Nothing else to install. If `ffmpeg` and `ffprobe` are not on your `PATH`, the app downloads them on first run (see "ffmpeg" below).
- Windows, macOS or Linux. See "Platforms" below for what has been tested.
- On Linux, a few packages to build it: `sudo apt install pkg-config libasound2-dev libxkbcommon-dev libwayland-dev` (Debian and Ubuntu names).

## Build and run

```
cargo build --release
```

The program is written to `target/release/scene_splitter` (`target\release\scene_splitter.exe` on Windows).

```
target/release/scene_splitter
```

With no arguments it opens the window. You can also run it through Cargo: `cargo run --release -- <arguments>`.

## ffmpeg

The app needs `ffmpeg` and `ffprobe`. It looks for them in this order:

1. Its own copy, in the `scene_splitter/tools/ffmpeg-6.1.1` folder of your per-user data folder (see the table below).
2. Your `PATH`.
3. If neither has them, it downloads ffmpeg 6.1.1 (two files, about 60 MB together, once) into the folder from step 1. The window shows a progress bar and the command line prints the progress. You can queue videos while it downloads.

| System | Per-user data folder | Settings folder |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%` | `%APPDATA%` |
| macOS | `~/Library/Application Support` | `~/Library/Application Support` |
| Linux | `$XDG_DATA_HOME` or `~/.local/share` | `$XDG_CONFIG_HOME` or `~/.config` |

The downloads are the static builds published in the GitHub releases of the `ffmpeg-static` project, one for each of: Windows x64, macOS on Apple Silicon and on Intel, Linux x64 and Linux ARM64. The app checks each file against a SHA-256 checksum built into the program and discards it on a mismatch. On any other system, or to use your own ffmpeg, put `ffmpeg` and `ffprobe` on your `PATH`.

ffmpeg is a separate project with its own license (these builds are GPL). It is not part of this repository.

## Platforms

The code is written for Windows, macOS and Linux, and a GitHub Actions workflow lints and tests it on all three, end-to-end tests included. The window itself has only been used by hand on Windows so far. On macOS and Linux, treat the window, the player's sound and the move to the Trash as untested until you have tried them.

What differs between systems:

- The original goes to the Recycle Bin on Windows and to the Trash on macOS and Linux.
- The window borrows a system font: Segoe UI on Windows, Arial on macOS, DejaVu Sans on Linux. Without it, the toolkit's built-in font is used.
- macOS may refuse to run a downloaded program it has not seen before. If ffmpeg fails to start there, install it with Homebrew (`brew install ffmpeg`) so it is on your `PATH`.

## Using the window

1. Drop videos or a folder onto the window, or paste a path into the box on the left and press Add. A folder queues every video directly inside it. Scene files from an earlier split are left out.
2. The app scans the queued videos in the background, one at a time. The queue on the left shows the state of each video.
3. When a scan finishes, the video is ready for review. Each scene is a colour-coded card with preview pictures. A timeline above the cards shows the whole video with one block per scene. Click a block to jump to that scene's card.
4. Edit the plan:
   - Drag a scene card onto another to merge them and everything between.
   - Click, Ctrl-click or Shift-click cards to select several, then press M or the merge button.
   - "Split selected apart" takes a merged scene apart again.
   - "Add a split at" takes a time such as `1:03:12`.
   - Undo (Ctrl+Z) takes back the last change. Reset returns to the scenes as detected.
5. If the video has scene changes with no black screen, press "Find hard cuts". This scan is much slower. You can set its sensitivity and the shortest scene it may produce.
6. Press "Split into N files". If scene files from an earlier split are in the way, the app asks before it replaces them. "Skip this video" writes nothing.

"Scan again" throws away the stored scan of the video and scans it afresh.

### Review keys

| Key | Action |
| --- | --- |
| M | Merge the selected scenes |
| Ctrl+A | Select every scene |
| Ctrl+Z | Undo the last change |
| Esc | Clear the selection |

### The player

Click a preview picture to play the video from that moment. The player starts muted, and asks before it turns the sound on.

| Key or control | Action |
| --- | --- |
| A or Left arrow | Go back 10 seconds |
| D or Right arrow | Go forward 10 seconds |
| Space | Pause or play |
| S | Split the scene at the current position |
| Mouse wheel over the picture | Step one frame (down is forward) |
| 1x, 2x, 4x buttons | Playback speed |
| Esc | Close the player |

A split made in the player lands on the keyframe nearest to the current position. The player shows where that keyframe is before you split.

## Command line

| Command | What it does |
| --- | --- |
| `scene_splitter` | Opens the window. |
| `scene_splitter <folder>` | Opens the window with every video in the folder queued. |
| `scene_splitter <video>` | Scans the video and splits it at once, with no review. |
| `scene_splitter <video> --review` | Opens that video in the window instead of splitting it at once. |
| `scene_splitter <video> --list-only` | Prints the black screens and scenes. Writes no scene files. |
| `scene_splitter <video or folder> --scan-only` | Scans the video, or every video directly inside the folder, and stores the results in the cache. Writes nothing else. |

Options:

| Option | Meaning |
| --- | --- |
| `--out-dir <folder>` | Where to write the scene files. The default is the video's own folder. |
| `--overwrite` | Replace scene files left in the output folder by an earlier run. Without it, the split stops if such files exist. |
| `--keep-original` | Keep the original video. By default it goes to the Recycle Bin or Trash after the scenes are verified. |
| `--hard-cuts` | Also look for hard cuts (scene changes with no black screen). This decodes every frame and is much slower. |
| `--scan suspicious` | Decode only where packet sizes hint at a black screen. This is the default. |
| `--scan every-frame` | Decode every frame. Use it when the black screens are noisy or grainy. |
| `--min-black <seconds>` | Shortest black screen that counts as a scene change. Default 0.25. |
| `--max-luma <0 to 255>` | Brightest pixel value that still counts as black. Default 32. |
| `--black-fraction <0 to 1>` | Share of a frame's pixels that must be black. Default 0.98. |

Command line options override the saved settings for that run.

### Scanning ahead of time

`--scan-only` lets you scan a batch of videos without the window open, for example overnight:

```
scene_splitter D:\videos --scan-only
scene_splitter D:\videos --scan-only --hard-cuts
```

The second form also runs the slow hard-cut pass. When you later open the same videos in the window, they are ready for review at once.

## How it works

**Packet-size prefilter.** A black frame compresses to almost nothing (about 25 bytes, against several kilobytes for a normal frame). `ffprobe` reads the packet table without decoding, which takes about a second even for a long file. The app then decodes only the stretches with tiny packets, as small grey thumbnails, to confirm which frames are really black. This is why the normal scan is fast. Grainy or noisy black screens do not compress this small, so `--scan every-frame` (or the thorough scan setting) decodes the whole video instead.

**Hard cuts.** The optional hard-cut scan decodes every frame and compares it with the frame before. A hard cut is a single frame that differs sharply from its predecessor while its neighbours barely change. Films cut between camera angles every few seconds, so the app keeps the strongest changes and drops the rest until every scene is at least as long as the shortest scene you set.

**Keyframe cuts.** The split copies the compressed streams, and a copy can only start a file on a keyframe, because every other frame depends on the frames before it. Each cut goes on the keyframe nearest to its black screen. A keyframe inside the black screen gives a clean cut. Otherwise the cut lands slightly before or after it, and one scene keeps a moment of its neighbour.

**Verification before recycling.** After the split, the app reads a checksum of every video and audio packet in the original and in the scene files. The scenes, joined in order, must hold exactly the original's packets, unchanged. It also decodes the first 2 seconds of each scene to check that it opens, and it checks that the original holds nothing the scenes do not carry (see Limitations). Only when all of this passes does the original go to the Recycle Bin (the Trash on macOS and Linux). If verification fails, the original stays where it is and the report says why. Recycling is the default. Turn it off with `--keep-original` or in the settings.

**Cache.** Scan results are stored as JSON files in `scene_splitter\scans` inside the system temp folder (on Windows, `%TEMP%\scene_splitter\scans`, elsewhere usually `/tmp/scene_splitter/scans`). A stored scan is reused only when the video's path, size, modified time and the scan settings all match. The stored hard-cut data lets you change sensitivity and shortest scene later without another slow scan. If the system clears the temp folder, the only cost is a rescan.

## Settings

The window has a Settings section with:

- Black-screen detection thresholds (shortest black screen, brightest pixel that counts as black, share of the frame that must be black).
- Thorough scan (decode every frame when looking for black screens).
- Where scenes are written (beside the video, or in a new `<video name>_scenes` folder next to it).
- Whether the original is moved to the Recycle Bin or Trash after verification.
- Hard-cut sensitivity and shortest scene.

Settings are saved to `scene_splitter/settings.json` in your settings folder (`%APPDATA%` on Windows, see the table under "ffmpeg"). Both the window and the command line use them. Command line options override them for that run only.

## Limitations

- Cuts land on keyframes. A cut can be up to one GOP (the distance between two keyframes) away from the frame you chose.
- The hard-cut scan finds shot changes, not story scenes. Expect to merge its results by hand.
- Audio and video sync in the player is loose. The player is for finding scene boundaries, not for watching.
- Scene files contain the first video stream and the audio streams. Other streams, such as subtitles or cover art, are not copied. An original that holds such streams fails verification on purpose, so it is never recycled.
- Run by hand on Windows only. See "Platforms".

## Running the tests

```
cargo test --release
```

This runs the unit tests, which need neither ffmpeg nor a display. They include tests that drive the real review screen with simulated mouse and keyboard input.

The end-to-end tests are ignored by default because they need `ffmpeg` and `ffprobe` on your `PATH`. They generate a small video from test patterns, then scan, split and verify it for real:

```
cargo test --release -- --include-ignored
```

The CI workflow runs all of them on Windows, macOS and Linux.

## License

No license has been chosen yet.
