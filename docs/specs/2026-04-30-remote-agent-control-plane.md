# Remote Agent Control Plane Technical Spec

Status: proposed
Date: 2026-04-30

## Problem and Goal

Agenter is a self-hostable remote wrapper for coding agents. Authenticated users should be able to continue persistent agent sessions from a browser, Telegram, or Mattermost while the actual agent harnesses run near the target workspaces.

The first product target is a minimal useful remote console-chat:

- workspace list;
- session list;
- chat view;
- browser streaming;
- compact messenger projections;
- approval routing;
- persistent Codex and Qwen sessions;
- data model that does not block a later richer workbench.

## Non-Goals

The first implementation does not include:

- file browser;
- Git branch management;
- custom sandboxing;
- scheduled tasks;
- multi-agent orchestration;
- audit-grade full transcript storage;
- live streaming in Telegram or Mattermost;
- arbitrary MCP management UI.

## Architecture

Use a control-plane / runner split.

The control plane is a Rust service that owns users, auth, session registry, workspace metadata, connector routing, public REST and realtime APIs, normalized event fan-out, pending approval state, delivery state, durable browser projection events, and a lightweight compatibility event cache.

The runner is a Rust daemon installed on every machine that can run agents against local workspaces. It connects outbound to the control plane over a secure bidirectional channel, supervises native harness processes, speaks provider protocols, and normalizes provider events before forwarding them.

Native agent systems remain the preferred source of truth for conversation history where their history can be reloaded. The control plane stores enough metadata for routing, authorization, UI responsiveness, connector delivery, browser reconnect, and pending control-plane state. Its durable universal event log is the browser/reconnect projection log, not an audit-grade full transcript unless a future ADR expands that scope.

## Universal Agent Protocol

The browser/control-plane semantic contract is universal agent protocol version `uap/1`, accepted in `docs/decisions/2026-05-03-universal-agent-protocol.md`. The browser renders `uap/1` state and declared capabilities rather than provider-specific branches.

`uap/1` is an event-sourced, capability-gated superset over native harness protocols:

- Codex uses native `codex app-server` as the primary adapter because it exposes the richest thread, turn, item, plan, diff, approval, and lifecycle semantics.
- Gemini, Qwen, and OpenCode use the generic ACP runtime as the baseline adapter path.
- OpenCode HTTP/SSE and Qwen stream-json are later enhanced or fallback profiles.
- Native protocol identity is preserved through `NativeRef` for debugging, replay analysis, and future remapping. By default this means redacted summaries, stable native IDs, method/type names, hashes, or pointers, not full raw provider payloads.
- One active turn per native harness session is the default. Parallel work uses multiple Agenter sessions unless a future adapter advertises safe turn concurrency.
- Approvals are blocking protocol obligations when the native harness is waiting for an answer. The runner owns live native delivery; the control plane owns durable state, policy, idempotency, and frontend presentation.

The four protocol questions from `docs/chatgpt/002_protocol.md` are resolved as follows:

- Codex is native app-server primary, not forced through ACP.
- Harnesses execute their own tools in the first universal milestone; the control plane owns policy and approval decisions, and runners may serve ACP client filesystem or terminal methods from the configured workspace.
- One active turn per native session is accepted by default.
- Pending approvals wait through transient control-plane/runner reconnects; timeout cancellation is allowed only by explicit configured policy, and silent denial is not the default.

## Component Boundaries

### Control Plane

Responsibilities:

- password and OIDC authentication;
- user, runner, workspace, session, connector, approval, and delivery metadata;
- REST API for browser and future clients;
- WebSocket API for browser realtime updates;
- WebSocket runner channel;
- Telegram webhook endpoint and optional polling worker;
- Mattermost bot event worker;
- authorization checks for session access and approval decisions;
- normalized event fan-out to active subscriptions and connector bindings.

The control plane must not expose raw Codex or Qwen harness transports to public clients.

### Runner

Responsibilities:

- register and authenticate with the control plane;
- advertise available agent providers and configured workspaces;
- spawn or reuse persistent harness processes per workspace/provider as supported;
- speak Codex app-server through stdio JSON-RPC;
- speak Qwen Code through ACP using `qwen --acp`;
- map native lifecycle, message, tool, command, file, plan, permission, and error events into `uap/1` events and compatibility `AppEvent` projections during migration;
- forward approval decisions back to the harness;
- report process health and degraded session state.

### Browser UI

The browser is the full-fidelity interface. It should support login, workspace list, session list, chat view, rich event rendering, streaming text deltas, approval cards, session status, and links into connector settings.

### Telegram Connector

Telegram is a constrained projection. It supports both polling and webhook modes, login linking through browser auth, session selection, message routing, compact event rendering, and inline approval buttons where available. It should not dump old transcripts into chats.

### Mattermost Connector

Mattermost uses a bot account. Mattermost threads are projections of Agenter sessions. The connector uses Mattermost WebSocket events for incoming activity and REST for posting. Approvals use interactive actions where available and command fallback otherwise.

## Data Model

Use Postgres via SQLx. Initial tables:

- `users`: app user identity;
- `auth_identities`: password and OIDC identity bindings;
- `password_credentials`: Argon2id password hashes;
- `browser_auth_sessions`: hashed HttpOnly browser session cookies with expiry and revocation state;
- `oidc_providers`: configured OIDC providers, including Authentik;
- `runners`: runner registry and heartbeat state;
- `runner_tokens`: hashed runner connection tokens;
- `workspaces`: existing directories advertised or registered for a runner;
- `agent_sessions`: app-level session registry with provider and external session IDs;
- `connector_accounts`: verified Telegram and Mattermost identities linked to users;
- `session_bindings`: connector chats or threads bound to app sessions;
- `pending_approvals`: normalized approval requests and resolution state;
- `agent_events`: append-only durable `uap/1` event log for browser replay and control-plane projection state;
- `session_snapshots`: materialized universal session snapshots derived from `agent_events`;
- `recent_turn_caches`: persistent correlation and recovery aid for active or recently active native turns;
- `event_cache`: lightweight, prunable event cache for UI and delivery recovery;
- `connector_deliveries`: per-connector delivery receipts and retry/idempotency keys.

The schema must tolerate provider limitations. Codex is expected to expose richer thread and history APIs than Qwen ACP in some versions. Missing provider history/list/resume support should degrade the UI rather than corrupting session registry state.

`agent_events` supersedes `event_cache` for universal browser replay after migration. During the migration, old `AppEvent` and `event_cache` rows remain readable, and new code may emit both `agent_events` and `event_cache` until the frontend fully consumes `uap/1`.

`agent_events` should be append-only and contain at least:

- `seq`: monotonic global database replay cursor from the `agent_events` sequence;
- `event_id`: globally unique event ID;
- `workspace_id` and `session_id`;
- nullable `turn_id` and `item_id`;
- `event_type`;
- `event_json`;
- nullable `native_json`: redacted native summary, native IDs, method/type names, hashes, or pointer data by default;
- `source`;
- nullable `command_id`;
- `created_at`.

`seq` is global across `agent_events`, not per session. Rust code should represent it as a 64-bit integer type (`i64` for SQLx/Postgres `bigint`, or `u64` where a domain wrapper guarantees non-negative values). JSON APIs and WebSocket messages serialize `seq`, `after_seq`, and `latest_seq` as strings to avoid JavaScript precision loss. Browser `after_seq` uses that global cursor filtered by the subscribed session.

`native_json`, `NativeRef`, approval subjects, and event metadata must not store or expose full raw native provider payloads by default. Raw Codex JSON-RPC/wire payloads are runner-local opt-in diagnostics under `docs/decisions/2026-05-03-runner-local-codex-wire-logs.md`; they must not be exposed through the control-plane API, browser WebSocket, database, or app event cache by default. Persisting full raw native payloads requires a future explicit policy decision covering opt-in scope, redaction, retention, access control, and operator-facing risk.

`recent_turn_caches` are not transcript storage. They store correlation state such as native prompt request IDs, native turn IDs, active items by native ID, pending approvals by native request ID, latest plan and diff IDs, content buffers, outstanding native RPCs, and latest universal sequence.

Runner-side lossless reconnect requires a local runner WAL plus control-plane ack. The runner must write outbound universal events to the WAL before sending them, and the control plane must ack only after durable append to `agent_events`. Without both pieces, reconnect may be best-effort but must not be described as lossless.

This updates the stopped-on-disconnect rule from `docs/decisions/2026-05-02-runner-session-process-lifecycle.md`: before Stage 4, active sessions may be marked `stopped` on runner disconnect to avoid stale running state; after Stage 4 runner WAL/reconnect lands, transient control-plane WebSocket disconnect alone must not stop sessions when runner evidence proves native ownership survived. If runner or native process ownership is gone, mark the session `stopped`, turn `detached` or failed, and approvals orphaned according to evidence.

## Internal Event Model

The runner normalizes provider events into universal events and, during migration, into compatibility app events. Initial compatibility `AppEvent` variants:

- `SessionStarted`;
- `SessionStatusChanged`;
- `UserMessage`;
- `AgentMessageDelta`;
- `AgentMessageCompleted`;
- `PlanUpdated`;
- `ToolStarted`;
- `ToolUpdated`;
- `ToolCompleted`;
- `CommandStarted`;
- `CommandOutputDelta`;
- `CommandCompleted`;
- `FileChangeProposed`;
- `FileChangeApplied`;
- `FileChangeRejected`;
- `ApprovalRequested`;
- `ApprovalResolved`;
- `QuestionRequested`;
- `QuestionAnswered`;
- `ProviderEvent`;
- `Error`.

Provider-specific native references may be preserved inside event metadata for debugging, but they must use redacted summaries, stable native IDs, hashes, or pointers by default. Connector renderers must consume the normalized event shape.
`ProviderEvent` is the fallback for provider notifications that are useful to show or diagnose but do not yet deserve a first-class normalized event. Provider adapters should prefer first-class variants for stable user-facing behavior, and should use provider events to avoid silently dropping newly observed or lower-priority native protocol activity.

Universal entities:

- `Session`: app session bound to a provider, workspace, runner, native session/thread ID, status, mode/model state, and capabilities.
- `Turn`: one user-driven unit of agent work with status such as queued, running, awaiting approval, awaiting user input, cancelling, completed, failed, cancelled, interrupted, or detached.
- `Item`: timeline element within a turn, such as user message, assistant message, reasoning, plan, tool call, file change, command, MCP tool call, web search, image, context compaction, mode change, or system event.
- `ContentBlock`: typed item content, including text, markdown, reasoning summaries, terminal output, JSON, image references, diff references, and file references.
- `ApprovalRequest`: first-class approval state with canonical options, status, risk, subject, native request identity, and redacted native summary or pointer data.
- `UserInputRequest`: first-class request for clarification or form input, distinct from risky action approval.
- `PlanState`: current plan state with entries, source, artifacts, and lifecycle statuses for discovering, draft, awaiting approval, approved, implementing, completed, cancelled, failed, and revision requested.
- `Diff`: structured or native diff state associated with a turn, item, file change, approval, or artifact.
- `Artifact`: durable reference to generated or captured outputs such as plan files, images, logs, reports, or large command output.
- `NativeRef`: preserved native protocol, method or type, native ID, and redacted summary, hash, or pointer data. Full raw payload persistence requires an explicit future policy ADR.
- `CommandEnvelope`: idempotent browser/control-plane command wrapper with `command_id`, `idempotency_key`, optional session and turn IDs, and typed command payload.
- `CapabilitySet`: nested protocol, content, tool, approval, plan, mode, and integration capability descriptor used to gate frontend behavior.
- `SessionSnapshot`: materialized view of current session, turns, items, approvals, user-input requests, plan, diffs, artifacts, capabilities, connection state, and latest sequence.

Universal event envelopes include `event_id`, global `seq`, `session_id`, optional `turn_id`, optional `item_id`, timestamp, source, optional safe `native` reference, and typed event data. Important event families include `session.*`, `turn.*`, `item.*`, `content.*`, `tool.*`, `approval.*`, `user_input.*`, `plan.*`, `diff.*`, `artifact.*`, `usage.*`, `connection.*`, and `native.unknown`.

Events are facts and commands are wishes. For example, a `CancelTurn` command may produce `turn.status_changed` with `cancelling`; `turn.cancelled` is emitted only after native confirmation or runner-owned finalization.

## Trait Boundaries

Agent adapters implement provider-side behavior behind explicit typed interfaces:

```rust
pub trait AgentProvider: Send + Sync {
    fn provider_id(&self) -> &'static str;
    fn capabilities(&self) -> AgentCapabilities;
}

#[async_trait::async_trait]
pub trait AgentConnection: Send + Sync {
    async fn create_session(&self, req: CreateAgentSession) -> anyhow::Result<ExternalSession>;
    async fn resume_session(&self, external_session_id: &str) -> anyhow::Result<ExternalSession>;
    async fn read_session(&self, req: ReadSessionRequest) -> anyhow::Result<SessionSnapshot>;
    async fn send_user_input(&self, req: AgentInputRequest) -> anyhow::Result<TurnHandle>;
    async fn interrupt(&self, external_session_id: &str) -> anyhow::Result<()>;
    async fn answer_approval(&self, req: ApprovalAnswerRequest) -> anyhow::Result<()>;
}
```

Interaction connectors and event rendering are separate:

```rust
#[async_trait::async_trait]
pub trait InteractionConnector: Send + Sync {
    fn connector_id(&self) -> &'static str;
    async fn start(&self) -> anyhow::Result<()>;
    async fn send_event(
        &self,
        target: ConnectorTarget,
        event: RenderedConnectorEvent,
    ) -> anyhow::Result<DeliveryReceipt>;
}

pub trait EventRenderer: Send + Sync {
    fn render(
        &self,
        event: &AppEvent,
        capabilities: ConnectorCapabilities,
    ) -> Vec<RenderedConnectorEvent>;
}
```

The exact Rust module layout can evolve during implementation, but provider protocols, interaction connectors, rendering, and control-plane domain types must remain separate.

## Provider Behavior

### Codex

Use `codex app-server` via stdio JSON-RPC by default. The runner should initialize the app-server, create or resume threads, send turns, read session history where supported, subscribe to streamed item/turn notifications, and route command/file approval requests.

Do not expose Codex app-server networking publicly. Do not expose shell-command style helper APIs through messengers in v1 unless they are explicit user-initiated operations with clear authorization.

Browser slash commands are modeled as Agenter command metadata rather than Codex-only UI code. The control plane owns command listing, parsing contracts, confirmation requirements, and routing. Provider adapters own provider-native command manifests and execution. Codex exposes provider-heavy browser commands for thread compaction, review, steering, fork, archive/unarchive, rollback, and native shell command execution. Dangerous commands such as rollback and native shell execution require explicit browser confirmation before the control plane dispatches them to the runner.

### Qwen

Use `qwen --acp`. The runner should initialize ACP, inspect capabilities, create or resume sessions when supported, send prompts, collect `session/update` notifications, route permission requests, and normalize chunks, tool calls, plan updates, and errors.

History and resume behavior must be feature-detected.

Qwen/ACP slash command support uses the same generic command registry, but its provider-native manifest can remain empty until ACP-specific commands are validated. The browser and control-plane APIs must not assume Codex command IDs or Codex JSON-RPC method names for future ACP commands.

## Approval Handling

The backend authenticates the human and routes the decision. The harness owns execution policy. Agenter does not reinterpret command/file safety in v1.

Approval decisions:

- accept;
- accept for session when supported by provider;
- decline;
- cancel;
- provider-specific payload when needed.

Approval requests can expire, be resolved from another interface, or be cleared by cancellation. All connectors must render stale approvals as already resolved rather than allowing duplicate decisions.

Approval handling is durable `uap/1` state. Approval requests, updates, resolutions, expirations, and orphaning are appended to `agent_events` and materialized into `SessionSnapshot`. Clarifying questions and form input are represented as `UserInputRequest`, not dangerous action approvals. Plan approval is distinct from both tool approval and user input.

## Messenger History Policy

Browser can show full provider-backed history where supported.

Telegram and Mattermost should avoid replaying entire old histories. When a messenger binds an existing session, render a compact session card, recent visible turns when available, and a browser link for full history.

## Public API Sketch

Initial browser/client endpoints:

- `POST /api/auth/password/login`;
- `POST /api/auth/password/logout`;
- `GET /api/auth/me`;
- `GET /api/auth/oidc/{provider}/login`;
- `GET /api/auth/oidc/{provider}/callback`;
- `POST /api/link/start`;
- `GET /link/{code}`;
- `POST /api/link/complete`;
- `GET /api/runners`;
- `POST /api/runners`;
- `GET /api/runners/{id}/workspaces`;
- `POST /api/runners/{id}/workspaces`;
- `GET /api/sessions`;
- `POST /api/sessions`;
- `GET /api/sessions/{id}`;
- `POST /api/sessions/{id}/messages`;
- `POST /api/sessions/{id}/interrupt`;
- `POST /api/sessions/{id}/archive`;
- `GET /api/sessions/{id}/history`;
- `POST /api/workspaces/{id}/providers/{provider_id}/sessions/refresh`;
- `GET /api/approvals`;
- `POST /api/approvals/{id}/decision`;
- `GET /api/ws`.

The REST API should be documented with a Rust-native OpenAPI generator once the first API crate exists.

Browser realtime subscriptions use snapshot plus replay:

- browser subscribes with `session_id`, optional global `after_seq`, and `include_snapshot`;
- control plane responds with `SessionSnapshot`, missed universal events after `after_seq` filtered by session, and `latest_seq`;
- live universal events include global `seq`;
- reconnecting clients resume from their last seen sequence.

Legacy REST and WebSocket routes may remain during migration by wrapping requests in `CommandEnvelope` internally and projecting universal events back to old `AppEvent` shapes where needed.

## Runner Protocol

Use outbound WebSocket from runner to control plane. The first protocol must support:

- authenticated `runner_hello`;
- heartbeat and server acknowledgment;
- workspace advertisement;
- provider capability advertisement;
- control-plane commands for create session, send input, resume, interrupt, answer approval, and shutdown session;
- runner events for normalized app events, command responses, errors, and health changes;
- request IDs for idempotency and correlation;
- runner event sequence and ack frames for durable universal event delivery;
- runner WAL replay of unacked outbound universal events after reconnect.

For local single-machine development, an embedded runner mode may share the same Rust traits without requiring a WebSocket hop.

The runner must not claim lossless reconnect until outbound native events are WAL-written before send and acknowledged by the control plane after durable append to `agent_events`. Before that Stage 4 behavior exists, runner disconnect may still mark active sessions `stopped`. After Stage 4, transient control-plane WebSocket disconnect alone does not stop sessions when runner evidence proves native ownership survived. If the runner or native process dies with pending native approvals, the corresponding universal approval becomes orphaned and the active turn transitions according to evidence.

## Security Model

Minimum v1 security:

- browser access requires app auth;
- password auth uses Argon2id;
- OIDC is supported from the beginning, with Authentik as the first target;
- runner tokens are stored hashed server-side;
- runner connections use TLS in deployed mode;
- messenger identities are linked only through authenticated browser login;
- approval decisions require session authorization;
- connector tokens and OIDC secrets use environment or secret references, not plaintext committed config;
- workspace file access remains runner-local.

## Verification

Development phases should follow `docs/harness/VERIFICATION.md`.

The first phase must include protocol spikes for Codex app-server and Qwen ACP before committing to final adapter APIs. Later phases add Rust workspace checks, frontend checks, connector integration checks, and deployment smoke tests.

## Open Questions

- Exact Codex app-server JSON-RPC method and event shapes available in the installed version.
- Exact Qwen ACP capabilities for session listing, loading, and resume.
- Runner token rotation flow.
- Final database migration tool choice.
- Connector retry and idempotency detail.
- Embedded runner mode scope.
