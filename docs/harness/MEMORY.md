# Project Memory

This file is the project-local memory surface for future agent sessions. Keep it compact and factual.

## Current State

- The repository is in documentation/harness initialization.
- The only source product spec is `docs/chatgpt/001_initial.md`.
- Application code has not been scaffolded.
- The project is not currently relying on a committed git history.

## Durable Assumptions

- Use a control-plane / runner split.
- Runners live near workspaces and harness processes.
- The control plane should not expose Codex or Qwen harness transports directly to the public internet.
- Browser is the full-fidelity interface.
- Telegram and Mattermost are constrained projections and should avoid full transcript backfill.
- Native agents are the preferred source of truth for conversation history.
- The control plane still stores registry metadata, connector bindings, pending approvals, delivery state, and lightweight event cache.
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

- 2026-04-30: Added protocol spike runbooks for Codex app-server (`docs/runbooks/codex-app-server-spike.md`) and Qwen ACP (`docs/runbooks/qwen-acp-spike.md`) so provider JSON-RPC shapes can be captured before adapter APIs are finalized.
- 2026-04-30: Created initial harness documentation and preserved the initial technical discussion in `docs/chatgpt/001_initial.md`.
