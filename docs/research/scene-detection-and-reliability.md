# Scene detection, splitting and reliability: research notes

Written 2026-10-14. These notes compare what Scene Splitter does today with primary sources: ffmpeg source and manuals, PySceneDetect source, the TransNet V2 paper and code, LosslessCut source, and the OS and language docs. Line numbers for this repository refer to the working tree on that date. Third-party source links are pinned to a commit:

- ffmpeg: `045dea2a34ef93f854f3f4a30ddf8f77b35b34ce` (abbreviated `FF@045dea2`)
- PySceneDetect: `7cd1005e8349d8e913d0930fc1750363eb4ddc51` (`PSD@7cd1005`)
- LosslessCut: `b83d4df9ccd30a701cc5442624250d58ff964349` (`LC@b83d4df`)
- TransNet V2: `85cef72af9a916bdfd7cc94a670c9cdfbf12d1ed` (`TN@85cef72`)

"Unverified" marks a claim I could not trace to a primary source, or a conclusion drawn from source code that no test has confirmed yet. Run each of those through the `ffmpeg-lab` skill on a generated video before changing code.

## Summary: top recommendations, ranked by value for effort

| # | Change | Value | Effort | Where |
|---|---|---|---|---|
| 1 | Compare the scenes of a copy split with the original packet for packet before recycling the original | High | Low to medium | `verify.rs`, `export.rs` |
| 2 | Decode scan thumbnails with `-fps_mode passthrough` (`-vsync passthrough` before ffmpeg 7) | High for phone and screen recordings | Low | `scan.rs:180-188` |
| 3 | Do not cut a copy split on an open-GOP keyframe | High for disc rips and broadcasts | Medium | `scenes.rs:143`, `plan.rs:598`, `plan.rs:788-800`, `probe.rs` |
| 4 | Keep packets flagged "discard" (`D`) out of the frame index the scan maps thumbnails onto | Medium | Low | `probe.rs:245-260` |
| 5 | Make the scene files durable (`sync_all`, and the folder on Unix) before the original goes to the Recycle Bin | Medium | Low | `split.rs:566-580` |
| 6 | Reject camera flashes in the hard-cut scan by comparing frame N-1 with frame N+1 | Medium | Low | `hardcut.rs:206-235` |
| 7 | Ignore letterbox bars when measuring picture change | Medium | Low to medium | `hardcut.rs:193-204` |
| 8 | Add synthetic test videos for VFR, discard packets, flash, fade, letterbox, interlaced, telecine, HDR tags, MPEG-TS offset | Medium | Low to medium | `test_videos.rs` |
| 9 | Property tests or fuzzing for every parser of ffmpeg output | Medium | Low | `probe.rs`, `verify.rs`, `ffmpeg.rs` |
| 10 | Add colour (chroma or histogram) to the hard-cut score | Medium | Medium | `scan.rs`, `hardcut.rs` |
| 11 | Use silence as extra evidence for a black-screen cut | Low to medium | Medium | new, beside `black.rs` |
| 12 | A learned shot-boundary model (TransNet V2) as an optional scan | High for dissolves | High | new |

Items 1 to 4 are correctness gaps: today the app can report success while a scene is wrong. Items 5 to 9 are cheap hardening. Items 10 to 12 add detection quality at a growing cost.

---

## 1. Black-frame and scene-change detection

### What the app does today

- **Black frames.** Each thumbnail is 64x36 grey (`scan.rs:17-21`, filter `scale=64:36:flags=fast_bilinear,format=gray` at `scan.rs:174-175`). A frame is black when at least 98% of its pixels have luma 32 or less, and a black screen must last 0.25 s (`black.rs:22-28`, `black.rs:43-52`).
- **Packet-size prefilter.** Only stretches whose packets are 8 times smaller than the median (or at most 64 bytes) are decoded, unless a full decode fits a pixel budget (`windows.rs:20-50`, `windows.rs:83-105`, `detect.rs:327-338`).
- **Hard cuts.** The mean absolute luma difference between consecutive thumbnails, kept when it is at least 12 and at least 3 times the mean of the 6 frames on each side (`hardcut.rs:22-29`, `hardcut.rs:193-235`). Cuts then move to the nearest keyframe and are thinned strongest first to a minimum scene length (`hardcut.rs:243-262`).

### How ffmpeg's filters score frames

| Filter | Rule (from source) | Defaults |
|---|---|---|
| `blackdetect` | Pixel black if luma <= `16 + pix_th * 219` in limited range, or `pix_th * max` in full range; frame black if the black share >= `pic_th`. Range comes from the frame's `color_range` ([FF@045dea2 vf_blackdetect.c:193-195, 211](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_blackdetect.c#L193-L211)) | `d=2` s (line 65), `pic_th=0.98`, `pix_th=0.10`, so about luma 38 in 8-bit limited range |
| `blackframe` | Counts luma pixels `< threshold` on plane 0 only; black when the integer percentage >= `amount` ([vf_blackframe.c:133-135](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_blackframe.c#L133-L135)) | `amount=98`, `threshold=32` |
| `scdet` | `mafd = SAD * 100 / count / 2^bitdepth`, `diff = abs(mafd - prev_mafd)`, `score = min(mafd, diff)` clipped to 0..100. Luma plane only for planar YUV ([vf_scdet.c:165-167](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_scdet.c#L165-L167)) | `threshold=10` (lines 54-55) |
| `select='gt(scene,X)'` | Same `min(mafd, diff)` idea, divided by 100 and clipped to 0..1 ([f_select.c:304-306](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/f_select.c#L304-L306)) | none |
| `freezedetect` | Frozen when the mean absolute difference over all planes against a reference frame is <= `noise` ([vf_freezedetect.c:135](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_freezedetect.c#L135)) | `noise=0.001`, `d=2` s (lines 55-56) |
| `silencedetect` | A sample is silent when strictly between `-noise` and `+noise`. Without `mono`, all channels must be silent together ([af_silencedetect.c](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/af_silencedetect.c)) | `noise=0.001` (-60 dBFS), `d=2` s |
| `cropdetect` | Scans rows and columns inward from each edge and stops at the first lines brighter than `limit` ([vf_cropdetect.c:452](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_cropdetect.c#L452)) | `limit=24/255`, `round=16`, `skip=2` |
| `idet` | Compares the two field parities against neighbouring frames and labels each frame TFF, BFF, progressive or undetermined, plus repeated top or bottom fields ([vf_idet.c](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_idet.c)) | `intl_thres=1.04`, `prog_thres=1.5`, `rep_thres=3.0` |

What this says about the app:

- The black rule matches `blackframe` exactly (98%, 32) and is close to `blackdetect` (about 38 in limited range). The app's own rule is a sound choice, and it keeps the thresholds testable in Rust.
- `blackdetect` picks its threshold from the frame's colour range. The app uses 32 whatever the range. **Unverified:** whether swscale keeps limited-range codes when it converts `yuv420p` to `gray`, or stretches them to full range. The tests in `black.rs` treat 16 as dark, which assumes limited range is kept. A full-range source (`yuvj420p`, many phone and screen recordings) puts black at 0, so the same 32 is a stricter cut-off there. Settle it with a lab test: a `color=black` and a `color=gray` clip in both ranges, then read the thumbnail bytes.
- `scdet` and `select` use `min(mafd, |mafd - prev_mafd|)`. That rejects steady motion, as the app's spike ratio does, but it compares only with the previous frame. The app's ratio against 12 neighbours is stricter and better at rejecting fast action. Keep it.

### PySceneDetect's detectors (source at `PSD@7cd1005`)

| Detector | Score | Defaults in source |
|---|---|---|
| `ContentDetector` | Weighted mean of per-pixel differences of hue, saturation and luma in HSV, plus optional Canny edges ([content_detector.py:155, 238](https://github.com/Breakthrough/PySceneDetect/blob/7cd1005e8349d8e913d0930fc1750363eb4ddc51/scenedetect/detectors/content_detector.py#L155)) | `threshold=27.0` (line 106), edges weight 0, `min_scene_len=15` frames |
| `AdaptiveDetector` | `ContentDetector` score divided by the mean of the `window_width` frames on each side, excluding the frame itself; cut when ratio >= `adaptive_threshold` and score >= `min_content_val` ([adaptive_detector.py:39-42, 137](https://github.com/Breakthrough/PySceneDetect/blob/7cd1005e8349d8e913d0930fc1750363eb4ddc51/scenedetect/detectors/adaptive_detector.py#L137)) | `3.0`, `window_width=2`, `min_content_val=15.0` |
| `ThresholdDetector` | Mean of all pixel values against a threshold; tracks fade-out and fade-in, and places the cut at `f_out + duration * (1 + fade_bias) / 2` ([threshold_detector.py:138, 167-168](https://github.com/Breakthrough/PySceneDetect/blob/7cd1005e8349d8e913d0930fc1750363eb4ddc51/scenedetect/detectors/threshold_detector.py#L167-L168)) | `threshold=12`, `fade_bias=0.0` |
| `HistogramDetector` | Y-channel histogram, `cv2.HISTCMP_CORREL` against the previous frame; cut when correlation <= `1 - threshold` ([histogram_detector.py:35-36, 98](https://github.com/Breakthrough/PySceneDetect/blob/7cd1005e8349d8e913d0930fc1750363eb4ddc51/scenedetect/detectors/histogram_detector.py#L98)) | `threshold=0.20`, `bins=128` |
| `HashDetector` | Perceptual hash: grey, resize, DCT, keep the low frequencies, compare with the median; normalised Hamming distance ([hash_detector.py:49-51](https://github.com/Breakthrough/PySceneDetect/blob/7cd1005e8349d8e913d0930fc1750363eb4ddc51/scenedetect/detectors/hash_detector.py#L49-L51)) | `threshold=0.35`, `size=8`, `lowpass=2` |
| `FlashFilter` | Enforces a minimum scene length. `MERGE` folds rapid cuts into one, `SUPPRESS` drops cuts within the length of the last one ([detector.py:106-114](https://github.com/Breakthrough/PySceneDetect/blob/7cd1005e8349d8e913d0930fc1750363eb4ddc51/scenedetect/detector.py#L106-L114)) | `MERGE` |

The published docs ([scenedetect.com detectors page](https://www.scenedetect.com/docs/latest/api/detectors.html)) list other defaults for `HistogramDetector` (0.05, 256 bins) and `HashDetector` (0.395, size 16) than the source at `main`. Trust the source of the version you compare against.

What this says about the app:

- `hardcut.rs` is in effect `AdaptiveDetector` on luma only, with a wider window (6 against 2) and an absolute floor (12 of 255). That is a proven design.
- Despite its name, `FlashFilter` does not detect a flash of light. It only enforces scene length, which `hardcut.rs:243-262` and `scene_length.rs` already do.
- Luma alone misses a cut between two shots of similar brightness layout and different colour. `ContentDetector` adds hue and saturation for this, and `HistogramDetector` is a cheap luma-only alternative that ignores where pixels are. See recommendation 10.

### Learned detectors: TransNet V2

- The paper ([arXiv 2008.04838](https://arxiv.org/abs/2008.04838), HTML version [here](https://arxiv.org/html/2008.04838)) resizes frames to 48x27, feeds 100-frame windows and scores the middle 50. It reports F1 of 77.9 on ClipShots, 96.2 on BBC Planet Earth and 93.9 on RAI (Table 2). Training took about 17 hours on one V100 GPU.
- The reference inference extracts frames with ffmpeg as `rawvideo`, `rgb24`, `48x27` and thresholds predictions at 0.5 ([TN@85cef72 inference/transnetv2.py:16, 83-92](https://github.com/soCzech/TransNetV2/blob/85cef72af9a916bdfd7cc94a670c9cdfbf12d1ed/inference/transnetv2.py#L83-L92)). The weights ship as a TensorFlow SavedModel; a PyTorch port is in `inference-pytorch`. MIT licence ([repo](https://github.com/soCzech/TransNetV2)).
- Fit for this app: the hard-cut scan already decodes every frame to a tiny thumbnail through one pipe. Changing it to 48x27 `rgb24` costs little. The hard part is running the model from Rust. The `ort` crate wraps ONNX Runtime ([crates.io/crates/ort](https://crates.io/crates/ort)), but the project publishes no ONNX file, so the model would need converting and checking against the reference outputs. ONNX Runtime is a native library for every target in CI.
- **Unverified:** inference speed on a CPU. The paper gives no CPU figures. Measure before promising "fast".
- Value: the model is trained on gradual transitions (dissolves, wipes), which no rule here detects. Cost: a model file, a native runtime on three systems, and a scan whose thresholds are no longer ours to explain. Treat it as a later, optional scan mode.

### Hard cases, and what the app should do about each

- **Fades to black.** Handled: the black screen sits between fade-out and fade-in, and the cut goes on the keyframe nearest to it, preferring one inside (`scenes.rs:143-156`). This matches `ThresholdDetector` with `fade_bias` near 0.
- **Dissolves and fades without black.** Not detected. No rule here sees them. Options: a learned model (above), or a second pass that sums small changes over a window. **Unverified:** I did not trace the original paper for the "twin comparison" method, so I give no citation.
- **Flashes (camera flash, lightning, muzzle fire).** A one-frame flash gives two strong changes in a row: into the flash and out of it. In `candidates_among` (`hardcut.rs:206-220`) each spike sits among 12 neighbours of which only one is large, so both can pass the 3x ratio. Then the strongest-first thinning (`hardcut.rs:255`) can prefer a bright flash to a real cut nearby. Recommendation 6: keep the thumbnail from two frames back, and drop a candidate when frame N-1 and frame N+1 differ little. This is a pure check, easy to test with a synthetic white frame.
- **Letterboxing and pillarboxing.** Bars do not stop a frame being black, since they are black too. They do dilute the mean difference: a 2.39:1 film in a 16:9 frame has about a quarter of its pixels in bars, so every change score drops by about that share, and "Most changes" detects fewer cuts in such films. Recommendation 7: find the static dark border once per video from the thumbnails (as `cropdetect` does with its edge scan), then measure change only inside it. Pure code.
- **Logos and burnt-in subtitles on black.** Handled by the 98% share, and `black.rs` has a test for a small bright logo.
- **Credits.** White text on black stays under the 2% bright share only for sparse credits. Rolling credits look like slow motion and do not trip the hard-cut ratio. No change needed.
- **Interlaced content.** `idet` can tell. A cut that falls between the two fields of one frame makes one combed frame, with two medium changes instead of one strong one. **Unverified** how often this costs a cut in practice. Add a synthetic interlaced test (`tinterlace`, [vf_tinterlace.c](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_tinterlace.c)) before spending more.
- **Telecine (24 to 30 pulldown).** The prefilter already guards against the tiny repeat packets (`windows.rs:32-38`). In the hard-cut scan a repeated frame has near-zero change, which lowers the neighbour mean and makes the ratio a little more sensitive. A test video made with the `telecine` filter ([vf_telecine.c](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_telecine.c)) would show whether that matters.
- **HDR (PQ or HLG).** **Unverified:** how the `format=gray` conversion maps 10-bit PQ to 8 bits, and what luma 32 means there. Black stays near the bottom code value in any transfer, so black screens should still be found. Dark scenes sit at higher code values in PQ than in SDR, which could make the dark share stricter. Read `color_transfer` with ffprobe, and add a test clip tagged `smpte2084` before tuning anything.
- **Variable frame rate.** This is a correctness bug today. See section 1b.

### 1b. The scan can attach a verdict to the wrong frame

`scan.rs:162-165` states that the nth thumbnail is the nth frame of the window, and `scan.rs:180-188` asks for `-f rawvideo` without `-fps_mode`. In ffmpeg, the default `fps_mode auto` "chooses between cfr and vfr depending on muxer capabilities" ([ffmpeg.html, -fps_mode](https://ffmpeg.org/ffmpeg.html)). The code picks CFR for any muxer without `AVFMT_VARIABLE_FPS` ([FF@045dea2 fftools/ffmpeg_mux_init.c:850-852](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/fftools/ffmpeg_mux_init.c#L850-L852)), and the `rawvideo` muxer sets only `AVFMT_NOTIMESTAMPS` ([libavformat/rawenc.c:591-599](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavformat/rawenc.c#L591-L599)). In CFR mode, "frames will be duplicated and dropped to achieve exactly the requested constant frame rate" (same manual page).

So on a variable-frame-rate video (phones, screen recordings, some Matroska files) the scan gets duplicated or missing thumbnails, and every later verdict in the window lands on the wrong packet time. The hard-cut scan uses the same decode (`hardcut.rs:171`), so its times shift too. The synthetic test videos are constant rate, which is why no test catches it.

Fix (recommendation 2): add `PictureTimesOption::for_version(...).arguments()` (`ffmpeg.rs:102-135`), which already exists for exact cuts, to the scan command. Test with a generated VFR video, for example one built with `setpts` so that frame spacing changes halfway, and a black screen after the change.

### 1c. Packets that are decoded but never shown

ffprobe prints `D` in the flags for `AV_PKT_FLAG_DISCARD` ([fftools/ffprobe.c:1462](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/fftools/ffprobe.c#L1462)), defined as packets "required to maintain valid decoder state but ... not required for output" ([libavcodec/packet.h:660-664](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavcodec/packet.h#L660-L664)). The MP4 demuxer sets it on frames that an edit list cuts away ([libavformat/mov.c:4711, 12667](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavformat/mov.c#L4711)). Such files are common: anything trimmed without encoding, including scene files from this app or LosslessCut.

`probe.rs:245-260` keeps those packets in the frame index (the test at `probe.rs:474` only clears the keyframe flag). The decoder does not output them, so a window that contains them gets fewer thumbnails than packets, and the verdicts shift. The README already says exact cuts refuse such files (README "When a video cannot be cut exactly"). Recommendation 4: drop `D` packets from the index used for thumbnails, or keep them with a flag that the window-to-frame mapping skips. **Unverified** for files where the discarded frames sit later than the first window; test it with a scene file the app wrote itself.

### 1d. Silence as extra evidence

Television breaks often pair black with silence. `silencedetect` gives the rule (-60 dBFS for 2 s by default). A cheap pure version: decode audio only in the windows the black scan already decodes, at a low sample rate, and record whether a black screen has silence under it. Use it to rank black-screen cuts when the minimum scene length thins them, not to make cuts by itself. Low priority.

---

## 2. Splitting without re-encoding

### What the app does today

One pass of the segment muxer: `-map 0:v:0 -map 0:a? -c copy -f segment -segment_times ... -reset_timestamps 1`, with cut times asked for 1 ms early (`split.rs:27`, `split.rs:500-515`, `ffmpeg.rs:41`). Pictures without a show time get their decode time through the `setts` bitstream filter (`ffmpeg.rs:199-275`). Cuts go to the keyframe nearest each black screen (`scenes.rs:92-106`, `scenes.rs:143-156`) or are nudged between keyframes (`plan.rs:788-800`). Exact cuts encode the pictures around each cut with libx264 and copy the rest (`exact_cut.rs`, `exact_split.rs`).

### Keyframes and the segment muxer

- The docs say "every segment starts with a keyframe of the selected reference stream" and that splitting "may not be accurate, unless you force the reference stream key-frames at the given time" ([ffmpeg-formats.html, segment](https://ffmpeg.org/ffmpeg-formats.html#segment_002c-stream_005fsegment_002c-ssegment)).
- The source starts a new file when a packet of the reference stream with the key flag arrives, in the order packets are read (decode order), and its pts has reached the cut ([libavformat/segment.c:936-944](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavformat/segment.c#L936-L944)). With `reset_timestamps`, every packet of the new file then has the keyframe's pts subtracted (lines 985-990).

### Open GOP: a gap in the copy split

In an open GOP, pictures stored after a keyframe can be shown before it, and they reference the previous GOP. The exact-cut planner knows this: `exact_cut.rs:995-1000` refuses to copy from such a keyframe, and the README says "A copy only starts on a keyframe that the pictures stored after it do not reach back across." The normal copy split does not: `scenes.rs:143-156` and `plan.rs:598`, `plan.rs:788-800` use every keyframe that `probe.rs` lists.

From the segment muxer source above, the leading pictures of an open-GOP keyframe are read after it, so they land in the new scene file with negative times, before its first picture. The previous scene then ends a few pictures short, and in the new file those pictures cannot decode because their references are in the other file. **Unverified:** what a player shows (most likely nothing, since the MP4 muxer writes an edit list that starts at the keyframe, [movenc.c:4277](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavformat/movenc.c#L4277)), and whether both scenes lose them. The existing `TestVideo::with_open_picture_groups` (`test_videos.rs:75-86`) can show it in one lab run: split at a later keyframe, then count the decoded pictures in each scene against the original.

The verification in section 3 would not catch it either: every packet is still present, just in the wrong file.

Recommendation 3: keep `dts_time`, which `probe.rs:56` already asks for and `probe.rs:253` drops. Mark a keyframe usable for a copy only when no packet stored after it is shown before it. Reuse the rule in `exact_cut.rs` (`is_copy_point`) so the two splits cannot disagree. Videos with no usable keyframe near a black screen then fall back to the nearest usable one, with a note.

### Timestamps, edit lists and audio priming

- `-ss` before `-i` seeks to "the closest seek point before position". When transcoding, the extra part is decoded and dropped (`-accurate_seek` is on by default). With stream copy it is kept ([ffmpeg.html, -ss](https://ffmpeg.org/ffmpeg.html)). `-copyts` keeps input timestamps "without trying to sanitize them", and `-start_at_zero` shifts them to start at zero (same page).
- `-avoid_negative_ts` has `make_non_negative`, `make_zero`, `auto` (default, "enables shifting when required by the target format") and `disabled` ([ffmpeg-formats.html, format options](https://ffmpeg.org/ffmpeg-formats.html)).
- An MP4 edit list entry maps movie time to media time. A media time of -1 is an empty edit, and "the last edit in a track should never be an empty edit" ([Apple QuickTime File Format, edit list table](https://developer.apple.com/documentation/quicktime-file-format/edit_list_atom/edit_list_table)). ffmpeg's MP4 muxer writes edit lists by default (`use_editlist` auto, [movenc.c:123](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavformat/movenc.c#L123)) and warns "Not writing any edit list even though one would have been required" when they are off (line 4507). The ISO/IEC 14496-12 text is paywalled; I used Apple's free QuickTime spec, which defines the same box.
- AAC encoders add priming samples. Apple's note says 2112 samples is the common delay, a player must discard them, and without that "the audio will be 2112 samples behind the video" ([Apple TN2258](https://developer.apple.com/library/archive/technotes/tn2258/_index.html)). The same note says MP4 has "no satisfactory and explicit signaling mechanism" for it, so players differ. This matters to scene files because each new AAC scene starts mid-stream: its first packet decodes with no priming, and a player that trims 2112 samples would cut real sound. **Unverified** for ffmpeg-written scenes; worth one lab test comparing the decoded audio length of a scene with the matching stretch of the original.
- Matroska: `TimestampScale` defaults to 1,000,000 ns, so times are whole milliseconds. `CodecDelay` "MUST be subtracted from each frame timestamp", and `SeekPreRoll` is how much "the decoder MUST decode before the decoded data is valid" after a discontinuity (Opus uses both) ([Matroska elements](https://www.matroska.org/technical/elements.html)). A scene file of Opus sound starts at such a discontinuity.

### Smart cut, as other tools do it

- LosslessCut checks whether the cut is on a keyframe, else finds the next keyframe within 10 s, then 60 s ([LC@b83d4df src/renderer/src/smartcut.ts:13-47](https://github.com/mifi/lossless-cut/blob/b83d4df9ccd30a701cc5442624250d58ff964349/src/renderer/src/smartcut.ts#L13-L47)). It encodes the head with the source codec at 1.2 times the source bitrate (lines 62-78), copies from the keyframe, and joins the two with the concat demuxer. It ends the encoded head one frame before the keyframe "so we don't end up with duplicates" ([useFfmpegOperations.ts:709-725](https://github.com/mifi/lossless-cut/blob/b83d4df9ccd30a701cc5442624250d58ff964349/src/renderer/src/hooks/useFfmpegOperations.ts#L709-L725)). It uses `-avoid_negative_ts make_zero` only when the start is cut (lines 299-303). The maintainer lists the limits in [issue #126](https://github.com/mifi/lossless-cut/issues/126): works on some H.264 files and few H.265 files, one video stream only, and audio glitches or desync at the join.
- skeskinen/smartcut recodes only around the cut points through PyAV and claims H.264, H.265, VP9 and AV1 ([repo](https://github.com/skeskinen/smartcut)). Its README says it is no longer updated.
- This app's exact cut already goes further than LosslessCut: it encodes the tail too, matches profile, level, pixel format and colour fields, keeps open-GOP keyframes out of copies, and proves the result picture by picture (`exact_verify.rs:1-32`). Nothing to copy from these tools. Extending exact cuts to HEVC has a poor record elsewhere (issue #126), so leave it.

---

## 3. Reliability of video software

### What the app does today

Every run goes through one wrapper with a time limit and a typed error (`ffmpeg.rs:23-32`, `ffmpeg.rs:294-350`), and a stuck run is stopped with its child processes (`ffmpeg.rs:697-711`). The downloaded ffmpeg is pinned and checked against built-in SHA-256 checksums (`tools.rs:1-9`, README "ffmpeg"). Scenes are written into `unfinished_<n>` and renamed into place only when all exist (`split.rs:298-347`). Version-dependent flags are chosen by asking ffmpeg (`ffmpeg.rs:102-197`). Tests generate their own videos with lavfi sources (`test_videos.rs:117-118`, `test_videos.rs:255-258`, `test_videos.rs:329-331`). This is a strong base. The gaps are below.

### 3a. Verify copies against the original (recommendation 1)

`verify.rs:10-12` says plainly that the check "does not compare the scenes with the original", and the README says the same ("Verification before recycling"). The check reads each scene to its end and decodes 2 seconds (`verify.rs:155-178`, `verify.rs:208-227`), and `export.rs:831-847` then sends the original to the Recycle Bin. So a split that loses packets, puts them in the wrong file, or cuts a scene short but well-formed passes, and the original goes.

ffmpeg already has the tool. `framecrc` "computes and prints the Adler-32 CRC for each audio and video packet", and `framemd5` and `framehash` do the same with MD5 or SHA-256; `streamhash` gives one hash per stream ([ffmpeg-formats.html, framecrc / framehash / framemd5 / streamhash](https://ffmpeg.org/ffmpeg-formats.html#framecrc)). With `-c copy` the muxer receives the compressed packets, so the hash covers the stored bytes.

The app already runs `framecrc` with `COPY_SCENE_STREAMS` on every scene (`verify.rs:158-163`) and parses its lines (`verify.rs:191-200`), but throws the CRC away. One more pass over the original with the same streams gives the reference. The check: per stream, the scenes in order hold the same sequence of (size, CRC) as the original, and the first video packet of each scene is the keyframe the plan chose. Cost: one read of the original, about as long as the copy itself.

Caveats to settle in the lab first:

- Times differ by design (`reset_timestamps`), so compare sizes and CRCs, not times.
- Some muxers change packet bytes. **Unverified:** the MPEG-TS muxer may add access unit delimiters to H.264, and moving H.264 between Annex B and MP4 length prefixes changes bytes. Scenes keep the original's container, so this may never happen here, but test each container the app writes (MP4, MOV, MKV, AVI, TS).
- Audio packets at a cut: compare the concatenation of all scenes, so a packet that moved across a cut does not fail it. Report a moved packet separately if that matters.

This turns "the files play" into "the files are the original", and it is the last gate before the original goes.

### 3b. Atomic rename and durability (recommendation 5)

- POSIX `rename` must be atomic when the target exists: "a directory entry named new shall remain visible to other threads throughout the renaming operation", and it fails with `EXDEV` across file systems ([POSIX rename](https://pubs.opengroup.org/onlinepubs/9799919799/functions/rename.html)).
- Rust's `std::fs::rename` uses `rename` on Unix and `MoveFileExW` with a fallback to `SetFileInformationByHandle` on Windows; it fails when the paths are on different file systems ([std::fs::rename](https://doc.rust-lang.org/std/fs/fn.rename.html)). `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` replaces the target, and only `MOVEFILE_WRITE_THROUGH` waits until the move is on disk ([MoveFileExW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)). Microsoft's page makes no promise of atomicity.
- On Linux, "calling fsync() does not necessarily ensure that the entry in the directory containing the file has also reached disk. For that an explicit fsync() on a file descriptor for the directory is also needed" ([fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html)).
- Rust's `File::sync_all` calls `fcntl(F_FULLFSYNC)` on Apple systems and `fsync` elsewhere ([rust-lang/rust library/std/src/sys/fs/unix.rs, fn fsync](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/unix.rs)).

The app syncs the downloaded ffmpeg before renaming it (`tools.rs:428`), but not the scene files (`split.rs:566-580`) or the scan cache (`cache.rs:207-213`). ffmpeg writing a file and exiting does not mean it reached the disk. **Unverified:** I did not trace whether ffmpeg's file protocol ever calls `fsync`. The original goes to the Recycle Bin, not away for good, so a power cut is not total loss. Still, the rule "scenes are safe before the original moves" should hold on disk, not only in the page cache. Fix: open each finished scene and call `sync_all` before the rename, and on Unix sync the folder after the renames. The cache needs none of this; a lost cache entry costs a rescan.

- The Recycle Bin path: the `trash` crate 5.2.9 deletes on Windows with `FOF_NO_UI | FOF_ALLOWUNDO | FOF_WANTNUKEWARNING` (`trash-5.2.9/src/windows.rs:97`). Microsoft documents `FOF_ALLOWUNDO` as "preserve undo information, if possible" and `FOF_WANTNUKEWARNING` as a warning "if a file or folder is being destroyed during a delete operation rather than recycled", which "partially overrides FOF_NOCONFIRMATION" ([IFileOperation::SetOperationFlags](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-setoperationflags)). **Unverified:** whether a file on a network share or a drive without a Recycle Bin is then destroyed silently under `FOF_NO_UI`. CLAUDE.md already lists this as untested. Until it is tested, consider keeping the original when the video is not on a local fixed drive.

### 3c. ffprobe output as the source of truth

- ffprobe's JSON and XML writers "omit the printing of fields with invalid or non-applicable values", and only the XML output has a schema (`ffprobe.xsd`) ([ffprobe.html](https://ffmpeg.org/ffprobe.html)). There is no stated stability promise for CSV column order.
- The app reads packets as positional CSV and relies on ffprobe printing fields in a fixed order (`probe.rs:243-250`), with a test for extra side-data columns (`probe.rs:460`). This is fast and works across the four versions it meets. Two cheap improvements: use `-of compact=p=0` (key=value pairs) or JSON for the small queries (streams, format), so a missing field is an error and not a shifted column; and fail clearly on a packet flagged `C` (corrupt, [ffprobe.c:1463](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/fftools/ffprobe.c#L1463)), or at least warn before a split, since a corrupt packet in the original is copied as it is.
- `-count_packets` gives a per-stream packet count without listing them (same page). That is a cheap cross-check on the total the verification in 3a sees.

### 3d. Corrupt or truncated input

- ffmpeg's `-xerror` stops on error, and `-max_error_rate` makes ffmpeg exit with code 69 when the share of failed decodes crosses a threshold, default 2/3, without stopping processing ([ffmpeg.html](https://ffmpeg.org/ffmpeg.html)). `-abort_on empty_output_stream` fails a run that wrote nothing to one of its streams (same page).
- The app uses `-xerror` when reading scenes back (`verify.rs:159`), and reports undecoded stretches in the scan instead of failing it (`scan.rs:91-110`). That is the right split between "warn" and "stop". Two additions: `-abort_on empty_output_stream` on the split, so a scene with no video or no sound packets fails at once; and a lower `-max_error_rate` for the scan decode, so a badly damaged file is reported as damaged and not as "no black screens".

### 3e. Testing with synthetic video and fuzzing

- ffmpeg's own regression suite, FATE, runs locally and on volunteer machines and can regenerate references with `GEN=1` ([fate.html](https://ffmpeg.org/fate.html)). The app's `#[ignore]` end-to-end tests are the same idea at a smaller scale.
- Test videos worth adding to `test_videos.rs` (recommendation 8), each from lavfi sources and filters that exist in ffmpeg's source tree: variable frame rate (`setpts`); an MP4 with an edit list that discards leading frames (trim a test video by copy); a one-frame white flash (`color=white` for one frame); a fade to black without a cut (`fade`, [vf_fade.c](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/libavfilter/vf_fade.c)); letterboxing (`pad`); interlaced (`tinterlace`); telecine (`telecine`); full range (`-color_range pc`); an HDR tag (`-color_trc smpte2084`); MPEG-TS with a start offset; AAC sound, to check priming at scene starts.
- ffmpeg is fuzzed itself, with libFuzzer targets for decoders and demuxers ([tools/target_dec_fuzzer.c](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/tools/target_dec_fuzzer.c), [tools/target_dem_fuzzer.c](https://github.com/FFmpeg/FFmpeg/blob/045dea2a34ef93f854f3f4a30ddf8f77b35b34ce/tools/target_dem_fuzzer.c)) in Google's OSS-Fuzz ([projects/ffmpeg](https://github.com/google/oss-fuzz/tree/master/projects/ffmpeg)). The app should not fuzz ffmpeg. It should fuzz its own parsers of ffmpeg's output: `probe::parse_packet_line`, `verify::parse_packet_line`, the chapter JSON, the progress lines, the version banner (recommendation 9). `cargo-fuzz` runs libFuzzer ([Rust Fuzz Book](https://rust-fuzz.github.io/book/cargo-fuzz.html)); a property-test crate is lighter and runs in the normal `cargo test` on all three systems. **Unverified:** whether `cargo-fuzz` builds on the Windows CI runner; the book has a separate Windows chapter.

### 3f. Pinning ffmpeg

The app pins one build per system and checks its checksum, but the builds are three different versions (6.0, 6.1.1, 7.0.2) and the PATH may hold 4.4 or 8 (`tools.rs:1-9`, ffmpeg-craft notes). Every behaviour this document cites can change between them: the ffmpeg manual describes the newest version only. Two habits keep this honest:

- Run the `#[ignore]` end-to-end tests in CI against each pinned build, not only against the runner's packaged ffmpeg.
- Record `ffmpeg -version` in each scan cache entry and split report, so a bug report says which ffmpeg made the files.

---

## Sources not reached

- ISO/IEC 14496-12 (ISO base media file format) is paywalled. Edit list facts come from Apple's QuickTime File Format spec, which defines the same box.
- The ffmpeg filters manual is too long for the fetch tool to read past about 500,000 characters. Filter facts come from the libavfilter source at the pinned commit, which is the higher authority anyway.
- No primary source found for CPU inference speed of TransNet V2, or for an official ONNX export.
