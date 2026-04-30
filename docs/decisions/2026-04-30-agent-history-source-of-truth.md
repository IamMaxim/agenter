# Native Agents as History Source of Truth

Status: accepted
Date: 2026-04-30

## Context

Codex and Qwen maintain their own session state. Agenter needs cross-connector routing and UI responsiveness, but it should not become an audit-grade transcript database in the first version.

## Decision

Treat native agents as the preferred source of truth for conversation history. The control plane stores session registry metadata, external provider IDs, workspace and runner binding, connector bindings, pending approvals, connector delivery state, and a lightweight prunable event cache.

## Consequences

The browser can load provider-backed history where supported. Messenger connectors avoid replaying full old histories and use compact session cards with links to the browser. The session registry must tolerate providers that cannot list, resume, or read history consistently.

## Alternatives Considered

- Store every message and event as canonical history: simplifies UI replay but creates duplicate truth and larger privacy/storage obligations.
- Store no events at all: preserves provider authority but makes realtime UI, reconnect, and connector delivery unreliable.
