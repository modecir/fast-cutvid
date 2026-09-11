# Changelog

## Unreleased

- Find standard Homebrew and MacPorts FFmpeg/FFprobe installations when launched from Finder, restoring timeline thumbnails and waveforms without shell configuration.

## [0.1.4](https://github.com/modecir/fast-cutvid/releases/tag/v0.1.4) — 2026-09-08

- Keep the timeline time beneath the mouse cursor in place during trackpad pinch zoom.
- Preserve source display aspect ratio in the preview and timeline filmstrip, including portrait, square, and rotated footage.
- Show frames and waveform sections progressively with bounded background decoding and cancellation when switching projects.
- Cache media analysis across sessions, invalidate changed sources, and regenerate damaged cache entries.
- Limit timeline drawing to the visible portion and budget background texture uploads to keep editing responsive.
- Play macOS timeline cuts through one native composition and shared playback clock.
- Adopt Git Flow with protected development and release branches, platform CI, and final-release tag checks.
- Pin the Rust toolchain and support older FFmpeg versions in media regression tests.

## [0.1.3](https://github.com/modecir/fast-cutvid/releases/tag/v0.1.3) — 2026-09-07

- Use `.fastcut` for new project saves and cut exports, with legacy JSON projects still supported.
- Register macOS project documents and handle Finder opens at launch and while running.
- Include per-user file association scripts in Windows and Linux releases.

## [0.1.2](https://github.com/modecir/fast-cutvid/releases/tag/v0.1.2) — 2026-09-05

- Fix a crash when clicking or dragging the timeline with accessibility active; preserve timeline pinch zoom without assigning focus to a nonexistent widget.
- Default Export cuts to the original project or media folder with a `-cutted.fastcut.json` filename; retain the original project path and unsaved-edit state when exporting a copy.
- Add regression tests for accessibility-safe timeline interactions, pinch-zoom routing, and export paths.
- Link [fastcutvid.com](https://fastcutvid.com) from the README.

[Compare 0.1.1 → 0.1.2](https://github.com/modecir/fast-cutvid/compare/v0.1.1...v0.1.2)

## [0.1.1](https://github.com/modecir/fast-cutvid/releases/tag/v0.1.1) — 2026-09-05

### Added

- Open a timeline JSON or import multiple video files through positional launch arguments.
- Open one project and import additional videos in the same launch, regardless of argument order.
- Reject missing files, unsupported file types, multiple projects, and combinations of GUI files with headless commands.
- Document launch commands for operators and local agents; add regression tests for arguments and path handling.

### Changed

- Resolve relative launch paths from the working directory and retain absolute imported media paths when saving projects elsewhere.
- Require `--render` when supplying `--output`.
- Replace the third-party editor comparison in the README with product-neutral wording.
- Make release checks portable with an explicit Python version and support manually rebuilding an existing tag.

## [0.1.0](https://github.com/modecir/fast-cutvid/releases/tag/v0.1.0) — 2026-09-05

- First public release: native Rust video cutting with timeline trimming, splitting, reordering, filmstrips, and waveforms.
- Native macOS synchronized video/audio playback and menus; Windows/Linux silent video preview.
- Drag-and-drop media/project import, keyboard shortcuts, JSON cut export, validation, and MP4 rendering.
- MIT license, operator and local-agent guides, timeline skill, and downloadable builds for four platform targets.

[Compare 0.1.0 → 0.1.1](https://github.com/modecir/fast-cutvid/compare/v0.1.0...v0.1.1)
