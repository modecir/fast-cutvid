# Use fastCutVid with local agents

fastCutVid 0.1.3 gives agents a file format and a command-line renderer. There is no built-in model, API key requirement, chat service, or MCP server. Your chosen agent reads source metadata, writes a `.fastcut` timeline, validates it, and optionally renders it. You can review the same timeline in the desktop app.

## Prepare your workspace

1. Download and extract the correct build from [v0.1.3](https://github.com/modecir/fast-cutvid/releases/tag/v0.1.3), and install FFmpeg and FFprobe. Follow the [operator guide](OPERATOR_GUIDE.md).
2. Create a working folder containing your videos, any transcript with source timestamps, and the intended output folder. Give the agent access to that folder.
3. Keep the bundled `docs`, `.agents`, `AGENTS.md`, and `CLAUDE.md` together. They are at the extracted package root on Windows/Linux; on macOS, they are inside `fastCutVid.app/Contents/Resources` (Finder → Show Package Contents). You can copy those four entries together into your working folder; do not copy just `SKILL.md`, because its references are relative.
4. Tell the agent the absolute path to the executable and the working folder. Confirm the executable runs with `--version` and that `ffprobe -version` works in the agent's execution environment.

You can instead clone the source repository and open it as the agent's project:

```bash
git clone https://github.com/modecir/fast-cutvid.git
cd fast-cutvid
git checkout v0.1.3
cargo build --release --locked
```

The executable is `target/release/fast-cutvid` (`fast-cutvid.exe` on Windows). Building from source requires Rust and the platform build dependencies; using the download does not.

### Executable paths

Use the path where you actually extracted or installed the release. For example, on macOS after moving the app to Applications:

```bash
/Applications/fastCutVid.app/Contents/MacOS/fast-cutvid --version
/Applications/fastCutVid.app/Contents/MacOS/fast-cutvid --validate ./edit.fastcut
/Applications/fastCutVid.app/Contents/MacOS/fast-cutvid --render ./edit.fastcut --output ./final.mp4
```

Linux, from the extracted package directory:

```bash
./fast-cutvid --validate ./edit.fastcut
./fast-cutvid --render ./edit.fastcut --output ./final.mp4
```

Windows PowerShell, from the extracted package directory:

```powershell
.\fast-cutvid.exe --validate .\edit.fastcut
.\fast-cutvid.exe --render .\edit.fastcut --output .\final.mp4
```

For a quoted executable path in PowerShell use the call operator: `& "C:\Tools\fastCutVid\fast-cutvid.exe" --version`. Relative media paths resolve from the JSON file, not from the executable. Rendering writes to the specified output and can overwrite it; choose a new filename and never a source video.

## OpenAI Codex

Open the source checkout or prepared working folder as a local Codex project. Repository skills live in `.agents/skills`; Codex can select them for matching tasks, and the CLI/IDE supports an explicit `$fast-cut-timelines` mention. If it does not appear, ask Codex to read the file directly. See [OpenAI's skill documentation](https://developers.openai.com/codex/skills/).

Start with this prompt, replacing the example paths with your own:

```text
Use $fast-cut-timelines. Read docs/AGENT_GUIDE.md and the timeline format reference.
My source is media/interview.mp4. Use the installed fast-cutvid executable at
<absolute executable path>. Probe the file, then make edit.fastcut containing
source seconds 4–12 followed by 30–45. Keep the original audio. Validate the file
with fast-cutvid --validate and report the total duration. Do not render yet.
```

To request the final media after reviewing the JSON in fastCutVid:

```text
Render edit.fastcut to outputs/interview-cut.mp4 with fast-cutvid --render.
Use a new output path. Verify the resulting video duration and audio stream with
ffprobe, then return the video path and the timeline path.
```

If you use a cloud task, local paths on your computer are not automatically present there. Supply the media and a compatible executable in that environment, or use a local task.

## Claude Code

Start Claude Code in the source checkout or prepared folder. The included `CLAUDE.md` imports `AGENTS.md` and points to the shared instructions. Claude Code supports this import mechanism; it does not automatically treat this repository's `.agents` directory as Claude slash commands. See [Claude Code's project-memory documentation](https://code.claude.com/docs/en/memory).

```text
Read .agents/skills/fast-cut-timelines/SKILL.md, its timeline reference, and
docs/AGENT_GUIDE.md. Use <absolute executable path> for fastCutVid commands.
Build edit.fastcut from media/interview.mp4, keeping source seconds 4–12
and 30–45 in that order. Probe the source first and validate the result.
Return the JSON for review; do not render yet.
```

This uses the same files and CLI as Codex. No separate Claude integration is required.

## Claude Cowork (Claude work)

In Claude Desktop, select/connect the working folder for Cowork and explicitly ask it to read the shared skill and guides. Folder instructions can point to those files. See [Anthropic's Cowork setup guide](https://support.claude.com/en/articles/13345190-get-started-with-claude-cowork).

Use the Claude Code prompt above. Ask Cowork to check whether its execution environment can actually run your fastCutVid binary and FFprobe. Connected file access does not guarantee host command execution: a cloud or isolated environment may need a compatible Linux binary or a handoff to your local terminal. Never assume that a macOS `.app` can run in a Linux environment.

If commands are unavailable, provide source metadata and timestamped cut decisions, have Cowork save a draft JSON, then run `--validate` locally before opening/rendering it. Cowork should report validation as pending instead of claiming success.

## Other local agents

Any agent with file access can prepare the JSON. An agent with a shell and compatible executables can also probe, validate, and render. Start with:

```text
Read AGENTS.md, docs/AGENT_GUIDE.md, docs/timeline.schema.json, and
.agents/skills/fast-cut-timelines/references/timeline-format.md.
Use only media in <working folder>. The fastCutVid executable is at <path>.
Create a timeline for <explicit cut instructions>. Validate it with --validate.
Report source paths, clip count, sequence duration, and validation output.
Render only if I request an output video.
```

There is no dependency on automatic skill discovery: explicitly reading the files works across agent tools. Start with a short cut to confirm your setup before processing long media.

## What agents must understand

- The `clips` array defines playback order. v1 is a single contiguous sequence without gaps, overlaps, effects, transitions, or independent tracks.
- Cut positions are source seconds. Use UUIDs for assets and clips and keep ranges inside the probed source duration. JSON paths must point to the actual media.
- Waveforms show amplitude; fastCutVid does not transcribe speech or decide which sentences to keep. Supply timestamped transcripts or explicit source ranges for speech editing. An agent needs a separate transcription tool if you want it to infer spoken content.
- `--validate` checks the document structure and relationships, not media existence or creative quality. Check paths and inspect/play the result too.
- The app works locally, but your agent provider's rules determine whether media or transcripts are sent to that provider. fastCutVid itself does not upload them.

See the [agent workflow guide](AGENT_GUIDE.md) for the exact JSON and render procedure. Provider-specific directions were checked against the linked official documentation on 2026-09-05.
