# Codex memory notes

Documentation in this folder describes **how upstream OpenAI Codex implements and consumes “agent memory”** on disk and in the session stack. It is **not** Agenter product behavior or code.

## Source

Research is based on a local checkout of the Codex repository at:

- [`tmp/codex`](../../tmp/codex) (workspace-relative)

Key upstream entry points:

- Runtime module: [`tmp/codex/codex-rs/core/src/memories/`](../../tmp/codex/codex-rs/core/src/memories/) (see `README.md` inside that directory for the pipeline narrative).
- Config types: [`tmp/codex/codex-rs/config/src/types.rs`](../../tmp/codex/codex-rs/config/src/types.rs) (`MemoriesToml` / `MemoriesConfig`).
- Prompt templates: [`tmp/codex/codex-rs/core/templates/memories/`](../../tmp/codex/codex-rs/core/templates/memories/).

If your checkout differs from ours, prefer the files in your tree when reconciling details.

## Contents

- [How Codex memory works](how-codex-memory-works.md) — filesystem layout, Phase 1 / Phase 2 pipeline, chat-time injection, telemetry, configuration, and ways to reduce clutter.

## Glossary

| Term | Meaning |
|------|---------|
| `CODEX_HOME` | Codex config and data root: default `~/.codex`, or `CODEX_HOME` if set (must exist as a directory). See [`tmp/codex/codex-rs/utils/home-dir/src/lib.rs`](../../tmp/codex/codex-rs/utils/home-dir/src/lib.rs). |
| Phase 1 | Startup job that claims rollouts from the state DB, runs an extraction model per rollout, and stores **stage-1** outputs (`raw_memory`, `rollout_summary`, optional `rollout_slug`) in the DB. |
| Phase 2 | Global consolidation job: selects stage-1 rows, syncs `raw_memories.md` and `rollout_summaries/`, then runs a **MemoryConsolidation** sub-agent to update consolidated files under the memories root. |
| Memory tool (feature) | `Feature::MemoryTool` gates the startup pipeline and (with `use_memories`) the memory **read-path** developer instructions. |
