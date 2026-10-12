# Scene Splitter

[![CI](https://github.com/algometrix/scene_splitter/actions/workflows/ci.yml/badge.svg)](https://github.com/algometrix/scene_splitter/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/algometrix/scene_splitter?color=4F46C8)](https://github.com/algometrix/scene_splitter/releases/latest)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-4F46C8)
![Rust](https://img.shields.io/badge/rust-1.88%2B-FF7A50?logo=rust&logoColor=white)
![ffmpeg](https://img.shields.io/badge/ffmpeg-downloaded%20on%20first%20run-1FA97C)
![No re-encoding](https://img.shields.io/badge/splits-without%20re--encoding-2E90D9)

Scene Splitter cuts a long video into its scenes. It finds the stretches of black screen between scenes, shows the proposed scenes in a review screen, and writes one file per scene. It copies the compressed streams and re-encodes nothing, so a split takes seconds and loses no quality. If you want a cut on one exact picture, an optional setting re-encodes only the few seconds around each cut (see [Exact cuts](#exact-cuts)).

It is a desktop app and a command line tool in one program, written in Rust with egui/eframe. It runs `ffmpeg` and `ffprobe` for all video work, and downloads them itself the first time it needs them.

![The review screen: a timeline and one colour-coded card per scene](docs/review.png)

## Download

Ready-built programs are on the [Releases page](https://github.com/algometrix/scene_splitter/releases), for Windows (x64), macOS (Apple Silicon and Intel) and Linux (x64). Download the package for your system, unpack it, and start `scene_splitter`. There is nothing to install. If the page lists no release yet, build the program yourself as shown under [Quick start](#quick-start).

You do not need ffmpeg. If it is missing, the app downloads it the first time it runs. See [ffmpeg](#ffmpeg).

Each release has a `SHA256SUMS.txt` file. To check that a download arrived unchanged, put that file next to the package and run:

```
sha256sum --ignore-missing -c SHA256SUMS.txt        # Linux
shasum -a 256 --ignore-missing -c SHA256SUMS.txt    # macOS
```

On Windows, run this in PowerShell and compare the result with the line for your package in `SHA256SUMS.txt`:

```
Get-FileHash .\scene_splitter-v0.1.0-windows-x86_64.zip -Algorithm SHA256
```

The programs are not signed. Windows may show a "Windows protected your PC" notice, and macOS may refuse to open the program the first time. On macOS, running `xattr -d com.apple.quarantine scene_splitter` once in the unpacked folder allows it.

## Quick start

To build from source you need a Rust toolchain (1.88 or newer). On Linux, install the [build packages](#requirements) first.

```
git clone https://github.com/algometrix/scene_splitter
cd scene_splitter
cargo run --release
```

1. Drop a video onto the window, or press "Add videos" (Ctrl+O). The app scans it for black screens.
2. Review the proposed scenes. Drag one scene card onto another to merge them.
3. Press "Split into N files".

By default the original video moves to the Recycle Bin or Trash once the scene files pass verification. The first time a split would do that, the app explains it and asks. Untick that option on the review screen to keep the original.

## Contents

- [Download](#download)
- [What it does](#what-it-does)
- [Requirements](#requirements)
- [Build and run](#build-and-run)
- [Using the window](#using-the-window)
- [Command line](#command-line)
- [Settings](#settings)
- [ffmpeg](#ffmpeg)
- [Where files are kept](#where-files-are-kept)
- [How it works](#how-it-works)
- [Platforms](#platforms)
- [Limitations](#limitations)
- [Running the tests](#running-the-tests)
- [Bugs and feature requests](#bugs-and-feature-requests)
- [License](#license)

## What it does

- **Fast scan.** A two hour 480p video is scanned in about two seconds, because black frames show up in the packet sizes before anything is decoded.
- **Hard cuts.** An optional slower scan finds scene changes that have no black screen.
- **Chapter marks.** If the file has chapters, each chapter start is offered as a split, and a scene shows the title of the chapter that begins with it.
- **Review before writing.** Merge scenes by dragging, add a split by time, move a split a step earlier or later, move a part back out of a merged scene, undo, or reset.
- **Review without the mouse.** Every step of a review has a key, and the window lists them.
- **Drop scenes.** Mark adverts or credits as not to be written. The split leaves them out, numbers the other files without gaps, and always keeps the original.
- **Your own file names.** Choose how scene files are named, with placeholders such as `{name}` and `{number}`.
- **Scene list without splitting.** Save the scenes with their times, file names and chapter titles as a table (CSV), as chapter marks for ffmpeg, or as an edit list (EDL).
- **Review now, split later.** Mark each reviewed video with "Split later", then split all of them with one button.
- **Detection settings per folder.** A folder can have detection settings of its own, for videos from a source that needs them.
- **Built-in player.** Click a preview picture to play from there, step frame by frame with the mouse wheel, and split the scene at the current position.
- **Exact cuts, if you want them.** Off by default. With "Cut on the exact picture" on, a split lands on the very picture you chose. Only the pictures between the cut and the nearest keyframes are encoded again, and the original is always kept.
- **Verified before the original is removed.** After a split, the app checks every packet of the scene files against the original. Only then does it move the original to the Recycle Bin or Trash.
- **Scan ahead.** Scan a whole folder from the command line, and review the results in the window later.
- **No ffmpeg setup.** The app fetches ffmpeg on first run and checks it against a built-in checksum.
- **Light and dark.** The window follows your system's light or dark setting, or stays in the one you choose.

![The player, with frame stepping and a split button](docs/player.png)

## Requirements

- A Rust toolchain, version 1.88 or newer, with Cargo. A [downloaded program](#download) needs none.
- Windows, macOS or Linux. See [Platforms](#platforms) for what has been tested.
- On Linux, a few packages to build it (Debian and Ubuntu names):

  ```
  sudo apt install pkg-config libasound2-dev libxkbcommon-dev libwayland-dev
  ```

You do not need to install ffmpeg. See [ffmpeg](#ffmpeg).

## Build and run

```
cargo build --release
```

Cargo writes the program to `target/release/scene_splitter` (`target\release\scene_splitter.exe` on Windows). Run it with no arguments to open the window:

```
target/release/scene_splitter
```

You can also run it through Cargo: `cargo run --release -- <arguments>`.

## Using the window

1. Drop videos or a folder onto the window. Or press "Add videos" in the left panel (Ctrl+O) to choose videos or a folder with the system's file picker, or to paste a path. A folder queues every video directly inside it. Subfolders are not searched, and scene files from an earlier split are left out.
2. The app scans the queued videos in the background, one at a time. The queue on the left shows the state of each video. The video being scanned shows a progress bar with a percentage, in the queue and on its own page.
3. When a scan finishes, the video is ready for review. Each scene is a colour-coded card with preview pictures. A timeline above the cards shows the whole video with one block per scene. Point at a block to see the scene's number and times, and click it to jump to that scene's card. A block shows its scene number, and the scene's length, when there is room for them. A scene longer than 45 minutes is marked in red, as a hint that a scene change may have been missed.
4. Edit the scenes:
   - Drag a scene card onto another to merge them and everything between. The small "Join scenes N and N+1" button between two cards merges just those two.
   - Click, Ctrl-click or Shift-click cards to select several, then press M or "Merge selected". A selection with gaps merges the scenes between as well.
   - "Undo merge" takes the selected merged scenes apart again. Inside a merged scene, "Separate here" restores a single split, and "Move out to its own scene" takes one part out of it.
   - A merged scene with more than 6 parts shows its first two and last two. "Show all N parts" shows the rest, and "Show fewer parts" hides them again. The app remembers this for each scene while the review is open.
   - "Add a split at" takes a time such as `1:03:12`. The split lands on the nearest point where the video can be cut (a keyframe). With [exact cuts](#exact-cuts) on, it stays at the time you typed.
   - "Move split earlier" and "Move split later", at the two ends of the row between two cards, move the split between those scenes to the next point where the video can be cut. With [exact cuts](#exact-cuts) on, they move it by one picture instead, since an exact cut can be on any picture. The line under the toolbar says the old and the new time. A split does not move onto or past the split beside it, into the parts of a merged scene, or to the ends of the video. The app says which of these stopped it.
   - "Drop this scene" on a card marks the scene as not to be written, for adverts or credits. X does the same for the selected scenes or, with none selected, for the scene with the dark frame. X again keeps them. A dropped scene turns grey, its card says "Dropped, not written" and offers "Keep this scene", and its timeline block is an empty outline with the word "dropped". The other cards then say which file they become ("File 2 of 3"), because files are numbered without the dropped scenes. Merging a dropped scene with a kept one keeps the whole merged scene. Merging dropped scenes with each other leaves the result dropped. The app refuses to drop every scene.
   - Undo takes back the last change, a moved split or a dropped scene included. "Reset scenes" returns to the scenes as found, with moved splits back in place and every scene written.
5. If the video has scene changes with no black screen, press "Scenes missed?" under the toolbar. Choose how much to detect (only obvious changes, most changes, or even subtle changes) and the shortest scene to allow, then press "Look for sudden picture changes". This scan looks at every picture and is much slower.
6. Under the video's name, choose whether the original is moved to the Recycle Bin or Trash and whether the scenes go in a new folder. Then press "Split into N files". The first time a split would move an original to the Recycle Bin or Trash, the app says what will happen and that it only happens if the new files pass the check. Choose "Move it to the Recycle Bin" (or Trash), "Keep the original", or cancel. The app remembers that you answered and does not ask again. If scene files from an earlier split are in the way, the app asks before it replaces them. "Skip this video" writes nothing, and a skipped video can be taken up again.

   While any scene is dropped, the button reads "Split into 3 files, 1 scene dropped". While any scene is dropped or [exact cuts](#exact-cuts) are on, the original is always kept. See [When the original is always kept](#when-the-original-is-always-kept).
7. When the split is done, the app shows whether the check passed and what happened to the original. "Open folder" shows the scene files.

When something goes wrong, the app says so in plain words and shows the technical reason under it. You can select that text, and "Copy details" puts all of it on the clipboard.

If the scan could not read a stretch of the video, a note above the scenes says where, because a scene change there may have been missed.

The first time, a "How to use this screen" card sits above the scenes with three tips. "Hide tips" removes it, and Settings or "Show tips" brings it back. A scan or rescan asks first if it would throw away merges or splits you made.

The window can be made as small as 900 by 600. Below the name and the toolbar, everything scrolls with the scenes. Only the scene cards in view are drawn, to keep a long list of scenes quick to scroll.

On a high-resolution screen the preview pictures and the player's picture are decoded larger, so they stay sharp.

Scene files are named `<video name>_scene_001`, `_scene_002` and so on, with the extension of the original, unless you choose another name (see [Scene file names](#scene-file-names)).

"Scan again" ignores the stored scan of the video and looks for black screens afresh. What an earlier look for sudden picture changes found in the same file is kept.

### When the original is always kept

Two things make a split keep the original whatever you chose for it:

- **A scene is dropped.** The new files do not hold all of the video, so the original is the only place the dropped part still exists.
- **Exact cuts are on.** The pictures around each cut are encoded again, and no check can compare those with the original one for one.

In both cases the choice about the original on the review screen gives way to one sentence that says the original is always kept, and why. The result of the split says the same. Your saved choice is not changed, and the one-time question about the Recycle Bin or Trash is not asked for such a split. It is left for the first split that could move an original.

### Chapter marks

If the video file has chapters, the scan reads them and offers each chapter start as a split, beside the ones it found itself. The line under the toolbar says how many splits come from chapter marks.

- A chapter mark is moved to the nearest point where the video can be cut (a keyframe).
- A mark at a split the scan found, or within one second of it, adds no second split there.
- A scene shows the title of the chapter that begins where it begins, on its card and when you point at its block on the timeline. A long title is cut short on the card, and pointing at it shows all of it. Titles are not used in file names. A saved [scene list](#scene-list) holds them.
- "Leave out the chapter marks" joins the scenes again wherever only a chapter mark separates them. Undo brings them back, and so does "Reset scenes".
- A split from a chapter mark can be moved like any other. Its scene keeps the chapter's title, and "Reset scenes" puts the split back on the mark.
- A dropped scene keeps its title on its card. It is in no file and in no scene list.

A scan stored by an earlier version of the app has no chapters in it. The app reads them the first time it reuses that scan, without scanning again.

### Split later, then split all ready videos

"Split later" (Ctrl+Shift+Enter), beside "Split into N files", keeps the scenes of the video as you reviewed them, dropped scenes included, and shows the next video. Nothing is written yet. The queue lists the video as "Ready to split".

When videos are ready, a "Split N ready videos" button appears above the list in the left panel. It splits them one after another, each into the scenes you reviewed. While it runs, the left panel shows how many are done. At the end it says how many were split and checked, and what happened to the rest. "Dismiss" puts that summary away.

Every safeguard of a single split applies to each video:

- If scene files from an earlier split are in the way when you press "Split later", the app asks before it marks the video ready. If such files turn up afterwards, the video is not split. It goes back into review with the question open.
- The one-time question about the Recycle Bin or Trash is asked once, before the first video is split, and the answer goes for all of them. It counts only the videos whose original could be moved. If every ready video [keeps its original anyway](#when-the-original-is-always-kept), nothing is asked.
- A ready video with dropped scenes keeps its original, and so does every video while exact cuts are on. Each video is split with the settings as they are when its turn comes.
- A video whose split fails keeps its review and offers "Back to review". The videos after it are still split.

Click a ready video to see its page. It says how many files the split will write, how many scenes are dropped, and when the original is always kept. "Edit the scenes again" takes it back into review, as you left it. It is then no longer ready, until you press "Split later" again. Videos you mark ready while a run is under way wait for the next run.

### Scene list

"Save the scene list", on the line under the toolbar, saves the scenes without splitting anything. Pick a format, then say where to save:

| Format | What it holds |
| --- | --- |
| Table for a spreadsheet (CSV) | One row per scene: its number, start, end, length, the name its file would get, and the title of the chapter that begins with it, if the file has one. |
| Chapter marks for ffmpeg | An `;FFMETADATA1` file with one `[CHAPTER]` per scene, named after the chapter of the file that begins with the scene, or "Scene 1" and so on. Add it to a video with `ffmpeg -i video.mp4 -i chapters.txt -map_metadata 1 -codec copy out.mp4`. |
| Edit list for an editing program (EDL) | A CMX3600 list with one cut per scene, in timecodes at the video's frame rate, counted without drop frames. It holds at most 999 scenes. |

Every format lists the scenes the split would write, numbered as their files are and named by the [pattern](#scene-file-names) in use. Dropped scenes are left out, not marked. Times are those of the original video, where your splits are now: with exact cuts on, a split you added or moved is listed at its exact time. In the edit list, the record side puts the kept scenes one after another. The app never writes a scene list over a video.

### Review keys

A review can be done without the mouse. The keys work on one scene at a time, which has a dark frame around its card. Click a card or press an arrow key to put the frame on a scene. The list scrolls to keep that scene in view.

On macOS, use Cmd where this page says Ctrl.

| Key | Action |
| --- | --- |
| Up, Down | Go to the scene above or below, and select it |
| Home, End | Go to the first or the last scene |
| Shift+Up, Shift+Down (also with Home and End) | Select more scenes, or fewer |
| Enter or Space | Play the scene from its start |
| Left, Right | Choose a picture of the scene, which gets a frame too. Enter or Space then plays from that picture |
| J | Join the scene with the one below it |
| M | Merge the selected scenes |
| U | Take a merged scene apart again (the selected scenes or, with none selected, the scene with the frame) |
| X | Drop the scene, or keep it again if it is dropped (the selected scenes or, with none selected, the scene with the frame) |
| Alt+Left, Alt+Right | Move the split at the start of the scene to the point before or after it where the video can be cut. With exact cuts on, by one picture |
| Ctrl+Enter | Split into files. The same questions are asked as after the button |
| Ctrl+Shift+Enter | Split later, with the other ready videos. The same questions are asked as after the button |
| Ctrl+O | Add videos |
| Ctrl+A | Select every scene |
| Ctrl+Z | Undo the last change |
| Esc | Clear the selection (when the player is open, it closes the player first) |
| F1 | Show or hide the list of keys |
| Tab, Shift+Tab | Go through the buttons and boxes. Space or Enter presses the one that is marked, and Esc gives the keys back to the scenes |

"Keys" on the line under the toolbar, "Show keys" on the tips card, and F1 show this list in the window, above the scenes.

While the player is open, Space and the plain Left and Right keys are the player's, and so are A, D and S, which the review does not use. Enter still plays the scene with the frame, and the other review keys work as before. While you type in a box, the keys are text. No key is used for two things on one screen.

### The player

Click a preview picture to play the video from that moment. A line on the timeline shows where the player is. The player starts muted, and asks before it turns the sound on. The question appears in front of the player with the rest of the window dimmed. Esc, or a click outside it, keeps the sound off and leaves the player open.

| Key or control | Action |
| --- | --- |
| A or Left arrow | Go back 10 seconds |
| D or Right arrow | Go forward 10 seconds (with Alt held, the arrow keys move a split in the review instead) |
| Space | Pause or play |
| S | Split the scene at the current position |
| Mouse wheel over the picture, or the "-1 frame" and "+1 frame" buttons | Pause and step one frame (wheel down is forward) |
| Position slider | Jump to any moment |
| 1x, 2x, 4x buttons | Playback speed |
| Mute box | Sound off or on |
| Esc | Close the player, or cancel the question about sound |

A split made in the player lands on the nearest point where the video can be cut (a keyframe). Once you pause, the player says how far that point is from the picture you are on. With [exact cuts](#exact-cuts) on, the split is made on the picture you are on, and the player says so. Skipping or dragging the slider while paused stays paused. The player sizes itself to fit the window.

## Command line

| Command | What it does |
| --- | --- |
| `scene_splitter` | Opens the window. |
| `scene_splitter <folder>` | Opens the window with every video directly inside the folder queued. |
| `scene_splitter <video>` | Scans the video and splits it at once, with no review. |
| `scene_splitter <video> --review` | Opens that video in the window instead of splitting it at once. |
| `scene_splitter <video> --list-only` | Prints the black screens and scenes. Writes no scene files. |
| `scene_splitter <video or folder> --scan-only` | Scans the video, or every video directly inside the folder, and stores the results in the cache. Writes nothing else. |

Splitting from the command line always scans the video afresh. Only the window and `--scan-only` read the cache. If the scan finds no scene changes, nothing is written.

| Option | Meaning |
| --- | --- |
| `--out-dir <folder>` | Where to write the scene files. The default is the saved setting: the video's own folder, unless you changed it. |
| `--overwrite` | Replace scene files left in the output folder by an earlier run. Without it, the split stops if such files exist. Also lets `--save-list` replace a file. |
| `--scene-name <pattern>` | How to name the scene files, whatever the saved setting says. See [Scene file names](#scene-file-names). |
| `--save-list <file>` | Write the list of scenes to this file: their numbers, times, the names their files get and their chapter titles. With `--list-only` it writes the list and no scene files. See [Scene list](#scene-list). |
| `--list-format <csv, chapters or edl>` | The format of the scene list. Left out, it follows the file's ending: `.csv`, `.txt` (or `.ffmetadata`) for chapters, `.edl`. |
| `--keep-original` | Keep the original video, whatever the saved setting says. |
| `--exact-cuts` | Cut on the exact picture where each scene changes: the middle of a black screen, or the first picture after a hard cut. A few seconds around each cut are encoded again, so the split takes longer. The original is always kept. See [Exact cuts](#exact-cuts). |
| `--hard-cuts` | Also look for hard cuts (scene changes with no black screen). This decodes every frame and is much slower. It uses the saved sensitivity and shortest scene. |
| `--no-chapters` | Do not split at the chapter marks of the file. Without it, a video with chapters is also split where each chapter begins. |
| `--scan suspicious` | Decode only where packet sizes hint at a black screen. |
| `--scan every-frame` | Decode every frame. Use it when the black screens are noisy or grainy. |
| `--min-black <seconds>` | Shortest black screen that counts as a scene change. |
| `--max-luma <0 to 255>` | Brightest pixel value that still counts as black. |
| `--black-fraction <0 to 1>` | Share of a frame's pixels that must be black. |
| `--help`, `--version` | Print the help text or the version. |

Options you leave out take their value from the saved [settings](#settings). Out of the box these are `--scan suspicious`, `--min-black 0.25`, `--max-luma 32` and `--black-fraction 0.98`. If the video's folder has [detection settings of its own](#settings-for-one-folder), those are the saved settings for that video, and the command says so. An option you give still wins.

A video with chapters is listed and split at its chapter marks too, and the list of scenes shows the chapter titles. `--no-chapters` leaves the marks out.

A command line split exits with an error code when the scenes could not be verified or the original could not be moved, so a script can tell.

On Windows the program is built as a window program, so that starting it from Explorer opens no console window. From a terminal it still prints there, but an interactive Command Prompt or PowerShell does not wait for a window program: the prompt comes back at once and the output appears after it. To wait for it, and to get its exit code, start it like this in Command Prompt:

```
start /wait "" scene_splitter video.mp4 --list-only
```

And like this in PowerShell:

```
scene_splitter video.mp4 --list-only | Out-Host
```

Which options work together:

- `--scan-only` cannot be combined with `--review`, `--list-only`, `--out-dir`, `--overwrite`, `--keep-original`, `--scene-name`, `--save-list`, `--no-chapters` or `--exact-cuts`.
- `--review` cannot be combined with `--list-only`, `--out-dir`, `--overwrite`, `--keep-original`, `--scene-name`, `--save-list`, `--no-chapters` or `--exact-cuts`. In the window, "Leave out the chapter marks" does the job of `--no-chapters`.
- `--list-only` cannot be combined with `--keep-original`.
- `--hard-cuts` works with every command. With `--review` or a folder, the window opens with the hard-cut scan already chosen for the queued videos.
- The detection options (`--scan`, `--min-black`, `--max-luma`, `--black-fraction`), `--keep-original`, `--scene-name` and `--exact-cuts` apply to a command line run. The app refuses them when the command opens the window, where the Settings section does that job.
- A folder without `--scan-only` opens the window. The app refuses `--list-only`, `--out-dir`, `--overwrite`, `--save-list` and `--no-chapters` with a folder, since they work on a single video.
- `--list-format` needs `--save-list`.

To save the scenes of a video without splitting it:

```
scene_splitter film.mp4 --list-only --save-list film.csv
scene_splitter film.mp4 --list-only --hard-cuts --save-list chapters.txt
```

The command line has no review, so it cannot drop scenes. Do that in the window.

### Scanning ahead of time

`--scan-only` scans a batch of videos without the window open, for example overnight:

```
scene_splitter D:\videos --scan-only
scene_splitter D:\videos --scan-only --hard-cuts
```

The second form also runs the slow hard-cut pass. When you later open the same videos in the window, they are ready for review at once, and "Look for sudden picture changes" returns without another slow scan.

The window reuses a stored scan only if its detection settings match the ones the scan was made with. `--scan-only` and the window both use the settings remembered for the folder when there are some, so they agree by themselves. If you pass detection options to `--scan-only`, set the same values in the window.

## Settings

The Settings section is at the bottom of the left panel. It holds:

- "Cut on the exact picture (slower)". Off by default. See [Exact cuts](#exact-cuts). While it is on, the original is always kept and the box for the Recycle Bin or Trash is greyed out.
- Whether the original is moved to the Recycle Bin or Trash after the check.
- Whether scenes are written beside the video or in a new `<video name>_scenes` folder next to it.
- "Name scene files": how scene files are named. See [Scene file names](#scene-file-names).
- Whether the tips are shown on the review screen.
- "Colours", under "How the window looks": "Same as the system", "Light" or "Dark". See [Light and dark](#light-and-dark).
- For missed scenes: how much to detect, and the shortest scene to allow.
- Under "Advanced: what counts as a black screen": the shortest black screen, how dark counts as black, how much of the picture must be black, and "Check every picture", the slow scan for grainy black screens.
- "Folders with their own detection settings", once there are some, each with "Forget".
- "Restore default settings". This leaves your choice about originals as it is, and does not bring back the one-time question about the Recycle Bin or Trash. It also forgets the settings remembered for folders, and the window follows the system's light or dark setting again.

If the settings could not be loaded or saved, the reason is shown at the top of the section, with "Copy details".

### Light and dark

"Colours" in the Settings section has three choices:

- "Same as the system" is the default. The window is dark when your system is set to dark and light when it is set to light, and it changes when the system does. On a system that does not say which it prefers, the window is light.
- "Light" and "Dark" keep the window that way whatever the system is set to.

The window changes as soon as you pick one, with no restart, and the choice is saved with the other settings. A settings file from an earlier version has no such choice and follows the system.

Both looks are made to be read: text has a contrast of at least 4.5 to 1 against whatever it is written on, and a scene keeps the same hue in both, lighter on the dark page, so "the orange scene" is orange in either. A video picture is always shown on black, as a film is.

### Settings for one folder

Videos from different sources need different detection settings: old tapes have grainy black screens, a screen recording has none. So a folder can have detection settings of its own. These are the black screen settings and, for missed scenes, how much to detect and the shortest scene.

A folder remembers only how scenes are found. How scenes are written is one choice for every video: exact cuts, the file name pattern, where the scenes go and what happens to the original are not kept per folder.

- A video is scanned with the settings remembered for the folder it is in. If that folder has none, it is scanned with the general settings, the ones in the Settings section.
- Only the exact folder counts. A subfolder does not inherit the settings of the folder above it.
- On the review screen, "Black screen settings" under the toolbar opens a card above the scenes with the black screen settings this video is scanned with. "Scenes missed?" shows the settings for missed scenes. Both say plainly whether these are the general settings or the ones remembered for the folder.
- "Remember for this folder" keeps the settings as they are now for that folder. From then on, what you change on the review screen of one of its videos applies to that folder alone, and changes in the Settings section do not reach it.
- "Use the general settings for this folder" forgets them again. So does "Forget" beside the folder in the Settings section.
- While a folder has no settings of its own, changing a setting on the review screen changes the general one.
- A changed setting applies the next time a video is scanned. Press "Scan again" to use it on the video you are looking at.

The command line follows the same rule. See [Command line](#command-line).

The app saves settings as you change them, to `scene_splitter/settings.json` in the settings folder (see [Where files are kept](#where-files-are-kept)). Changed detection settings apply to videos scanned from then on. The window and the command line share the file.

Command line options override the saved settings for that run and are never saved.

If the settings file cannot be read, the app says so, uses the defaults, and keeps originals until you save settings again. It does not fall back to recycling them.

The review screen shows a short "How to use this screen" card. "Hide tips" removes it, and the Settings section brings it back.

### Scene file names

"Name scene files" in Settings, and `--scene-name` on the command line, take a pattern. The default, `{name}_scene_{number}`, gives the names the app has always written. The extension of the video is added to it.

| Placeholder | Stands for |
| --- | --- |
| `{name}` | The video's name without its extension. |
| `{number}` | The scene's number, from 1, with leading zeros: at least three digits, and as many as the number of scenes has. |
| `{count}` | How many scene files the split writes. |
| `{start}` | Where the scene starts in the video, as `00h03m12s`. |
| `{length}` | How long the scene is, in the same form. |

For example, `{name} part {number} of {count}` names the second of twelve scenes of `My video.mp4` as `My video part 002 of 12.mp4`. Under the box, the window shows an example of the name as you type, or says what is wrong with the pattern. A pattern that cannot be used changes nothing: the last good one stays in use.

A pattern must contain `{number}`, or every scene would get the same name, and `{name}`, or the scenes of two videos in one folder would replace each other. It cannot contain `/`, `\`, `<`, `>`, `:`, `"`, `|`, `?`, `*`, a line break or a tab, start with a space, end with a space or a dot, start with a name Windows keeps for itself (such as `CON` or `LPT1`), or be longer than 100 bytes, which is 100 plain letters and fewer with accents.

Scenes that are written are numbered from 1 without gaps, also when scenes are dropped. `{count}` counts the files, not the scenes. The pattern names the files of an exact split in the same way.

The check for scene files left by an earlier split uses the pattern in use now. Files written under another pattern are not counted, not replaced and not removed.

## ffmpeg

The app needs `ffmpeg` and `ffprobe`. It looks for them in this order:

1. Its own copy, in `scene_splitter/tools/ffmpeg-6.1.1` inside the data folder (see [Where files are kept](#where-files-are-kept)).
2. Your `PATH`.
3. If neither has both programs, it downloads ffmpeg 6.1.1 into the folder from step 1. This happens once. The window shows a progress bar, and the command line prints the progress. You can queue videos while it downloads. If the download fails, the window offers "Try again".

The downloads are the static builds from the `b6.1.1` release of the [`ffmpeg-static`](https://github.com/eugeneware/ffmpeg-static) project on GitHub, two gzipped files per system:

| System | Download size |
| --- | --- |
| Windows x64 | about 56 MB |
| macOS, Apple Silicon | about 37 MB |
| macOS, Intel | about 48 MB |
| Linux x64 | about 56 MB |
| Linux ARM64 | about 49 MB |

The app checks each file against a SHA-256 checksum built into the program and discards it on a mismatch. On any other system (Windows on ARM, for example), or to use your own ffmpeg, put `ffmpeg` and `ffprobe` on your `PATH`.

ffmpeg is a separate project with its own license (these builds are GPL). It is not part of this repository.

## Where files are kept

| System | Data folder | Settings folder | Temp folder |
| --- | --- | --- | --- |
| Windows | `%LOCALAPPDATA%` | `%APPDATA%` | `%TEMP%` |
| macOS | `~/Library/Application Support` | `~/Library/Application Support` | `$TMPDIR` |
| Linux | `$XDG_DATA_HOME` or `~/.local/share` | `$XDG_CONFIG_HOME` or `~/.config` | `$TMPDIR` or `/tmp` |

Inside each, the app uses a `scene_splitter` folder:

- Data folder: `scene_splitter/tools/ffmpeg-6.1.1`, the downloaded ffmpeg.
- Settings folder: `scene_splitter/settings.json`.
- Temp folder: `scene_splitter/scans`, the scan cache.

## How it works

**Packet-size prefilter.** A black frame compresses to almost nothing (about 25 bytes, against several kilobytes for a normal frame). `ffprobe` reads the packet table without decoding, which takes about a second even for a long file. The app then decodes only the stretches with tiny packets, as small grey thumbnails, to confirm which frames are really black. This is why the normal scan is fast. Grainy or noisy black screens do not compress this small, so `--scan every-frame` (or "Decode every frame" in the settings) decodes the whole video instead.

**Hard cuts.** The hard-cut scan decodes every frame and compares it with the frame before. A hard cut is a single frame that differs sharply from its predecessor while its neighbours barely change. Films cut between camera angles every few seconds, so the app keeps the strongest changes and drops the rest until every scene is at least as long as the shortest scene you set.

**Keyframe cuts.** The split copies the compressed streams, and a copy can only start a file on a keyframe, because every other frame depends on the frames before it. Each cut goes on the keyframe nearest to its black screen. A keyframe inside the black screen gives a clean cut. Otherwise the cut lands slightly before or after it, and one scene keeps a moment of its neighbour.

<a id="exact-cuts"></a>**Exact cuts.** With "Cut on the exact picture" on (or `--exact-cuts`), each cut lands on the first picture shown at or after the moment you chose, so a scene starts less than one picture after it. The app writes each scene from up to three parts:

1. The pictures from the cut to the next keyframe are encoded again with libx264, set up like the original: same profile, level, pixel format, picture size, colour properties and picture times.
2. Everything from that keyframe to the last keyframe before the scene's end is copied, untouched.
3. The pictures from that last keyframe to the end of the scene are encoded again.

ffmpeg then joins the parts without touching them. A scene with no keyframe to copy between is encoded again as a whole. A cut that happens to be on a keyframe encodes nothing. A copy only starts on a keyframe that the pictures stored after it do not reach back across.

Sound is never encoded again. Each scene gets every packet of sound that starts while its pictures are shown, so the sound of a scene can begin up to one packet (about 0.02 seconds for AAC) after its first picture and end up to one packet after its last. Inside a scene, sound and picture are in step as in the original, to within 2 thousandths of a second.

Before the scene files get their names, the app decodes the original and every scene from start to end and checks that:

- every scene decodes to its end without an error,
- each scene has exactly the pictures planned for it, so the scenes together have as many pictures as the original and each scene starts on the picture that was chosen,
- every copied picture decodes to the very same picture as in the original,
- the pictures of each scene are shown at the pace of the original, also across the joins,
- the scenes hold every packet of the original's sound, unchanged and in order, each in the scene in which it starts,
- the sound of each scene is in step with its pictures.

With dropped scenes, an exact split still writes and checks every scene, since the check counts the pictures and the sound of the whole video. Only then are the dropped scenes left out and the kept ones named, numbered without gaps.

If any of this does not hold, the scene files are thrown away, the app says so, and you can split the video the normal way instead. What the check cannot prove is how the pictures that were encoded again look, since they are new. So the original is always kept after an exact cut, whatever the setting for originals says. See [When the original is always kept](#when-the-original-is-always-kept).

Exact cuts work for H.264 video with 8-bit 4:2:0 colour that is not interlaced, in MP4, MOV or Matroska files, and need an ffmpeg with the libx264 encoder (the one the app downloads has it). For any other video the app refuses before it writes anything, and says how to split it the normal way. Checking takes about as long as decoding the video twice, and while a scene is written its parts take up as much room again as that scene.

**Nothing half-written.** The split writes the scenes into a folder named `unfinished_` and a number, inside the folder they go to. Only when every scene is there does it move them to their names and remove that folder. A split that fails or stalls leaves no partial scene files, and leaves the scene files of an earlier split as they were. In the window, a video whose split failed offers "Back to review", which brings back the scenes as you left them, and "Try again", which scans it afresh.

**Verification before recycling.** After the split, the app checks three things:

1. The original holds no stream that the scenes do not carry (see [Limitations](#limitations)).
2. The scene files, joined in order, hold exactly the original's video and audio packets. The app compares the size and checksum of every packet.
3. Each scene opens and its first 2 seconds decode without error.

Only when all three pass does the original go to the Recycle Bin (the Trash on macOS and Linux). If verification or the move fails, the original stays where it is and the report says why. Recycling is the default. Turn it off with `--keep-original`, on the review screen, or in the settings.

**Dropped scenes.** ffmpeg cuts the whole video in one pass, so a split with dropped scenes first writes every scene into the `unfinished_` folder, the dropped ones too. It notes the size and checksum of every packet of each dropped scene, moves only the kept scenes to their names, and removes the rest with the folder. The check then joins, in scene order, the packets read back from the kept files and the packets noted for the dropped scenes, and compares the result with the original. It passes only when every kept file holds exactly the packets of its own stretch of the original. Such a split needs as much free room while it runs as a split of every scene. The original is [always kept](#when-the-original-is-always-kept) after it.

**Chapter marks.** `ffprobe` lists the chapters of the file with the scan. Each chapter start goes to the nearest keyframe. A start within one second of a cut the scan found, or one that lands on the same keyframe, joins that cut instead of adding another, so a chapter that begins at a black screen does not leave a scene of a few frames. A chapter that begins with the video adds no cut.

**Cache.** The app stores each scan as a JSON file in the scan cache. It reuses a stored scan only when the video's path, size, modified time and the detection settings in force for that video all match. The stored hard-cut data lets you change sensitivity and shortest scene later without another slow scan. If the system clears the temp folder, the only cost is a rescan.

## Platforms

The code is written for Windows, macOS and Linux. The window has only been used by hand on Windows. The tests, including the end-to-end ones, run on Linux, and the CI workflow is set up to lint and test on all three systems. On macOS and Linux, treat the window, the player's sound and the move to the Trash as untested until you have tried them.

What differs between systems:

- The original goes to the Recycle Bin on Windows and to the Trash on macOS and Linux.
- The window borrows a system font: Segoe UI on Windows, Arial on macOS, DejaVu Sans on Linux. Without it, the toolkit's built-in font is used.
- macOS may refuse to run a downloaded program it has not seen before. If ffmpeg fails to start there, install it with Homebrew (`brew install ffmpeg`) so it is on your `PATH`.

## Limitations

- Cuts land on keyframes, unless [exact cuts](#exact-cuts) are on. A cut can be up to half the distance between two keyframes away from the moment you chose.
- Exact cuts place the splits you add yourself. The scenes a scan proposes in the window still start on keyframes. On the command line, `--exact-cuts` places the scan's own cuts exactly.
- A split you added or moved with exact cuts on stays where you put it if you turn exact cuts off again. The normal split then cuts at the first keyframe at or after it.
- Chapter marks go to the nearest keyframe also when exact cuts are on. Move the split by hand to put it on the exact picture.
- The scene files of an exact split carry no chapters, like those of a normal split.
- The hard-cut scan finds shot changes, not story scenes. Expect to merge its results by hand.
- Audio and video sync in the player is loose. The player is for finding scene boundaries, not for watching.
- Scene files contain the first video stream and the audio streams. Other streams, such as subtitles or cover art, are not copied. An original that holds such streams fails verification on purpose, so it is never recycled.
- A folder you add leaves out scene files from an earlier split only if they have the default name (`<video name>_scene_001`). With another name pattern, scene files in that folder are queued like any other video.
- The player does not show which scenes are dropped. The review screen does.
- Scene files do not carry chapters. An MP4 file written by ffmpeg keeps its chapters in a stream of their own, and other tools may do the same. That stream counts as one of the streams that are not copied. Such a video is split at its chapter marks like any other, but the check does not pass, so its original is kept. A Matroska (MKV) file keeps chapters beside its streams and passes the check.
- The window has been run by hand on Windows only. See [Platforms](#platforms).

## Running the tests

```
cargo test --release
```

This runs the unit tests, which need neither ffmpeg nor a display. They include tests that drive the real review screen with simulated mouse and keyboard input.

The end-to-end tests are ignored by default because they need `ffmpeg` and `ffprobe` on your `PATH` (the copy the app downloads does not count), built with the libx264 encoder. They generate small videos from test patterns, then scan, split, verify and play them for real, with normal cuts and with exact cuts:

```
cargo test --release -- --include-ignored
```

The CI workflow runs this second command on Windows, macOS and Linux, after a formatting check and Clippy.

## Bugs and feature requests

To report a bug or request a feature, [open an issue](https://github.com/algometrix/scene_splitter/issues). For a bug, include your operating system, the command or steps you used, and what the app printed or showed. Do not attach or name private videos.

## License

Scene Splitter is licensed under either of

- the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), or
- the MIT license ([LICENSE-MIT](LICENSE-MIT)),

at your option. The packages on the Releases page hold both texts.

ffmpeg, which the app downloads and runs, is a separate program with its own license. See [ffmpeg](#ffmpeg).
