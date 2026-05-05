# Project Memory

This file is the project-local memory surface for future agent sessions. Keep it compact and factual.

## Current State

- The repository is in active Rust/Svelte implementation, with `uap/2` as the runtime contract.
- The active implementation plan is `docs/plans/2026-05-05-universal-only-uap2.md`.
- The active source spec is `docs/specs/2026-05-05-universal-only-uap2.md`, with older source discussions preserved under `docs/chatgpt/`.
- The Rust workspace, browser UI, fake runner flow, ACP runner stack, and Qwen/Gemini/OpenCode integrations are present.
- Codex runtime adapter code is intentionally absent after the universal-only reset; a future Codex adapter should be designed as a provider that emits `AgentUniversalEvent` / `uap/2` events directly.

## Durable Assumptions

- Use a control-plane / runner split.
- Runners live near workspaces and harness processes.
- The control plane should not expose provider harness transports directly to the public internet.
- Browser is the full-fidelity interface.
- Telegram and Mattermost are constrained projections and should avoid full transcript backfill.
- Native agents are the preferred source of truth for conversation history.
- The control plane still stores registry metadata, connector bindings, pending approvals, delivery state, universal event log/snapshots, and lightweight event cache.
- Auth must support password and OIDC from the beginning.
- Messenger identity should be linked through authenticated browser login.

## Update Protocol

Update this file when:

- a major architecture decision is accepted;
- a protocol spike proves or disproves an assumption;
- a verification command becomes canonical;
- a recurring failure mode is discovered;
- the implementation plan changes phase.

Do not store large transcripts here. Link to specs, plans, decisions, and runbooks instead.

## Recent Notes

- 2026-05-05: Accepted `docs/decisions/2026-05-05-uap2-breaking-universal-protocol.md` and completed the phase on universal frame hardening: active universal frames now use `protocol_version: "uap/2"` and browser snapshots expose explicit replay cursor fields.
- 2026-05-05: Implemented `docs/plans/2026-05-05-universal-only-uap2.md` / `docs/specs/2026-05-05-universal-only-uap2.md`: runtime crates and browser source are universal-only, with no `NormalizedEvent`, `CachedEventEnvelope`, legacy normalized projection, or `AgentProviderId::CODEX` path; slash commands now emit universal user items and `slash_command` provider notifications, and provider commands are manifest-driven.
- 2026-05-05: Phase 7 universal-protocol hardening clarified `docs/runbooks/universal-protocol-smoke.md` and `docs/harness/VERIFICATION.md` so automated reconnect/receipt/interrupt/EOF checks are separated from manual live-provider chaos drills.
- 2026-05-04: Clarified `docs/plans/2026-05-03-universal-agent-protocol.md` replay behavior: truncated `session_snapshot` replay with `include_snapshot=true` is non-fatal, the browser uses the materialized snapshot as its live checkpoint, and replay-only subscriptions still close on incomplete replay to avoid advancing past a gap.
- 2026-05-03: Implemented `docs/plans/2026-05-03-nonblocking-session-refresh.md`: workspace/provider refresh now returns a background `refresh_id`, runner discovery runs off the connection path, DB-backed forced imports use a history fingerprint to skip unchanged projection rewrites, and runner refresh progress is exposed as an operation state machine with sidebar progress/log UI.
- 2026-05-03: Accepted `docs/decisions/2026-05-03-persistent-browser-auth-sessions.md`: Postgres-backed browser auth now persists SHA-256 cookie token hashes for 30 days in `browser_auth_sessions`; without `DATABASE_URL`, browser sessions remain in-memory development state.
- 2026-05-02: Accepted `docs/decisions/2026-05-02-runner-session-process-lifecycle.md` and implemented `docs/plans/2026-05-02-runner-session-lifecycle.md`: sessions now use durable `idle`/`stopped` statuses, `SessionStatusChanged` updates registry/database state, and runner disconnect marks active sessions stopped.
- 2026-05-02: Added the ACP runner support notebook under `docs/acp/` and accepted `docs/decisions/2026-05-02-generic-acp-runner-runtime.md`: Qwen, Gemini, and OpenCode use a shared runner ACP runtime with provider profiles; provider auth stays local setup.
- 2026-05-02: Implemented the browser workbench redesign plan in `docs/plans/2026-05-02-workbench-redesign.md`: sidebar from `tmp/mockup-1/Agenter Prototype.html`, chat/tool rows from `tmp/mockup-1/Tool Calls Mockup.html`, with no backend/protocol changes.
- 2026-05-01: Added model/mode/question support: provider-neutral turn settings, model/reasoning/mode option discovery, composer settings in the browser, and question cards for tool input plus MCP elicitation forms.
- 2026-04-30: Added protocol spike runbooks for Qwen ACP (`docs/runbooks/qwen-acp-spike.md`) so provider protocol shapes can be captured before adapter APIs are finalized.
- 2026-04-30: Created initial harness documentation and preserved the initial technical discussion in `docs/chatgpt/001_initial.md`.
