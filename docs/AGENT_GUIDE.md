# fastCutVid agent guide

This guide is for AI and automation agents operating fastCutVid or preparing work for a human operator. Load the project skill at `.agents/skills/fast-cut-timelines/SKILL.md` before constructing, modifying, validating, or rendering a timeline.

For installing the binary, configuring Codex or Claude, and example prompts, start with [local agent setup](LOCAL_AGENTS.md).

## Choose the correct mode

- Use project JSON and the headless CLI when the requested result is a timeline, validation report, or rendered file. This is deterministic and does not require GUI automation.
- Use the GUI only when the user explicitly wants interactive application work or needs help with what is visible in the editor.
- Render media only when the user requests a rendered output. Creating or editing cut information alone does not authorize a potentially long render.

fastCutVid is a cutter, not a finishing suite. Do not encode transitions, titles, effects, compositing, color work, speed changes, overlapping tracks, or advanced mixes into v1 JSON.

## Inspect source media

Use only source paths supplied by the user or present in the scoped workspace. Never invent a path or silently substitute media.

For unknown metadata, inspect each source:

```bash
ffprobe -v error -show_streams -show_format -of json path/to/source.mp4
```

Record exact duration, encoded dimensions, frame rate, audio presence, and display rotation. fastCutVid stores rotation as clockwise `0`, `90`, `180`, or `270`; FFprobe display-matrix rotation has the opposite sign.

## Construct or update a project

1. Read `.agents/skills/fast-cut-timelines/references/timeline-format.md` and `docs/timeline.schema.json`.
2. Preserve `format: "fastcut.timeline/v1"`.
3. Preserve existing asset and clip UUIDs. Generate unique UUIDs for new records.
4. Put every source once in `assets`; clips may reuse an asset in any order.
5. Express trims in source seconds and keep `0 <= source_in < source_out <= asset.duration`.
6. Use `audio_gain: 1.0` unless the user requests a change. Use `muted: true` for intentional silence.
7. Prefer paths relative to the JSON file when the project and media will move together.

The array order of `clips` is the timeline order. There are no explicit timeline positions or gaps. Program duration is the sum of every clip's `source_out - source_in`.

## Validate before handoff

From a source checkout:

```bash
cargo run --release -- --validate path/to/edit.fastcut.json
```

With a downloaded executable:

```bash
fast-cutvid --validate path/to/edit.fastcut.json
```

Do not report the project as ready unless validation succeeds. Validation checks the format, unique IDs, references, source bounds, dimensions, frame rates, rotations, audio values, and render settings. It does not prove that media files exist or that the creative cut is correct, so verify source paths separately.

## Render when requested

```bash
fast-cutvid \
  --render path/to/edit.fastcut.json \
  --output path/to/final.mp4
```

Use a new output path rather than a source path. After the command succeeds, verify that the output exists and has nonzero duration with FFprobe before reporting completion.

If rendering fails, return the actionable FFmpeg or validation error. Do not repeatedly retry unchanged inputs. Correct a clearly local project error when authorized; ask the user when media is missing or the intended edit is ambiguous.

## Interoperate with the GUI

- Since 0.1.1, launch `fast-cutvid "edit.fastcut.json"` to open a project, or `fast-cutvid "clip one.mp4" "clip two.mov"` to import media. A project plus videos opens the project first, then imports the videos into its media bin. Only one project is accepted per launch. Use the installed executable path or `cargo run --release --` as needed. These positional arguments cannot be mixed with headless flags.
- Dropping videos imports them into Media but does not automatically append them to the sequence.
- Dropping timeline JSON opens it as the current project and rebuilds previews in the background.
- **Export cuts** writes the same JSON structure used by the headless commands.
- **Export video** invokes the same renderer as `--render`.

When handing work to an operator, report the timeline path, source count, clip count, total duration, validation result, and rendered output path if one was requested. Call out missing or externally located media explicitly.
