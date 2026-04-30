# Engineering Workflow

This file defines how work should move from idea to implementation in this repository.

## Work Stages

1. Capture context.
2. Write or update a spec in `docs/specs/`.
3. Write an implementation plan in `docs/plans/`.
4. Execute the plan in small phases.
5. Verify each phase.
6. Record durable decisions in `docs/decisions/`.
7. Update project-local memory in `docs/harness/MEMORY.md`.

Do not skip directly from a broad idea to code. This project depends on several external protocols, so assumptions should be written down before they become hidden coupling.

## Specs

Specs should describe:

- problem and goal;
- explicit non-goals;
- architecture;
- data model;
- protocol boundaries;
- failure behavior;
- security model;
- testing and verification approach;
- open questions.

Use `docs/specs/YYYY-MM-DD-topic.md`.

## Plans

Plans should be executable checklists, not essays. Each phase should include:

- files or crates likely to change;
- behavior to add;
- verification command;
- exit criteria;
- known risks.

Use `docs/plans/YYYY-MM-DD-topic.md`.

## Decisions

Create an ADR when a choice is hard to infer from code alone. Examples:

- choosing stdio JSON-RPC over direct network harness exposure;
- choosing runner outbound WebSocket;
- changing database ownership boundaries;
- changing source-of-truth strategy for history;
- adding or dropping a connector capability.

Use `docs/decisions/YYYY-MM-DD-short-title.md`.

Each ADR should include:

- status;
- context;
- decision;
- consequences;
- alternatives considered.

## Runbooks

Runbooks are for repeatable procedures, especially protocol spikes and local operations. A runbook should include:

- prerequisites;
- exact commands;
- expected output shape;
- troubleshooting notes;
- cleanup steps.

Use `docs/runbooks/topic.md`.

## Context Management

Long-running work should maintain a compact current-state note in the active plan:

- what is done;
- what is currently being investigated;
- what failed and why;
- what command proved the latest state;
- what should happen next.

When context grows, update the plan instead of relying on chat history.

## Implementation Discipline

- Build protocol spikes before committing to adapter APIs.
- Keep control-plane types separate from provider-native payloads.
- Normalize events at the runner boundary, but preserve provider-specific payloads where useful for debugging.
- Treat connector rendering as a projection of app events, not as business logic.
- Keep auth and authorization checks in the control plane.
- Keep workspace file access in the runner.

