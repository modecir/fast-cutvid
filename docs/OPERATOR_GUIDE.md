# fastCutVid operator guide

This guide is for people using the desktop application to assemble and cut video. fastCutVid intentionally covers only the cutting stage: importing sources, arranging them in sequence, trimming, reviewing audio activity, and exporting either a simple video or a reusable edit decision list.

## Before starting

1. Download the package for your platform from [GitHub Releases](https://github.com/modecir/fast-cutvid/releases/latest), or build it with `cargo run --release`.
2. Install FFmpeg and FFprobe and make sure both commands are available on `PATH`. They provide metadata, timeline frames, waveforms, and final export.
3. Keep source videos on a local or reliably mounted drive. fastCutVid never modifies the source files.

If the commands are installed somewhere else, launch fastCutVid with `FASTCUT_FFMPEG` and `FASTCUT_FFPROBE` set to their executable paths.

## Install version 0.1.3

Download a platform archive from [release 0.1.3](https://github.com/modecir/fast-cutvid/releases/tag/v0.1.3), rather than GitHub's source-code archives. Keep the extracted support files with the application. No Rust installation is needed for downloaded builds.

### macOS 12 or newer

Choose `macos-arm64` for Apple silicon or `macos-x86_64` for Intel, extract the archive, and move `fastCutVid.app` to Applications. Install FFmpeg using your preferred package manager; with Homebrew, run `brew install ffmpeg`.

The first release is ad-hoc signed and not Apple-notarized. After attempting to open a trusted download, macOS may offer **Open Anyway** in **System Settings → Privacy & Security**. Do not disable Gatekeeper globally.

Finder-launched apps may not inherit your terminal's `PATH`. If import/export cannot find the media tools, launch the executable from Terminal with explicit paths. With Homebrew installed:

```bash
FASTCUT_FFMPEG="$(brew --prefix)/bin/ffmpeg" \
FASTCUT_FFPROBE="$(brew --prefix)/bin/ffprobe" \
/Applications/fastCutVid.app/Contents/MacOS/fast-cutvid
```

### Windows x86-64

Extract the ZIP to a writable directory, install a Windows FFmpeg build from the options on the [FFmpeg download page](https://ffmpeg.org/download.html), and add the directory containing `ffmpeg.exe` and `ffprobe.exe` to `PATH`. Restart the app after changing `PATH`. Run `fast-cutvid.exe`; this early release is unsigned and may show a Windows reputation prompt.

Alternatively, launch from PowerShell with explicit paths (adjust them to your installation):

```powershell
$env:FASTCUT_FFMPEG = "C:\Tools\ffmpeg\bin\ffmpeg.exe"
$env:FASTCUT_FFPROBE = "C:\Tools\ffmpeg\bin\ffprobe.exe"
.\fast-cutvid.exe
```

### Linux x86-64

The binary is built on Ubuntu 22.04 and needs a compatible system with glibc 2.35 or newer and a graphical desktop for the GUI. On Ubuntu 22.04 install runtime dependencies with `sudo apt install ffmpeg libgtk-3-0 libxkbcommon0 libx11-xcb1 libxcb-render0 libxcb-shape0 libxcb-xfixes0 libwayland-client0 libegl1 libgl1 libvulkan1`. Distribution package names can differ. Extract the tarball and run `./fast-cutvid` from its directory. Headless validation/rendering does not open a window.

### Verify a download

Download both the archive and its `.sha256` file into the same directory. On macOS use `shasum -a 256 -c ARCHIVE.tar.gz.sha256`; on Linux use `sha256sum -c ARCHIVE.tar.gz.sha256`. On Windows compare `(Get-FileHash .\ARCHIVE.zip -Algorithm SHA256).Hash` with the hash in the checksum file. Substitute the actual filename.

### Platform limitations

macOS uses native AVFoundation for video and audio preview. Windows/Linux preview is silent in 0.1.3; the waveform and MP4 export still include audio. There is no undo/redo or autosave yet. Save your JSON before replacing a project or closing the app. A saved project references its videos and does not bundle them.

For Codex, Claude, and other automation tools, see [local agent setup](LOCAL_AGENTS.md).

## Start a cut

1. Launch fastCutVid.
2. Select **+ Import**, press `Cmd/Ctrl+I`, or drop one or more video files onto the window.
3. Media appears immediately. Timeline frames and waveform sections appear as they are decoded, independently of each other. Reopening unchanged media reuses cached analysis. The status bar reports when analysis is complete.
4. Double-click an item in Media or select **+ Timeline** to append the whole source to the end of the sequence.

The preview and timeline frames follow the video’s original display shape, including portrait, square, and rotated footage. Background analysis uses a limited number of workers to keep editing responsive. Switching projects cancels pending analysis for the previous project.

Analysis is stored in the system cache directory under `fastCutVid/analysis-v1` (`~/Library/Caches` on macOS, `%LOCALAPPDATA%` on Windows, and `$XDG_CACHE_HOME` or `~/.cache` on Linux). Set `FASTCUT_CACHE_DIR` to choose a different base folder. Cache entries are rebuilt when source size or modification time changes; unreadable entries are regenerated automatically. Old entries are pruned toward 256 MiB when opening the app or another project. The cache can be deleted safely.

Dropping a valid `.fastcut`, `.fastcut.json`, or `.json` file opens that complete project instead of adding it to the current cut. Save the current project before opening another timeline if its changes matter.

The app also accepts files at launch: `fast-cutvid "edit.fastcut"` opens a project, and `fast-cutvid "clip one.mp4" "clip two.mov"` imports videos. You can combine one project with videos; the project opens first, then videos enter its media bin without being added to the sequence. See [launch examples](../README.md#open-files-at-launch) for macOS and Cargo commands. Missing files, unsupported extensions, and multiple projects are rejected; invalid project contents are reported in the app without importing the additional videos.

## Navigate and review

- Click-drag anywhere inside a clip to scrub the playhead.
- Press `Space` or click the program monitor to play and pause.
- Press `Left` or `Right` to jump one second.
- Press `Shift+Left` or `Shift+Right` to move one source frame.
- Press `Up` or `Down` to move to the previous or next edit.
- Pinch over the timeline or use its Zoom control to change scale.
- Read the upper clip lane as video frames and the lower lane as audio amplitude. Quiet waveform areas usually indicate pauses, but always review playback before cutting.

Press `?` for every keyboard shortcut. Press `F1` for the in-app quick start.

On macOS, the native menu bar exposes the same actions without requiring shortcuts:

- **File:** Import videos, open and save timelines, export cuts, and render video.
- **Edit:** Split, delete, reorder, and mute the selected clip.
- **Playback:** Play or pause, move by seconds or frames, and jump between edit points.
- **View:** Zoom the timeline in, out, or fit the complete sequence.
- **Window** and **Help:** Standard macOS window controls, quick start, and shortcut reference.

## Edit the sequence

- **Trim:** Drag the left or right edge of a clip. The inspector can also edit exact source-in and source-out seconds.
- **Split:** Position the playhead inside a clip and press `S`, `Cmd/Ctrl+K`, or the Split button.
- **Delete:** Select a clip and press `Delete` or `Backspace`.
- **Reorder:** Select a clip and press `Option/Alt+Left` or `Option/Alt+Right`, or use the inspector arrows.
- **Audio:** Select a clip to mute it or change its linear gain in the inspector. Press `M` to toggle mute.

Clips are always consecutive. fastCutVid v1 does not create gaps, overlaps, transitions, titles, effects, or independent tracks.

## Save and reopen

- **Save** writes the editable `fastcut.timeline/v1` JSON project. Use `Cmd/Ctrl+S` for normal saving and `Cmd/Ctrl+Shift+S` to choose a new file.
- **Open** or `Cmd/Ctrl+O` opens a saved timeline. You can also drop its JSON onto the window.
- Media paths in JSON may be absolute or relative to the JSON file. For a portable project, keep the JSON and media under one folder and use relative paths.

If the status bar reports missing media, restore the files at the recorded paths or correct the asset paths in the JSON before reopening it. Automatic relinking is not yet available.

## Deliver the cut

- **Export cuts** saves agent-readable timeline JSON for another person, tool, or AI agent to inspect and render.
- **Export cuts** defaults to the original project's folder and basename plus `-cutted.fastcut`. Without a saved project, it uses the first timeline clip's source video (or the first media asset if the timeline is empty). For example, `Interview.mov` suggests `Interview-cutted.fastcut` beside the video. You can change the name or folder in the dialog. Exporting writes a copy without changing the current project path or clearing unsaved edits; **Save** and **Save As** are unchanged.
- **Export video** renders the sequence to MP4 using the resolution, frame rate, and codecs stored in the project.

Export to a new destination; do not choose a source-media path. Wait for the status bar to report completion before moving or closing the output.

## Troubleshooting

- **FFmpeg or FFprobe not found:** Confirm both commands work in a terminal, or set `FASTCUT_FFMPEG` and `FASTCUT_FFPROBE` before launching.
- **Frames or waveform are still blank:** Background analysis may still be running. Check the status bar.
- **A JSON project will not open:** Run `fast-cutvid --validate PROJECT.fastcut` or the equivalent Cargo command to see the structural error.
- **A source is missing:** Relative paths resolve from the JSON file's directory. Check that relationship first.
- **macOS blocks an early unnotarized build:** Confirm it came from the official GitHub release, then use Finder's **Open** command to review the system prompt. Fully signed and notarized installers remain planned.
