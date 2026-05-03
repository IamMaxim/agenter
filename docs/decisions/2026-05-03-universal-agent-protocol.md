# Universal Agent Protocol

Status: accepted
Date: 2026-05-03

## Context

Agenter needs one browser/control-plane protocol that can represent Codex, Gemini, Qwen, and OpenCode without making the frontend branch on provider-specific protocol details. Codex app-server exposes richer thread, turn, item, plan, diff, approval, and lifecycle semantics than the generic Agent Client Protocol shape. Gemini, Qwen, and OpenCode all have ACP-oriented protocol modes that are suitable as the shared baseline, while Qwen stream-json and OpenCode HTTP/SSE expose useful but different profiles.

The existing project decisions require a control-plane / runner split, provider protocols separated from interaction connectors, and native agents as the preferred source of truth for conversation history where their history can be reloaded.

## Decision

Agenter will expose universal agent protocol version `uap/1` as the browser/control-plane semantic contract. `uap/1` is an event-sourced, capability-gated superset over native harness protocols.

Codex uses native `codex app-server` as the primary adapter because it preserves Codex-native threads, turns, items, plans, diffs, approvals, file changes, command execution, and lifecycle events. Codex ACP, if added later, is a reduced-capability adapter profile rather than the primary Codex path.

Gemini, Qwen, and OpenCode use the generic ACP runtime as the baseline adapter path. OpenCode HTTP/SSE and Qwen stream-json remain later enhanced or fallback profiles, not the first universal protocol milestone.

Provider-specific details are preserved through `NativeRef` as redacted summaries, stable native IDs, method/type names, hashes, or pointers by default. Full raw native provider payloads, including Codex JSON-RPC wire frames, must not be exposed through the control-plane API, browser WebSocket, database, or app event cache by default. Persisting full raw native payloads requires a future explicit policy decision covering opt-in scope, redaction, retention, access control, and operator-facing risk. This aligns with the runner-local Codex wire-log boundary in `docs/decisions/2026-05-03-runner-local-codex-wire-logs.md`.

Unknown useful native messages must be represented as native fallback events using redacted summaries or pointers instead of being silently dropped or crashing the adapter.

The frontend must render from `uap/1` entities and `CapabilitySet`, not from provider-name branches. Capabilities gate plan UI, approval options, model and mode switching, diff support, replay support, tool detail, images, usage, and integration affordances.

One active turn per native harness session is the default. Parallel work uses multiple Agenter sessions unless a future adapter explicitly advertises safe native turn concurrency.

Approvals are protocol obligations, not just UI cards. A native approval may be a blocking native request that must receive exactly one native answer. The runner owns live native request delivery; the control plane owns policy, durable state, idempotency, and frontend presentation.

## Open Question Resolutions

1. Codex will use native app-server as its primary adapter, not forced ACP, because `uap/1` is intended to preserve the richest native semantics available.
2. Native harnesses execute their own tools for the first universal milestone. The control plane owns policy and approval decisions, and runners may serve ACP client filesystem or terminal methods from the configured workspace, but Agenter will not replace every harness tool with control-plane-hosted tools in this stage.
3. One active turn per native session is the accepted default. Multiple concurrent turns require multiple Agenter sessions unless an adapter later declares safe concurrency.
4. During control-plane/runner outages, pending approvals should wait through transient reconnects. Cancellation after a timeout is allowed only by explicit configured policy; silent denial is not the default.

## Consequences

The universal protocol can grow without making the browser provider-specific, and richer providers can expose richer capabilities without blocking simpler ACP profiles.

Adapters must include safe native reference preservation and capability declaration from the beginning. This increases type surface area, but makes protocol drift observable without moving raw wire payloads into the control-plane privacy boundary.

The current `AppEvent` model remains a compatibility projection while `uap/1` rolls out. Existing REST and WebSocket clients can be bridged until the frontend consumes universal snapshots and events directly.

Approvals, user-input requests, and plan-implementation requests must remain distinct state machines so the frontend does not present clarifying questions or plan handoffs as ordinary danger approvals.

## Alternatives Considered

- Force every provider through ACP: simpler adapter selection, but loses Codex fidelity and makes `uap/1` less useful as a superset.
- Expose provider protocols directly to the browser: avoids normalization, but violates provider/connector boundaries and leaks native protocol churn into public clients.
- Frontend branches by provider name: quick for early UI, but scales poorly and hides missing capability handling.
- Allow parallel turns by default: attractive for throughput, but native harness semantics are not uniformly safe for concurrent turns in one session.
