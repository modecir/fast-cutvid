# fastCutVid agent instructions

For tasks that operate fastCutVid or create, modify, validate, or render timeline JSON, load `.agents/skills/fast-cut-timelines/SKILL.md` and follow `docs/AGENT_GUIDE.md`.

Do not invent source media, exceed source durations, or add unsupported finishing concepts to `fastcut.timeline/v1`. Validate every agent-created or modified project with `--validate` before handoff. Render only when the user asks for rendered media, and never use a source-media path as the output.

## Git Flow

- `main` contains final releases. `dev` integrates completed work for the next release.
- Start each feature, fix, or maintenance change on its own `codex/feature/<name>` branch from current `dev`. Do not make changes directly on `main` or `dev`.
- Preserve unrelated working changes and keep them out of the feature's commits. Commit the completed change, run formatting, Clippy, and tests, then merge into `dev` through a pull request with a merge commit.
- Start `codex/release/<version>` from `dev` only when preparing a requested release. Stabilize it, update version and release notes, and merge it into `main` through a pull request. Tag that merge commit as `vX.Y.Z`, then merge `main` back into `dev`.
- Use `codex/hotfix/<name>` from `main` for urgent released-version fixes. Finish through a release pull request and merge the result back into `dev`.
- Create or push version tags only when the user requests a release. Feature and `dev` changes must not publish releases.
- See `docs/GIT_FLOW.md` for the complete workflow.
