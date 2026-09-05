# fastCutVid agent instructions

For tasks that operate fastCutVid or create, modify, validate, or render timeline JSON, load `.agents/skills/fast-cut-timelines/SKILL.md` and follow `docs/AGENT_GUIDE.md`.

Do not invent source media, exceed source durations, or add unsupported finishing concepts to `fastcut.timeline/v1`. Validate every agent-created or modified project with `--validate` before handoff. Render only when the user asks for rendered media, and never use a source-media path as the output.
