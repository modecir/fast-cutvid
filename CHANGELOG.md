# Changelog

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
