# fastCutVid

![fastCutVid logo](assets/fastcutvid-logo-readme.png)

Website: [fastcutvid.com](https://fastcutvid.com)

fastCutVid is a native, open-source video cutter for macOS, Windows, and Linux. It combines a focused video-editing timeline with a portable JSON cut format that humans and AI agents can render in exactly the same way.

There is no HTML, JavaScript, Electron, or webview. The interface is Rust/egui and media processing is delegated to FFmpeg.

## Project scope

fastCutVid is one focused stage of an editing process: assembling source videos, finding the useful moments, and cutting them into sequence. It is deliberately not a full finishing suite. Effects, transitions, titles, color grading, compositing, animation, and advanced audio mixing are outside its scope.

That narrow scope is intentional. fastCutVid is designed to open quickly, keep preview and timeline interaction responsive, and produce either a finished simple cut or a precise JSON edit decision list that another editor or AI agent can take into the finishing and release stages.

## Download

Published builds are available from [GitHub Releases](https://github.com/modecir/fast-cutvid/releases/latest):

- macOS app for Apple silicon
- macOS app for Intel Macs
- Windows x86-64 executable package
- Linux x86-64 binary package

FFmpeg and FFprobe must currently be installed separately and available on `PATH`; they are used for media analysis and final export. Early macOS builds are ad-hoc signed but not Apple-notarized, and Windows/Linux packages are not yet installer-signed.

**0.1.2 preview support:** macOS provides synchronized video/audio playback. Windows and Linux currently provide silent FFmpeg video preview; exported MP4 files include audio. See the [installation guide](docs/OPERATOR_GUIDE.md), [release notes](docs/releases/v0.1.2.md), and [changelog](CHANGELOG.md) for setup, limitations, and changes.

## Current MVP

- Native multi-video import and metadata probing
- Native drag-and-drop for video files and complete timeline JSON projects
- GPU-rendered desktop UI with media bin, program monitor, timeline, and inspector
- Native macOS menu bar with File, Edit, Playback, View, Window, and Help commands
- Aspect-safe, letterboxed native video preview with playback and seeking
- Native AVFoundation video/audio playback on macOS with one shared media clock
- Filmstrip timeline with source-aware frame sampling and speech-visible waveforms
- Progressive non-blocking imports with concurrent frame and audio analysis
- Trackpad pinch-to-zoom when the timeline is hovered or focused
- Split, trim, delete, and clip reordering
- Non-destructive source in/out points and per-clip audio controls
- Project/cut export as readable `fastcut.timeline/v1` JSON
- MP4 rendering with normalized resolution, frame rate, and audio
- Headless renderer for agents and automation

## Usage guides

- [Operator guide](docs/OPERATOR_GUIDE.md): installing prerequisites, importing media, assembling and trimming a cut, saving, exporting, and troubleshooting.
- [Agent guide](docs/AGENT_GUIDE.md): inspecting sources, generating valid timelines, validation, headless rendering, and handing work back to an operator.
- [Codex, Claude, and local agents](docs/LOCAL_AGENTS.md): setup with downloaded binaries, copyable prompts, Claude Cowork handoffs, and command examples for each platform.
- [Timeline schema](docs/timeline.schema.json): canonical machine-readable `fastcut.timeline/v1` contract.

Inside the app, press `F1` for the operator quick start or `?` for the complete shortcut reference.

## Run locally

Install [Rust](https://rustup.rs/) and [FFmpeg](https://ffmpeg.org/download.html), then:

```bash
cargo run --release
```

If FFmpeg is not on `PATH`, point fastCutVid to the binaries:

```bash
FASTCUT_FFMPEG=/path/to/ffmpeg FASTCUT_FFPROBE=/path/to/ffprobe cargo run --release
```

## Open files at launch

Pass a project or one or more videos as positional arguments (available since 0.1.1):

```bash
fast-cutvid "edit.fastcut.json"
fast-cutvid "first clip.mp4" "second.mov"
fast-cutvid "edit.fastcut.json" "extra footage.mp4"
# From a source checkout:
cargo run --release -- "edit.fastcut.json"
# Installed macOS app:
/Applications/fastCutVid.app/Contents/MacOS/fast-cutvid "/absolute/path/edit.fastcut.json"
```

One JSON project can be opened per launch. When combined with videos, the project opens first and videos import into its media bin in the background; they are not automatically appended to the timeline. Relative arguments resolve from the terminal's working directory, while media references inside JSON resolve from the project directory. Quote paths containing spaces; use `--` before filenames beginning with `-`. These GUI arguments cannot be combined with `--validate` or `--render`. On Windows, use `fast-cutvid.exe` instead.

## Agent rendering

The GUI's **Export cuts** action creates an edit decision document that references your source media.

Since 0.1.2, Export cuts suggests the original project's folder and basename with `-cutted.fastcut.json`. For an unsaved project it uses the first timeline clip's source video, or the first media asset if the timeline is empty. Exporting writes a copy without changing the current project path or clearing unsaved edits.

An agent can inspect or modify that JSON and invoke the exact same renderer without opening the interface:

```bash
cargo run --release -- \
  --render documentary.fastcut.json \
  --output documentary-final.mp4
```

The stable v1 contract lives in [`docs/timeline.schema.json`](docs/timeline.schema.json).

Agent-created timelines can contain any number of source assets and ordered clips. Media paths may be absolute or relative to the timeline JSON file. Validate a generated timeline without opening the interface or rendering:

```bash
cargo run --release -- --validate documentary.fastcut.json
```

A valid JSON timeline can be opened with **Open** or dropped directly onto the app. Dropped video files are imported into the current media bin; a dropped JSON timeline opens as the current project.

## Agent skill

The project includes the [`fast-cut-timelines`](.agents/skills/fast-cut-timelines/SKILL.md) skill for coding agents. It routes agents to the relevant operational or timeline-format instructions, explains how to inspect media, and requires validation before handoff. Agents should load this skill whenever they operate fastCutVid or create, modify, validate, or render a fastCutVid timeline.

## Editing controls

| Action | Control |
| --- | --- |
| Play / pause | `Space` or click the monitor |
| Jump one second | `Left` / `Right` |
| Previous / next frame | `Shift+Left` / `Shift+Right` |
| Previous / next edit | `Up` / `Down` |
| Split | `S`, `Cmd/Ctrl+K`, or **Split** |
| Delete clip | `Backspace` / `Delete` |
| Trim | Drag either edge of a timeline clip or use the inspector |
| Reorder | `Option/Alt+Left` / `Option/Alt+Right` |
| Timeline zoom / fit | `Cmd/Ctrl++`, `Cmd/Ctrl+-`, `Cmd/Ctrl+0` |
| Import / open | `Cmd/Ctrl+I`, `Cmd/Ctrl+O` |
| Save | `Cmd/Ctrl+S` |

Press `?` inside the app to see the complete shortcut reference.

## Architecture

```text
Rust / egui UI
      │
      ├── fastcut.timeline/v1 project model ── JSON for agents
      │
      ├── native playback backend
      │     └── macOS: AVPlayer + AVPlayerItemVideoOutput
      │
      └── FFmpeg normalized clip render ── concat ── MP4
```

On macOS, AVFoundation handles synchronized interactive playback; FFmpeg handles analysis and export. Windows and Linux currently use a silent FFmpeg preview backend while native playback support is planned.

## Roadmap

- Multi-track video and audio
- Drag-and-drop insertion and ripple/slip tools
- Persistent media cache for instant filmstrips and waveforms on project reopen
- Undo/redo history
- Hardware-accelerated export presets
- Windows Media Foundation and Linux GStreamer native playback backends
- Proxy media and relinking
- Signed and notarized installers

## Publishing a release

The release workflow builds downloadable packages for macOS Apple silicon, macOS Intel, Windows x86-64, and Linux x86-64. Each package includes its SHA-256 checksum. To publish:

1. Update the version in `Cargo.toml` and `Cargo.lock`, update `CHANGELOG.md`, add `docs/releases/vX.Y.Z.md`, and commit them.
2. Create a matching version tag, for example `git tag v0.1.2`.
3. Push the tag with `git push origin v0.1.2`.

The tag starts `.github/workflows/release.yml`, which verifies the version, tests with FFmpeg, builds every platform package, checks archive contents/checksums, and publishes a GitHub Release using the matching notes file. It uploads into a draft first and publishes after all eight assets are attached. No release is published if any platform build fails.

## Contributing

Issues and pull requests are welcome. Keep the v1 timeline format backward compatible and run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` before submitting.

## License

fastCutVid is open-source software available under the [MIT License](LICENSE). Copyright © 2026 fastCutVid contributors.
