# Cutaway guide

Everything Cutaway does, screen by screen and option by option. For a short tour, see the [README](../README.md).

The program file is still called `scene_splitter` (`scene_splitter.exe` on Windows), and so are its settings folder and the repository. Cutaway is the name in the window.

## Contents

- [The window](#the-window)
- [Command line](#command-line)
- [Settings](#settings)
- [ffmpeg](#ffmpeg)
- [Where files are kept](#where-files-are-kept)
- [How it works](#how-it-works)
- [Platforms](#platforms)
- [Limitations](#limitations)
- [Running the tests](#running-the-tests)

## The window

1. Drag videos or a folder into the window. Or press "Add videos" above the list on the left (Ctrl+O) to choose videos or a folder with the system's file picker, or to paste a path. A folder adds every video directly inside it. Subfolders are not searched, and scene files from an earlier split are left out.
2. The app scans the videos you added in the background, one at a time. The list on the left shows the state of each video. The video being scanned shows a progress bar with a percentage, in the list and on its own page.
3. When a scan finishes, the video is ready for review. Click it in the list. The review screen shows the picture, a timeline of the whole video with one coloured block per scene, and the scenes themselves. How these are laid out depends on the [style](#styles).
4. Edit the scenes: merge them, add splits, move splits, drop scenes you do not want written, and undo. See [Editing the scenes](#editing-the-scenes).
5. Choose whether the original is moved to the Recycle Bin or Trash, and whether the scenes go beside the video or in a new folder. Then press "Split into N files". The first time a split would move an original to the Recycle Bin or Trash, the app says what will happen and that it only happens if the new files pass the check. Choose "Move it to the Recycle Bin" (or Trash), "Keep the original", or cancel. The app remembers that you answered and does not ask again. If scene files from an earlier split are in the way, the app asks before it replaces them. "Skip this video" writes nothing, and a skipped video can be taken up again.
6. While the split runs, a bar shows how far it is: first writing the scene files, then checking them. The video's card in the list shows the same. A split with [exact cuts](#exact-cuts) shows no bar. When the split is done, the app shows whether the check passed and what happened to the original. "Open folder" shows the scene files.

While any scene is dropped, the button reads "Split into 3 files, 1 scene dropped". While any scene is dropped or [exact cuts](#exact-cuts) are on, the original is always kept. See [When the original is always kept](#when-the-original-is-always-kept).

A video whose file you delete, move or rename while it is listed is taken off the list within a few seconds, and a note in the sidebar says so. A video that is being scanned or split, or is finished, stays. So does one whose whole folder cannot be seen, since its drive may be back.

When something goes wrong, the app says so in plain words and shows the technical reason under it. You can select that text, and "Copy details" puts all of it on the clipboard.

If the scan could not read a stretch of the video, a note says where, because a scene change there may have been missed.

The window can be made as small as 900 by 600, and every style is laid out to work at that size. On a high-resolution screen the preview pictures and the player's picture are decoded larger, so they stay sharp.

Scene files are named `<video name>_scene_1_of_5`, `_scene_2_of_5` and so on, with the extension of the original, so each one says how many scenes there are. You can choose another name (see [Scene file names](#scene-file-names)).

"Rescan" ("Scan again" in Studio and Soft) ignores the stored scan of the video and looks for black screens afresh. What an earlier look for sudden picture changes found in the same file is kept. A rescan asks first if it would throw away merges or splits you made.

### Styles

"Style", in Settings under Appearance, picks one of three layouts for the review screen. Each comes in light and dark (see [Light and dark](#light-and-dark)). They show the same review. Editor and Soft hold every control. Studio keeps only the main tools on screen: switch to Editor for "Scenes missed?", "Scene lengths", saving the scene list or "Reset scenes".

| Style | What it looks like |
| --- | --- |
| Editor (the default) | Like a video editor. The player sits beside a list of scenes with preview pictures, and a film strip timeline runs under both. The buttons along the top give the picture the whole window ("Focus"), open the black screen settings, scan again ("Rescan"), and open a menu with "Scenes missed?", "Scene lengths", "Leave out the chapter marks" (for a video with chapters), "Keys", "Reset scenes" and "Save the scene list". Drag the grips between the parts to give the picture, the scenes or the timeline more room. |
| Studio | The picture as large as the window allows. The tools (Undo, Merge, Drop, "Scan again" and "Settings for black screens") are square buttons down the left. Under the picture are a tall film strip and one button per scene: click one to select that scene and move the player to its start, Shift-click another to select every scene between. |
| Soft | Scene cards with preview pictures in a scrolling list, under a row of coloured scene bars. A row of links above the scenes holds "Scan again", "Black screen settings", "Scene lengths", "Scenes missed?", "Keys" and "Save the scene list". The first time, a "How to use this screen" card sits above the scenes with three tips. |

"Focus" (F) in Editor and Studio gives the window to the picture and the timeline. Press it again ("Show everything") to bring the rest back.

### Editing the scenes

- **Select.** Click a scene, Ctrl-click to add one, Shift-click to select every scene between. Clicks on the timeline and the film strip select in the same way.
- **Merge.** Select scenes and press M or "Merge selected". A selection with gaps merges the scenes between as well. In Editor and Soft you can also drag a scene onto another. Soft also has a small "Merge scenes N and N+1" button between two cards.
- **Take apart.** U takes the selected merged scenes apart, as does "Take merged scenes apart" in Editor and Soft. In Soft, inside a merged scene, "Take apart here" restores a single split, and "Move out to its own scene" takes one part out of it. A merged scene with more than 6 parts shows its first two and last two, and "Show all N parts" shows the rest.
- **Add a split.** "Split at" in Editor and Studio, or "Split near here" in Soft's player, adds a split where the player is (S). "Add a split at" takes a time such as `1:03:12`. The split lands on the nearest point where the video can be cut (a keyframe). With [exact cuts](#exact-cuts) on, it stays at the time you chose.
- **Move a split.** Alt+Left and Alt+Right, or "Earlier" and "Later" in Soft, move the split at the start of a scene to the next point where the video can be cut. With exact cuts on, they move it by one picture instead. A split does not move onto or past the split beside it, into the parts of a merged scene, or to the ends of the video. The app says which of these stopped it.
- **Drop a scene.** X, the Drop tool, or "Drop this scene" marks the selected scenes as not to be written, for adverts or credits. X again keeps them. A dropped scene turns grey, and in Soft the other cards say which file they become ("File 2 of 3"), because files are numbered without the dropped scenes. Merging a dropped scene with a kept one keeps the whole merged scene. The app refuses to drop every scene.
- **Undo.** Ctrl+Z takes back the last change, a moved split or a dropped scene included. "Reset scenes" (in Soft, and in Editor's menu) returns to the scenes as found, with moved splits back in place and every scene written.
- **Scenes missed?** (not in Studio) If the video has scene changes with no black screen, choose how much to detect (only obvious changes, most changes, or even subtle changes) and the shortest scene to allow, then press "Look for sudden picture changes". This scan looks at every picture and is much slower.
- **Too many short scenes?** A video with many black screens close together, such as title cards that fade in and out, gives a row of scenes a few seconds long. "Scene lengths" sets the shortest scene to allow (see [Scene lengths](#scene-lengths)).

A scene longer than 45 minutes is marked as a hint that a scene change may have been missed.

### When the original is always kept

Two things make a split keep the original whatever you chose for it:

- **A scene is dropped.** The new files do not hold all of the video, so the original is the only place the dropped part still exists.
- **Exact cuts are on.** The pictures around each cut are encoded again, and no check can compare those with the original one for one. The original also stays when the video could not be cut on the exact pictures and was [cut at the nearest points instead](#when-a-video-cannot-be-cut-exactly). You asked for a split that keeps it, so it is kept.

In both cases the choice about the original is replaced. Soft shows one sentence that says the original is always kept, and why. Editor and Studio show a grey "Keep the original", and pointing at it gives the reason. The result of the split says the same. Your saved choice is not changed, and the one-time question about the Recycle Bin or Trash is not asked for such a split. It is left for the first split that could move an original.

### Chapter marks

If the video file has chapters, the scan reads them and offers each chapter start as a split, beside the ones it found itself. The status line says how many splits come from chapter marks.

- A chapter mark is moved to the nearest point where the video can be cut (a keyframe).
- If the chapter marks of a file cannot be read, the video is still scanned and can be split, without them. The status line says that they could not be read, and they are looked for again the next time the video is added.
- A mark at a split the scan found, or within one second of it, adds no second split there.
- A scene shows the title of the chapter that begins where it begins, on its card and when you point at its block on the timeline. A long title is cut short on the card, and pointing at it shows all of it. Titles are not used in file names. A saved [scene list](#scene-list) holds them.
- "Leave out the chapter marks" joins the scenes again wherever only a chapter mark separates them. Undo brings them back, and so does "Reset scenes".
- A split from a chapter mark can be moved like any other. Its scene keeps the chapter's title, and "Reset scenes" puts the split back on the mark.
- A dropped scene keeps its title on its card. It is in no file and in no scene list.

A scan stored by an earlier version of the app has no chapters in it. The app reads them the first time it reuses that scan, without scanning again.

### Split later, then split all ready videos

"Split later" (Ctrl+Shift+Enter), beside "Split into N files", keeps the scenes of the video as you reviewed them, dropped scenes included, and shows the next video. Nothing is written yet. The list on the left shows the video as "Ready to split".

When videos are ready, a "Split N ready videos" button appears above the list on the left. It splits them one after another, each into the scenes you reviewed. While it runs, the place of the button shows how many are done. At the end it says how many were split and checked, and what happened to the rest. "Hide this summary" puts that summary away.

Every safeguard of a single split applies to each video:

- If scene files from an earlier split are in the way when you press "Split later", the app asks before it marks the video ready. If such files turn up afterwards, the video is not split. It goes back into review with the question open, and agreeing there puts it back with the ready videos.
- Every safeguard is checked again when a video's turn comes, with the settings as they are then. A video whose split could move its original to the Recycle Bin or Trash before you were ever asked about that is not split. It stays ready, and the next run asks first. A video with splits on exact pictures is not split while exact cuts are off (see [Exact cuts](#exact-cuts)). It goes back into review with that question open.
- The one-time question about the Recycle Bin or Trash is asked once, before the first video is split, and the answer goes for all of them. It counts only the videos whose original could be moved. If every ready video [keeps its original anyway](#when-the-original-is-always-kept), nothing is asked.
- A ready video with dropped scenes keeps its original, and so does every video while exact cuts are on. Each video is split with the settings as they are when its turn comes.
- A ready video that could not be cut on the exact pictures is [cut at the nearest points instead](#when-a-video-cannot-be-cut-exactly). The run counts it among the videos whose result needs a look, and its page says why and how far its splits moved.
- A video whose split fails keeps its review and offers "Back to review". The videos after it are still split.

Click a ready video to see its page. It says how many files the split will write, how many scenes are dropped, and when the original is always kept. "Edit the scenes again" takes it back into review, as you left it. It is then no longer ready, until you press "Split later" again. Videos you mark ready while a run is under way wait for the next run.

### Scene list

"Save the scene list", in the review screen's menu (the row of links in Soft, not in Studio), saves the scenes without splitting anything. Pick a format, then say where to save:

| Format | What it holds |
| --- | --- |
| Table for a spreadsheet (CSV) | One row per scene: its number, start, end, length, the name its file would get, and the title of the chapter that begins with it, if the file has one. |
| Chapter marks to add to a video (ffmpeg metadata) | An `;FFMETADATA1` file with one `[CHAPTER]` per scene, named after the chapter of the file that begins with the scene, or "Scene 1" and so on. Add it to a video with `ffmpeg -i video.mp4 -i chapters.txt -map_metadata 1 -codec copy out.mp4`. |
| Edit list for an editing program (EDL) | A CMX3600 list with one cut per scene, in timecodes at the video's frame rate, counted without drop frames. It holds at most 999 scenes. |

Every format lists the scenes the split would write, numbered as their files are and named by the [pattern](#scene-file-names) in use. Dropped scenes are left out, not marked. Times are those of the original video, where your splits are now: with exact cuts on, a split you added or moved is listed at its exact time. In the edit list, the record side puts the kept scenes one after another. The app never writes a scene list over a video.

### Review keys

A review can be done without the mouse. The keys work on one scene at a time, which has a thick frame around its card in Soft or its row in Editor. Click a scene or press an arrow key to move the frame. The list scrolls to keep that scene in view.

On macOS, use Cmd where this page says Ctrl.

| Key | Action |
| --- | --- |
| Up, Down (W, S in Soft with the player closed) | Go to the scene above or below, and select it |
| Home, End | Go to the first or the last scene |
| Shift+Up, Shift+Down (also with Home and End) | Select more scenes, or fewer |
| Enter (Space too, in Soft with the player closed) | Play the scene from its start |
| Left, Right (in Soft with the player closed) | Choose a picture of the scene, which gets a frame too. Enter or Space then plays from that picture |
| J | Merge the scene with the one above it |
| M | Merge the selected scenes |
| U | Take a merged scene apart (the selected scenes or, with none selected, the scene with the frame) |
| X | Drop the scene, or keep it again if it is dropped (the selected scenes or, with none selected, the scene with the frame) |
| Alt+Left, Alt+Right | Move the split at the start of the scene to the point before or after it where the video can be cut. With exact cuts on, by one picture |
| S | Split where the player is (while the player is open) |
| F | Show only the picture and the timeline, or everything again (Editor and Studio) |
| Ctrl+Enter | Split into files. The same questions are asked as after the button |
| Ctrl+Shift+Enter | Split later, with the other ready videos. The same questions are asked as after the button |
| Ctrl+O | Add videos |
| Ctrl+A | Select every scene |
| Ctrl+Z | Undo the last change |
| Esc | Clear the selection (in Soft, when the player is open, it closes the player first) |
| F1 | Show or hide the list of keys (Soft) |
| Tab, Shift+Tab | Go through the buttons and boxes. Space or Enter presses the one that is marked, and Esc gives the keys back to the scenes |

"Keys" in the review screen's menu (Editor and Soft), "Show keys" on the tips card, and F1 in Soft show this list in the window.

In Editor and Studio the player is always open, and in Soft while you play a scene. Then Space, the plain Left and Right keys, A, D and S belong to the player: S splits where it is, and W with the Up and Down keys still move between scenes. Enter still plays the scene with the frame, and the other review keys work as before. While you type in a box, the keys are text.

### The player

Click a preview picture to play the video from that moment. A line on the timeline shows where the player is. The player starts muted, and asks before it turns the sound on. The question appears in front of the player with the rest of the window dimmed. Esc, or a click outside it, keeps the sound off and leaves the player open.

| Key or control | Action |
| --- | --- |
| A or Left arrow | Go back 10 seconds |
| D or Right arrow | Go forward 10 seconds (with Alt held, the arrow keys move a split in the review instead) |
| Space | Pause or play |
| S | Split the scene at the current position |
| Mouse wheel over the picture, or the "-1 picture" and "+1 picture" buttons | Pause and step one picture (wheel down is forward) |
| Position slider | Jump to any moment |
| 1x, 2x, 4x buttons | Playback speed |
| Mute box | Sound off or on |
| Esc | Close the player, or cancel the question about sound |

A split made in the player lands on the nearest point where the video can be cut (a keyframe). Once you pause, the player says how far that point is from the picture you are on. With [exact cuts](#exact-cuts) on, the split is made on the picture you are on, the button reads "Split here (S)" and not "Split near here (S)", and the player says so. Skipping or dragging the slider while paused stays paused. The player sizes itself to fit the window.

## Command line

| Command | What it does |
| --- | --- |
| `scene_splitter` | Opens the window. |
| `scene_splitter <folder>` | Opens the window with every video directly inside the folder in its list. |
| `scene_splitter <video>` | Scans the video and splits it at once, with no review. |
| `scene_splitter <video> --review` | Opens that video in the window instead of splitting it at once. |
| `scene_splitter <video> --list-only` | Prints the black screens and scenes. Writes no scene files. |
| `scene_splitter <video or folder> --scan-only` | Scans the video, or every video directly inside the folder, and stores the results in the cache. Writes nothing else. |

Splitting from the command line always scans the video afresh. Only the window and `--scan-only` read the cache. If the scan finds no scene changes, nothing is written.

| Option | Meaning |
| --- | --- |
| `--out-dir <folder>` | Where to write the scene files. The default is the saved setting: the video's own folder, unless you changed it. |
| `--overwrite` | Replace scene files left in the output folder by an earlier run, also when another pattern named them (see [Scene files of an earlier split](#scene-files-of-an-earlier-split)). Without it, the split stops if such files exist, and names them. With it, earlier scene files whose names the new split does not use go to the Recycle Bin or Trash. Also lets `--save-list` replace a file. |
| `--scene-name <pattern>` | How to name the scene files, whatever the saved setting says. The default is `{name}_scene_{number}_of_{count}`. See [Scene file names](#scene-file-names). |
| `--save-list <file>` | Write the list of scenes to this file: their numbers, times, the names their files get and their chapter titles. With `--list-only` it writes the list and no scene files. See [Scene list](#scene-list). |
| `--list-format <csv, chapters or edl>` | The format of the scene list. Left out, it follows the file's ending: `.csv`, `.txt` (or `.ffmetadata`) for chapters, `.edl`. |
| `--keep-original` | Keep the original video, whatever the saved setting says. |
| `--exact-cuts` | Cut on the exact picture where each scene changes: the middle of a black screen, or the first picture after a hard cut. A few seconds around each cut are encoded again, so the split takes longer. The original is always kept. A video that cannot be cut that way is cut at the nearest points where it can be cut instead. The run says so before and after, lists how far each split moved, and exits with an error code. A video that an exact cut would encode again for the most part is still cut exactly, after a warning on the error output that says how much of it is encoded (see [When an exact cut encodes most of the video](#when-an-exact-cut-encodes-most-of-the-video)). See [Exact cuts](#exact-cuts). |
| `--hard-cuts` | Also look for hard cuts (scene changes with no black screen). This decodes every frame and is much slower. It uses the saved sensitivity and shortest scene. |
| `--no-chapters` | Do not split at the chapter marks of the file. Without it, a video with chapters is also split where each chapter begins. |
| `--scan suspicious` | Decode only where packet sizes hint at a black screen. A video small enough to decode in a few seconds is decoded in full all the same. |
| `--scan every-frame` | Decode every frame. Use it when the black screens are noisy or grainy. |
| `--min-black <seconds>` | Shortest black screen that counts as a scene change. |
| `--max-luma <0 to 255>` | Brightest pixel value that still counts as black. |
| `--black-fraction <0 to 1>` | Share of a frame's pixels that must be black. |
| `--min-scene <time>` | The shortest a scene may be, whatever the saved setting says: a time like `90`, `1:30` or `1:02:03`. Splits are left out until no scene is shorter. Those at the longest black screens stay. `off` allows scenes of any length in this run. See [Scene lengths](#scene-lengths). |
| `--max-scene <time>` | The longest a scene may be, whatever the saved setting says. A longer scene is cut into equal parts, at the nearest points where the video can be cut. `off` allows scenes of any length in this run. |
| `--help`, `--version` | Print the help text or the version. |

Options you leave out take their value from the saved [settings](#settings). Out of the box these are `--scan suspicious`, `--min-black 0.25`, `--max-luma 32` and `--black-fraction 0.98`, with no limit on scene length. If the video's folder has [scan settings of its own](#settings-for-one-folder), those are the saved settings for that video, and the command says so. An option you give still wins.

A video with chapters is listed and split at its chapter marks too, and the list of scenes shows the chapter titles. `--no-chapters` leaves the marks out.

With a limit on scene length set, the listing says what it did, above the scenes: `No scene shorter than 1 min 00 s: 26 splits left out.` and `No scene longer than 20 min 00 s: 3 splits added.` The black screens are still all listed, so you can see which ones got no split.

A command line split exits with an error code when the scenes could not be verified, the original could not be moved, or `--exact-cuts` was given and the video was cut at the nearest points instead, so a script can tell.

On Windows the program is built as a window program, so that starting it from Explorer opens no console window. From a terminal it still prints there, but an interactive Command Prompt or PowerShell does not wait for a window program: the prompt comes back at once and the output appears after it. To wait for it, and to get its exit code, start it like this in Command Prompt:

```
start /wait "" scene_splitter video.mp4 --list-only
```

And like this in PowerShell:

```
scene_splitter video.mp4 --list-only | Out-Host
```

Which options work together:

- `--scan-only` cannot be combined with `--review`, `--list-only`, `--out-dir`, `--overwrite`, `--keep-original`, `--scene-name`, `--save-list`, `--no-chapters`, `--exact-cuts`, `--min-scene` or `--max-scene`. A scan stores what it found, and the limits on scene length are applied to that later.
- `--review` cannot be combined with `--list-only`, `--out-dir`, `--overwrite`, `--keep-original`, `--scene-name`, `--save-list`, `--no-chapters` or `--exact-cuts`. In the window, "Leave out the chapter marks" does the job of `--no-chapters`.
- `--list-only` cannot be combined with `--keep-original`.
- `--hard-cuts` works with every command. With `--review` or a folder, the window opens with the hard-cut scan already chosen for the videos in its list.
- The detection options (`--scan`, `--min-black`, `--max-luma`, `--black-fraction`, `--min-scene`, `--max-scene`), `--keep-original`, `--scene-name` and `--exact-cuts` apply to a command line run. The app refuses them when the command opens the window, where Settings does that job.
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

The window reuses a stored scan only if its scan settings match the ones the scan was made with. `--scan-only` and the window both use the settings remembered for the folder when there are some, so they agree by themselves. If you pass scan options to `--scan-only`, set the same values in the window.

## Settings

"Settings", at the top right of the window, opens a dialog with four tabs:

- **Splitting.** "Cut on the exact picture (slower)", off by default (see [Exact cuts](#exact-cuts)); whether the original is moved to the Recycle Bin or Trash after the check; whether scenes go beside the video or in a new `<video name>_scenes` folder next to it; and "Name scene files" (see [Scene file names](#scene-file-names)). While exact cuts are on, the original is always kept and the switch for the Recycle Bin or Trash is greyed out.
- **Finding scenes.** For missed scenes: how much to detect, and the shortest scene to allow between sudden picture changes. "How long a scene may be": "No scene shorter than" and "No scene longer than", both off at first (see [Scene lengths](#scene-lengths)). Under "Black screens": the shortest black screen, how dark counts as black, how much of the picture must be black, and "Check every picture", the slow scan for grainy black screens.
- **Appearance.** The [style](#styles), the colours (see [Light and dark](#light-and-dark)), and whether the tips are shown on the review screen (Soft).
- **Folders.** The folders with their own scan settings, each with "Forget" (see [Settings for one folder](#settings-for-one-folder)).

"Restore default settings", at the bottom of the dialog, leaves your choice about originals as it is, and does not bring back the one-time question about the Recycle Bin or Trash. It also forgets the settings remembered for folders, which the note beside the button says before you press it, and the window follows the system's light or dark setting again.

Settings are saved as you change them. If they could not be loaded or saved, the reason is shown at the top of the dialog, with "Copy details".

### Light and dark

"Colours", under Appearance in Settings, has three choices:

- "Same as the system" is the default. The window is dark when your system is set to dark and light when it is set to light, and it changes when the system does. On a system that does not say which it prefers, the window is light.
- "Light" and "Dark" keep the window that way whatever the system is set to.

The window changes as soon as you pick one, with no restart, and the choice is saved with the other settings. A settings file from an earlier version has no such choice and follows the system.

Both looks are made to be read: text has a contrast of at least 4.5 to 1 against whatever it is written on, and a scene keeps the same hue in both, lighter on the dark page, so "the orange scene" is orange in either. A video picture is always shown on black, as a film is.

### Scene lengths

Two limits say how long a scene may be. Both are off unless you tick them, and each works without the other.

**No scene shorter than.** For videos with many black screens close together, such as title cards that fade in and out. The app leaves out splits until no scene is shorter than the length you set. Which splits stay is decided like this:

- Splits are taken in order of how much they are worth. A split at a black screen comes before a split at a chapter mark, which comes before a split at a sudden picture change. Of two black screens the longer one comes first, of two sudden changes the stronger one, and of two that are alike the earlier one.
- A split stays if it is at least the shortest scene away from both ends of the video and from every split that already stays. Otherwise it is left out, and the scenes on both sides of it become one.

So a split is only ever left out because an end of the video, or a split worth more, is too close to it. The first and the last scene count like any other.

**No scene longer than.** A scene longer than the length you set is cut into equal parts, each within the limit: a 50 minute scene with a limit of 20 minutes becomes three scenes of about 17 minutes. Each cut goes on the nearest point where the video can be cut. If a part comes out a little too long because of that, the scene gets one part more. Where the video can only be cut far apart, the cuts come as close to the limit as the video allows.

The shortest scene is seen to first, and it wins where the two pull apart: the app never adds a cut that would leave a scene shorter than the shortest one, even if a scene then stays longer than the longest. A longest scene below the shortest is refused.

What you should know:

- The limits are applied to what the scan found, after the scan. Changing them needs no new scan and does not make a stored scan useless.
- They decide which scenes a review starts with, and what a split from the command line writes. They never undo a merge or a split you made by hand in a review. "Reset scenes" goes back to the scenes with the limits applied.
- The status line says what they did: "26 short scenes were merged away", "3 splits were added to long scenes".
- "Scene lengths", in the review screen's menu (the row of links in Soft, not in Studio), opens a card with the two limits for the video you are looking at. A change is saved straight away and holds for videos scanned from then on. "Use on this video" makes the scenes of this video again with them, without scanning. If you have merged or split scenes by hand, the app asks first, because those edits are replaced.
- The shortest scene set for missed scenes is another thing. It thins sudden picture changes, which films have every few seconds, before they are offered as splits at all, and it never takes a split at a black screen away. The limits here are applied afterwards, to every split of whatever kind.
- A folder with settings of its own remembers its own limits. On the command line, `--min-scene` and `--max-scene` set them for one run.

### Settings for one folder

Videos from different sources need different scan settings: old tapes have grainy black screens, a screen recording has none. So a folder can have scan settings of its own. These are the black screen settings, the limits on [scene length](#scene-lengths) and, for missed scenes, how much to detect and the shortest scene.

A folder remembers only how scenes are found. How scenes are written is one choice for every video: exact cuts, the file name pattern, where the scenes go and what happens to the original are not kept per folder.

- A video is scanned with the settings remembered for the folder it is in. If that folder has none, it is scanned with the general settings, the ones in Settings.
- Only the exact folder counts. A subfolder does not inherit the settings of the folder above it.
- On the review screen, the gear button ("Black screen settings" in Soft's row of links) opens a card with the black screen settings this video is scanned with. "Scenes missed?" shows the settings for missed scenes. Both say plainly whether these are the general settings or the ones remembered for the folder.
- "Remember these settings for this folder" keeps the black screen settings, the scene lengths and the settings for missed scenes as they are now for that folder. From then on, what you change on the review screen of one of its videos applies to that folder alone, and changes in Settings do not reach it.
- "Use the general settings for this folder" forgets them again. So does "Forget" beside the folder in Settings.
- While a folder has no settings of its own, changing a setting on the review screen changes the general one.
- A changed setting applies the next time a video is scanned. Press "Scan again" to use it on the video you are looking at. For the scene lengths, "Use on this video" does that without a scan.

The command line follows the same rule. See [Command line](#command-line).

The app saves settings as you change them, to `scene_splitter/settings.json` in the settings folder (see [Where files are kept](#where-files-are-kept)). Changed scan settings apply to videos scanned from then on. The window and the command line share the file.

Command line options override the saved settings for that run and are never saved.

If the settings file cannot be read, the app says so, uses the defaults, and keeps originals until you save settings again. It does not fall back to recycling them.

### Scene file names

"Name scene files" in Settings, and `--scene-name` on the command line, take a pattern. The default is `{name}_scene_{number}_of_{count}`, so every scene file says how many scenes its split wrote: `holiday_scene_1_of_5.mp4`, or `holiday_scene_07_of_12.mp4`. The extension of the video is added to the pattern.

| Placeholder | Stands for |
| --- | --- |
| `{name}` | The video's name without its extension. |
| `{number}` | The scene's number, from 1, with leading zeros, so the files of one split sort in scene order. In a pattern with `{count}` it has as many digits as the count: `1` of 5, `07` of 12. In a pattern without `{count}` it has three digits at least (`001`), and four in a split into a thousand scenes or more. |
| `{count}` | How many scene files the split writes, without leading zeros. |
| `{start}` | Where the scene starts in the video, as `00h03m12s`. |
| `{length}` | How long the scene is, in the same form. |

For example, `{name} part {number} of {count}` names the second of twelve scenes of `My video.mp4` as `My video part 02 of 12.mp4`, and `{name} part {number}` names it `My video part 002.mp4`. Under the box, the window shows an example of the name as you type, or says what is wrong with the pattern. A pattern that cannot be used changes nothing: the last good one stays in use.

Earlier versions named scene files `{name}_scene_{number}` by default (`holiday_scene_001.mp4`), and gave every number three digits at least, also beside a count (`My video part 002 of 12.mp4`). Settings saved by such a version that still hold the old default get the new one. If you want the old names back, type `{name}_scene_{number}` into the box.

A pattern must contain `{number}`, or every scene would get the same name, and `{name}`, or the scenes of two videos in one folder would replace each other. It cannot contain `/`, `\`, `<`, `>`, `:`, `"`, `|`, `?`, `*`, a line break or a tab, start with a space, end with a space or a dot, start with a name Windows keeps for itself (such as `CON` or `LPT1`), or be longer than 100 bytes, which is 100 plain letters and fewer with accents.

Scenes that are written are numbered from 1 without gaps, also when scenes are dropped. `{count}` counts the files, not the scenes. The pattern names the files of an exact split in the same way.

#### Scene files of an earlier split

Before a split, the app looks in the folder the scenes go to for scene files an earlier split of the same video left there. It knows them under:

- the pattern in use now,
- the default pattern, and the default of earlier versions (`holiday_scene_001.mp4`),
- the patterns earlier splits were written with. The app notes the pattern in the settings file each time it writes a split, in the window and on the command line, and keeps the last eight. "Restore default settings" does not forget them. A pattern you used only before the app kept this note is not known, unless it is one of the defaults.
- each of those as earlier versions wrote it, with three digits at least beside a count (`My video part 002 of 12.mp4`).

So changing the pattern does not hide an earlier split. The warning in the window and the message of the command line name the files of each earlier split, so you see each one before you agree to replace them. Files that are named another way than scenes are named now are said to be so: the new split writes over none of them, and all of them go to the Recycle Bin or Trash. Your agreement is for the files that were named. If other files are found when the split starts, the app asks again.

A file counts as a scene of an earlier split by its name alone, so the check is strict about names, and nothing it finds is deleted for good:

- The whole name has to fit: the video's name where `{name}` stands, the same ending as the video, and nothing before, between or after the parts of the pattern. `holiday_scene_1_of_50_extended.mp4` beside `holiday.mp4` is another video, not a scene.
- The video itself is never taken for one of its scenes.
- Beside a count, the number has to be one that split wrote: from 1 up to the count, with exactly the digits that count gives it, and the count has no leading zeros. `holiday_scene_6_of_5.mp4`, `holiday_scene_0_of_5.mp4`, `holiday_scene_01_of_5.mp4` and `holiday_scene_1_of_12.mp4` are not scenes.
- Without a count, a scene number has at least three digits and is not 0. With the pattern `{name} {number}`, `Rocky 2.mp4` beside `Rocky.mp4` is another video, and so is `holiday_scene_2.mp4` beside `holiday.mp4`.
- Without a count, a number with four digits or more only counts when the first scene with that many digits is there too (`film_0001.mp4`), as after a split into a thousand scenes or more. `Blade Runner 2049.mp4` beside `Blade Runner.mp4` is left alone.
- A pattern without `{count}` that is no longer in use counts only for a whole run of numbers from 1 with no gap (`IMG_001.mp4`, `IMG_002.mp4`, `IMG_003.mp4`). After you used `{name}_{number}` once, a single `IMG_007.mp4` beside `IMG.mp4` is left alone. The old default is the exception: its `_scene_` says enough.
- On Windows and macOS, names that differ only in capitals are the same file, so the check ignores capitals there. On Linux capitals count, in the ending of the name too.
- Files are told apart by the split that wrote them: the same pattern, the same count where the name says it, and numbers of the same length. Only files that belong to such a split are replaced or removed.
- A new split writes over the earlier scene files whose names it uses. Earlier scene files with names it does not use go to the Recycle Bin or Trash, where you can get them back: the higher numbers of a split into more scenes, and every file of a split with another count or another pattern.
- If a file that was not recognised as an earlier scene has the very name a new scene would get, the split stops before it writes anything, and names that file.

## ffmpeg

The app needs `ffmpeg` and `ffprobe`. It looks for them in this order:

1. Its own copy, in `scene_splitter/tools/ffmpeg-6.1.1` inside the data folder (see [Where files are kept](#where-files-are-kept)).
2. Your `PATH`.
3. If neither has both programs, it downloads ffmpeg into the folder from step 1. This happens once. The window shows a progress bar, and the command line prints the progress. You can add videos while it downloads. If the download fails, the window offers "Try again".

The downloads are the static builds from the `b6.1.1` release of the [`ffmpeg-static`](https://github.com/eugeneware/ffmpeg-static) project on GitHub, two gzipped files per system. The release bundles builds from different builders, so the ffmpeg version depends on the system. The folder name is the same everywhere:

| System | Download size | ffmpeg version |
| --- | --- | --- |
| Windows x64 | about 56 MB | 6.1.1 |
| macOS, Apple Silicon | about 37 MB | 6.0 |
| macOS, Intel | about 48 MB | 6.1.1 |
| Linux x64 | about 56 MB | 7.0.2 |
| Linux ARM64 | about 49 MB | 7.0.2 |

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

**Packet-size prefilter.** A black frame compresses to almost nothing (about 25 bytes, against several kilobytes for a normal frame). `ffprobe` reads the packet table without decoding, which takes about a second even for a long file. The app then decodes only the stretches with tiny packets, as small grey thumbnails, to confirm which frames are really black. This is why the normal scan is fast. Grainy or noisy black screens do not compress this small, and neither does a black screen with a logo in a corner. A video is therefore decoded in full whenever that takes only a few seconds: up to three billion pixels for each processor thread, which on 16 threads is about two and a half hours of 640x360 or a quarter of an hour of 1080p. A scan stored before this was so is made again the next time the video is opened. For a larger video, `--scan every-frame` (or "Check every picture" in the settings) decodes the whole video instead.

**Hard cuts.** The hard-cut scan decodes every frame and compares it with the frame before. A hard cut is a single frame that differs sharply from its predecessor while its neighbours barely change. Films cut between camera angles every few seconds, so the app keeps the strongest changes and drops the rest until every scene is at least as long as the shortest scene you set.

**Scene lengths.** The limits on [scene length](#scene-lengths) are not part of the scan. A scan stores every black screen and every sudden picture change it found. When a review or a split starts, the splits those suggest and the splits from chapter marks are put in one list, each with its reason: a black screen and how long it lasted, a chapter mark, or a sudden change and how strong it was. The shortest scene keeps the splits worth most and leaves out those too close to them, and the longest scene then cuts what is still too long into equal parts on keyframes. The window and the command line use the same code for this, so they give the same scenes.

**Keyframe cuts.** The split copies the compressed streams, and a copy can only start a file on a keyframe, because every other frame depends on the frames before it. Each cut goes on the keyframe nearest to its black screen. A keyframe inside the black screen gives a clean cut. Otherwise the cut lands slightly before or after it, and one scene keeps a moment of its neighbour.

**Videos that do not say when a picture is shown.** ffmpeg starts a scene file only at a keyframe whose show time it knows. An AVI file with MPEG-4 video (DivX, Xvid) says when each picture is decoded, but for its keyframes not when they are shown. Left alone, ffmpeg copies such a video into a single file and reports no problem. The split therefore gives every picture without a show time its decode time, which is also the time the scan reads for it, so each cut falls on the keyframe it was planned on. This needs ffmpeg 4.4 or newer. An older ffmpeg is asked to work the times out itself, and a cut can then fall on the keyframe before the planned one where two keyframes are only a few pictures apart. If ffmpeg still writes fewer scene files than there are scenes, the split fails and says how many were written.

<a id="exact-cuts"></a>**Exact cuts.** With "Cut on the exact picture (slower)" on (or `--exact-cuts`), each cut lands on the first picture shown at or after the moment you chose, so a scene starts less than one picture after it. The app writes each scene from up to three parts:

1. The pictures from the cut to the next keyframe are encoded again with libx264, set up like the original: same profile, level, pixel format, picture size, colour properties and picture times.
2. Everything from that keyframe to the last keyframe before the scene's end is copied, untouched.
3. The pictures from that last keyframe to the end of the scene are encoded again.

ffmpeg then joins the parts without touching them. A video filmed with a phone held upright is marked to be shown turned, and ffmpeg 8 loses that mark from the pictures it encodes again, so with that version the app names the turn again as it joins the parts. A scene with no keyframe to copy between is encoded again as a whole. A cut that happens to be on a keyframe encodes nothing. A copy only starts on a keyframe that the pictures stored after it do not reach back across.

Sound is never encoded again. Each scene gets every packet of sound that starts while its pictures are shown, so the sound of a scene can begin up to one packet (about 0.02 seconds for AAC) after its first picture and end up to one packet after its last. Inside a scene, sound and picture are in step as in the original, to within 2 thousandths of a second.

Before the scene files get their names, the app decodes the original and every scene from start to end and checks that:

- every scene decodes to its end without an error,
- each scene has exactly the pictures planned for it, so the scenes together have as many pictures as the original and each scene starts on the picture that was chosen,
- every copied picture decodes to the very same picture as in the original,
- every picture that was encoded again looks like its picture in the original, and not more like the one before or after it. The app compares small grey copies of them, 16 by 16 cells each. This is what would show an encoding that began one picture early or late,
- each scene is to be shown as the original is: the same picture size and shape, the same turn, the same colour properties,
- the pictures of each scene are shown at the pace of the original, also across the joins,
- the scenes hold every packet of the original's sound, unchanged and in order, each in the scene in which it starts,
- every packet of sound of each scene is in step with its pictures.

With dropped scenes, an exact split still writes and checks every scene, since the check counts the pictures and the sound of the whole video. Only then are the dropped scenes left out and the kept ones named, numbered without gaps.

If any of this does not hold, the scene files are thrown away before they get their names. What the check cannot prove is that the pictures that were encoded again look the same as the original's in every detail, since they are new. The small copies tell a wrong picture from the right one, not a slightly worse one from a good one, and they cannot tell apart two pictures in a row that show the same thing. So the original is always kept after an exact cut, whatever the setting for originals says. See [When the original is always kept](#when-the-original-is-always-kept).

A scene that has no stretch between two points where a copy can start and end is encoded again as a whole. In a video whose keyframes are no clean starts (many discs and broadcasts are stored that way) that is every scene, so the whole video is encoded again and the split takes as long as that. The result says how many pictures were encoded again and how many scenes as a whole.

<a id="when-an-exact-cut-encodes-most-of-the-video"></a>**When an exact cut encodes most of the video.** The app tells you before the split, not only after it. With exact cuts on, it lists the pictures of the video under review in the background and works out, for your splits as they are, how much of the video an exact cut would encode again. From half of the video's pictures on, the review screen says so above the choices for the split, for example: "An exact cut encodes all of this video again (1 h 58 min). That can take hours. Cutting at the nearest points instead takes seconds." For less than half an hour of encoding it says that the split takes much longer than cutting at the nearest points. The notice follows your splits: merge scenes or move a split onto a point where a copy can start, and it changes or goes away. The command line prints the same warning to the error output before it starts, and how to ask for the normal cut. The split is not refused and not changed. You asked for exact cuts, so you are told what they cost and decide: split anyway, or turn "Cut on the exact picture" off (leave out `--exact-cuts`) and have the video cut at the nearest points in seconds.

Checking takes about as long as decoding the video twice, and while a scene is written its parts take up as much room again as that scene.

<a id="when-a-video-cannot-be-cut-exactly"></a>**When a video cannot be cut exactly.** Exact cuts work for H.264 video with 8-bit 4:2:0 colour that is not interlaced, in MP4, MOV or Matroska files. They need ffmpeg 4.4 or newer with the libx264 encoder, which the ffmpeg the app downloads has. They do not work for a file that stores pictures it never shows, as a video does that was trimmed without encoding.

A video that cannot be cut exactly is not refused. It is cut the normal way, by copying, at the points nearest to your splits where it can be cut:

- **Known before the split.** The kind of file, how its pictures are stored and what the ffmpeg in use can do show at a glance. The rest shows once every picture is listed, such as two splits on the same picture, or a file that stores pictures it never shows. With exact cuts on, the app looks at both in the background, and the review screen then says above the choices for the split that this video will be cut at the nearest points instead, and why. The command line prints the same before it starts. Should the pictures not be listed in time or at all, the split itself finds those reasons when it starts.
- **Found out during the split.** If the scenes could not be written on the exact pictures, or were written and did not pass the check above, they are thrown away. Nothing of them is left, and scene files of an earlier split are still as they were. The video is then cut at the nearest points as well.

Either way the result says so in plain words: that the video could not be cut on the exact picture, why, that it was cut at the nearest points where it can be cut instead, how many splits moved, and how far the one that moved farthest went ("from 00:00:02.920 to 00:00:02.000, 0.920 seconds earlier"). The technical reason is under "Details". The command line lists every split with where it was asked for and where the video was cut. Such a result is headed "The files were written, but check the note below", and a run over the ready videos counts it among the results that need a look.

The scene files of such a split are plain copies, and the app [checks them](#how-it-works) like those of any normal split. The original is kept all the same, because you asked for a split that always keeps it.

Two things stop even that, with nothing written and the original untouched: two of your splits have the same nearest point where the video can be cut (merge the scenes on both sides of one of them), or the video has no such point near a split. Trouble that has nothing to do with exact cuts, such as a full disk, a drive that stopped answering or an ffmpeg that does not start, fails the split as before. A normal cut would only fail on it again.

**Nothing half-written.** The split writes the scenes into a folder named `unfinished_` and a number, inside the folder they go to. Only when every scene is there does it move them to their names and remove that folder. A split that fails or stalls leaves no partial scene files, and leaves the scene files of an earlier split as they were. In the window, a video whose split failed offers "Back to review", which brings back the scenes as you left them, and "Try again", which scans it afresh.

**Verification before recycling.** After the split, the app checks three things:

1. The original holds no stream that the scenes do not carry (see [Limitations](#limitations)).
2. Each scene file reads to its end. A file that is cut short or has a broken index fails here.
3. Each scene opens and the first 2 seconds of its pictures decode to at least one picture.

The check does not compare the scenes with the original. Scenes are copies, so damage the original has itself, such as a broken packet of sound, is in the scenes too and does not fail the check.

Only when all three pass does the original go to the Recycle Bin (the Trash on macOS and Linux). If verification or the move fails, the original stays where it is and the report says why. Recycling is the default. Turn it off with `--keep-original`, on the review screen, or in the settings.

**Dropped scenes.** ffmpeg cuts the whole video in one pass, so a split with dropped scenes first writes every scene into the `unfinished_` folder, the dropped ones too. It moves only the kept scenes to their names, removes the rest with the folder, and checks the kept files like those of any split. Such a split needs as much free room while it runs as a split of every scene. The original is [always kept](#when-the-original-is-always-kept) after it.

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
- A split you added or moved with exact cuts on stays where you put it if you turn exact cuts off again. A normal split cannot cut there, so it does not start: the review names those splits and offers to move them to the nearest points where the video can be cut (Undo takes that back), or you turn exact cuts back on. A split that cannot move, because another split is at that point already, is named too. Merge the scenes on both sides of it.
- Chapter marks go to the nearest keyframe also when exact cuts are on. Move the split by hand to put it on the exact picture.
- The scene files of an exact split carry no chapters, like those of a normal split.
- A scene of an exact split holds pictures from two encoders, the original's and libx264, one after another in one stream. ffmpeg decodes that, which the check makes sure of, and players built on ffmpeg should too. A player that reads how the pictures are stored only once, at the start of the file, may show broken pictures for a moment after a join. If yours does, split that video with exact cuts off.
- Whether a video is interlaced is not always stated in the file. Exact cuts refuse a video that says it is. One that does not say is taken to be not interlaced, and the pictures around its cuts are encoded as whole pictures.
- A video that was cut at the nearest points instead of on the exact pictures reports where its splits were moved to. It does not measure afterwards where each scene file begins.
- The hard-cut scan finds shot changes, not story scenes. Expect to merge its results by hand.
- Audio and video sync in the player is loose. The player is for finding scene boundaries, not for watching.
- Scene files contain the first video stream and the audio streams. Other streams, such as subtitles or cover art, are not copied. An original that holds such streams fails verification on purpose, so it is never recycled.
- The check makes sure every scene file is whole and plays. It does not prove that the scene files together hold everything the original did.
- A folder you add leaves out scene files from an earlier split if they have the default name (`<video name>_scene_1_of_5`), the default name of earlier versions (`<video name>_scene_001`, with three digits or more), or a name from a pattern with `{count}` that is in use or was used for a split. Scene files named by a pattern without `{count}`, such as `{name} part {number}`, are added like any other video, because such a name could be any video's. A video of your own that happens to be named like a scene, such as `Lecture 1 of 2.mp4` while the pattern `{name} {number} of {count}` is known, is left out too. Add it by itself: a file you name directly is always taken.
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
