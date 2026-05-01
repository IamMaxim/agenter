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

The control plane is a Rust service that owns users, auth, session registry, workspace metadata, connector routing, public REST and realtime APIs, normalized event fan-out, pending approval state, delivery state, and a lightweight event cache.

The runner is a Rust daemon installed on every machine that can run agents against local workspaces. It connects outbound to the control plane over a secure bidirectional channel, supervises native harness processes, speaks provider protocols, and normalizes provider events before forwarding them.

Native agent systems remain the preferred source of truth for conversation history. The control plane stores enough metadata for routing, authorization, UI responsiveness, and connector delivery, but its event cache is not canonical transcript storage.

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
- map native lifecycle, message, tool, command, file, plan, permission, and error events into `AppEvent`;
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
- `oidc_providers`: configured OIDC providers, including Authentik;
- `runners`: runner registry and heartbeat state;
- `runner_tokens`: hashed runner connection tokens;
- `workspaces`: existing directories advertised or registered for a runner;
- `agent_sessions`: app-level session registry with provider and external session IDs;
- `connector_accounts`: verified Telegram and Mattermost identities linked to users;
- `session_bindings`: connector chats or threads bound to app sessions;
- `pending_approvals`: normalized approval requests and resolution state;
- `event_cache`: lightweight, prunable event cache for UI and delivery recovery;
- `connector_deliveries`: per-connector delivery receipts and retry/idempotency keys.

The schema must tolerate provider limitations. Codex is expected to expose richer thread and history APIs than Qwen ACP in some versions. Missing provider history/list/resume support should degrade the UI rather than corrupting session registry state.

## Internal Event Model

The runner normalizes provider events into app events. Initial event variants:

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

Provider-specific raw payload fragments may be preserved inside event metadata for debugging, but connector renderers must consume the normalized event shape.
`ProviderEvent` is the fallback for provider notifications that are useful to show or diagnose but do not yet deserve a first-class normalized event. Provider adapters should prefer first-class variants for stable user-facing behavior, and should use provider events to avoid silently dropping newly observed or lower-priority native protocol activity.

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

## Runner Protocol

Use outbound WebSocket from runner to control plane. The first protocol must support:

- authenticated `runner_hello`;
- heartbeat and server acknowledgment;
- workspace advertisement;
- provider capability advertisement;
- control-plane commands for create session, send input, resume, interrupt, answer approval, and shutdown session;
- runner events for normalized app events, command responses, errors, and health changes;
- request IDs for idempotency and correlation.

For local single-machine development, an embedded runner mode may share the same Rust traits without requiring a WebSocket hop.

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
- SvelteKit versus plain Svelte SPA.
- Connector retry and idempotency detail.
- Embedded runner mode scope.
