# Project Context

Last updated: 2026-05-06

## Product Goal

Build a self-hostable remote wrapper for coding agents so users can interact with agents from:

- browser UI;
- Telegram;
- Mattermost.

The target experience is similar to a desktop agent app: a list of chats, a chat view, persistent sessions, workspace selection, live browser streaming, event cards, and approval handling.

## Core Architecture

The project should use a control-plane / runner split.

The control plane owns:

- users and authentication;
- session registry;
- workspace metadata;
- connector routing;
- public REST and realtime APIs;
- universal event fan-out;
- pending approval state;
- explicit approval modes and durable approval policy rules;
- `uap/2` session snapshots and lightweight event cache.

The runner owns:

- local workspace access;
- persistent agent harness processes;
- Qwen/Gemini/OpenCode ACP integration;
- native protocol supervision;
- universal event reduction before forwarding to the control plane.

The runner should connect outbound to the control plane over a secure bidirectional channel so a home machine behind NAT can participate without exposing harness internals.

## Initial Stack

- Backend: Rust, Tokio, Axum.
- Database: Postgres via SQLx.
- Frontend: Svelte.
- Realtime: WebSocket for browser and runner channels.
- Auth: password plus OIDC, with Authentik as an explicit first OIDC target.
- Password hashing: Argon2id.
- API documentation: OpenAPI through `utoipa`, `aide`, or an equivalent Rust-native option.

## Initial Agent Providers

ACP:

- integrate through local ACP CLIs (`qwen --acp`, `gemini --acp`, and `opencode acp`);
- keep provider auth and runtime local to the workspace;
- map provider turns/items/messages into `uap/2` universal events for control-plane replay.

Qwen:

- integrate through `qwen --acp`;
- prefer ACP over CLI wrapping;
- treat ACP capabilities as provider-specific and feature-detect history/resume support.

## Initial Interaction Connectors

Browser:

- canonical full-fidelity UI;
- rich event rendering;
- streaming output;
- session list and chat view;
- approval modal/card.

Telegram:

- support polling and webhook modes;
- use login linking through browser auth;
- use compact event rendering;
- use inline buttons for approvals when available;
- avoid spamming old history into chats.

Mattermost:

- use a bot account;
- use Mattermost threads as session projections;
- use WebSocket events plus REST posting;
- use interactive approval UI where available, with command fallback.

## Source-of-Truth Rule

Native agents should remain the source of truth for conversation history when possible. The control plane still needs to store:

- app session IDs;
- external agent session/thread IDs;
- workspace and runner binding;
- connector bindings;
- pending approvals;
- delivery state;
- lightweight event cache.

The event cache is not canonical transcript storage and may be pruned.

## MVP Scope

Include:

- Rust control plane;
- Rust runner daemon;
- Postgres schema;
- browser session list and chat;
- browser streaming;
- ACP adapter;
- Qwen/Gemini/OpenCode ACP runtime;
- Telegram connector;
- Mattermost connector;
- login/password auth;
- OIDC auth;
- messenger login linking;
- approval routing;
- lightweight event cache.

Exclude for the first implementation:

- file browser;
- Git branch management;
- custom sandboxing;
- scheduled tasks;
- multi-agent orchestration;
- full audit-grade transcript storage;
- live streaming in Telegram or Mattermost;
- arbitrary MCP management UI.

## Open Design Areas

These need protocol spikes or explicit decisions before implementation hardens:

- exact ACP session semantics and approval payloads;
- exact Qwen ACP session history/resume support;
- future Codex adapter design on top of direct `uap/2` event emission;
- runner protocol authentication and token rotation;
- database migration tool choice;
- frontend app shape: SvelteKit versus Svelte SPA;
- connector retry and idempotency model;
- local embedded runner mode versus always separate process.

## Approval Posture

Agenter approval modes are product policy, not sandbox modes. The supported
session modes are `ask`, `read_only_ask`, `trusted_workspace`,
`allow_all_session`, and `allow_all_workspace`.

Dangerous allow-all modes are intentionally supported for VM-isolated runners.
The browser must label them as "Allow all operations" and
"danger-full-access", and `allow_all_workspace` persists a revocable
workspace/provider rule that applies to every approval kind. Agenter does not
provide an agent sandbox; provider-native sandbox or permission fields are
pass-through details only.
