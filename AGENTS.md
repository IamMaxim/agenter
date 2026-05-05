# Agent Working Guide

This repository is in the active Rust/Svelte implementation stage. Do not add broad product surface area or new provider architecture until a concrete implementation plan has been written and approved.

## Startup Checklist

Every agent session should begin by reading these files, in order:

1. `docs/harness/PROJECT_CONTEXT.md`
2. `docs/harness/WORKFLOW.md`
3. `docs/harness/VERIFICATION.md`
4. Any active plan under `docs/plans/`
5. The relevant source spec under `docs/specs/` or `docs/chatgpt/`

If these files conflict, prefer the newest explicit user instruction, then the active plan, then `AGENTS.md`, then other harness docs.

## Current Project Shape

Agenter is intended to become a self-hostable remote control plane for coding agents. The target architecture is:

- Rust control-plane backend.
- Rust agent-runner daemon near workspaces and harness processes.
- Svelte browser UI.
- Postgres storage.
- Shared ACP runner runtime for Qwen, Gemini, and OpenCode.
- Future provider adapters, including any new Codex adapter, must emit `uap/2` universal events directly.
- Interaction connectors for browser, Telegram, and Mattermost.

The first implementation target is a minimal useful remote console-chat with a data model that does not block a later agent workbench.

## Working Rules

- Keep agent harness protocols separate from interaction connectors.
- Treat native agent systems as the source of truth for conversation history when possible.
- Store only registry metadata, connector bindings, pending approvals, and lightweight event cache in the control plane.
- Prefer explicit typed interfaces and small crates/modules over broad shared utility layers.
- Add decisions to `docs/decisions/` when architecture, protocol behavior, storage shape, or security posture changes.
- Keep plans in `docs/plans/` and make them executable in small phases.
- Keep runbooks in `docs/runbooks/` for repeatable local operations and protocol spikes.

## Before Editing

1. Check the current plan and decision log.
2. Identify whether the change affects control plane, runner, agent adapters, connectors, UI, or deployment.
3. Update documentation before or alongside implementation when behavior or architecture changes.
4. Do not silently change foundational assumptions from `docs/harness/PROJECT_CONTEXT.md`; record a decision instead.

## Completion Criteria

A task is not complete until:

- The relevant docs or plan reflect the implemented behavior.
- Verification from `docs/harness/VERIFICATION.md` has been run, or the reason it could not be run is recorded.
- Any new unresolved question is written down in the active plan or a decision file.
