---
name: fast-cut-timelines
description: Construct, modify, validate, or render fastCutVid video timeline JSON. Use when an agent prepares cuts from source media, edits a .fastcut project, or needs to understand fastCutVid's timeline semantics; not for effects or full finishing workflows.
---

# fastCutVid Timelines

fastCutVid is the assembly and cutting stage of a video workflow. Its v1 project is one contiguous ordered sequence of trimmed source clips. Do not add effects, transitions, titles, compositing, color-grading instructions, overlapping tracks, or advanced audio mixing; those belong in a downstream finishing tool.

## Choose the relevant instructions

- To operate the application, choose between GUI and headless commands, troubleshoot an execution, or hand a project to a human operator, read [`docs/AGENT_GUIDE.md`](../../../docs/AGENT_GUIDE.md).
- For downloaded executable paths and setup with Codex, Claude, or other local agents, read [`docs/LOCAL_AGENTS.md`](../../../docs/LOCAL_AGENTS.md).
- To create or modify timeline JSON, read [references/timeline-format.md](references/timeline-format.md) before writing the file.
- To help a human use the interface, consult [`docs/OPERATOR_GUIDE.md`](../../../docs/OPERATOR_GUIDE.md) and give only the steps relevant to their current task.

## Construct or modify a timeline

1. Work only from media paths the user or repository provides. Do not invent files.
2. Inspect each source with `ffprobe` when its exact duration, dimensions, frame rate, audio presence, or display rotation is not already known.
3. Follow [references/timeline-format.md](references/timeline-format.md). It defines ordering, trim, audio, path, and render semantics and includes a complete multi-asset example.
4. Preserve `format: "fastcut.timeline/v1"`, stable asset IDs, and stable clip IDs when updating an existing project. Generate unique UUIDs only for new objects.
5. Validate the result before reporting completion:

   ```bash
   fast-cutvid --validate path/to/edit.fastcut
   ```

6. Render only when the user requests media output:

   ```bash
   fast-cutvid --render path/to/edit.fastcut --output path/to/final.mp4
   ```

Use source timecodes in seconds. Keep every clip range inside its referenced asset duration. JSON paths may be absolute or relative to the JSON file; prefer relative paths when the project and media will move together.

Use the actual installed executable path if it is not on `PATH`; see the setup guide for macOS, Windows, and Linux examples. From a source checkout, `cargo run --release --` can replace `fast-cutvid` in these commands. If your agent environment cannot run the executable, report validation as pending and hand the document and validation command to a local operator.

The canonical machine-readable contract is `docs/timeline.schema.json` at the repository root. The application performs additional relational validation, such as UUID uniqueness, asset references, source bounds, finite numeric values, and valid rotations.
