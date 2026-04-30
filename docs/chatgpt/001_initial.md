I want to implement a nice remote wrapper for agents (codex, qwen code, etc.), so I can interact with my agent from web browser or messenger from phone. The target experience is similar to Codex macoOS desktop app: a list of chats and a chat view. Just using remote agent instead of built-in one. I want to implement the backend in Rust and browser frontend in Svelte. Both agent interfaces and interaction interfaces should be extensible (i.e. use traits, so I can add new ones without refactoring the entire project). In the first version, I want web, telegram and mattermost interaction interfaces and codex + qwen agent interfaces. Can we build a technical spec and plan for that? If you have questions, let's do Q&A before producing the plan.

---

Here are the questions I’d answer first. You can reply tersely, even as 1a, 2b, ...; unanswered items will become assumptions in the plan.

1. Agent execution model

1.1. Should Codex/Qwen run as CLI processes on your server?
For example: backend spawns codex / qwen inside a project directory and talks to stdin/stdout.

1.2. Do you want one persistent OS process per chat, or restart the agent CLI for every message?
Persistent is closer to desktop-agent UX and preserves tool/session state, but is harder to supervise.

1.3. Do you need full TTY/terminal semantics?
Some CLIs behave differently with a PTY: colors, progress bars, interactive prompts, keyboard shortcuts.
Options:

* simple stdin/stdout protocol
* PTY-backed session
* both, depending on agent adapter

2. Project/workspace model

2.1. Is each chat attached to a working directory / Git repo?
Example: chat A runs in /srv/workspaces/my-game, chat B runs in /srv/workspaces/homelab.

2.2. Should the app manage workspaces, or only connect to existing directories?
Examples of “manage”: clone repo, create branch, pull, commit, expose diffs.

2.3. Should chats be allowed to modify files directly?
Probably yes for Codex/Qwen, but this affects permissions, audit log, snapshots, rollback, and “dangerous command” controls.

3. Chat semantics

3.1. Should the system store full chat history itself, or treat the agent CLI as the source of truth?
I’d strongly lean toward storing normalized messages/events in your backend, but the adapters may also keep their own native state.

3.2. Do you want streaming output everywhere?
Browser can stream nicely. Telegram and Mattermost need batching/editing messages, which affects the event model.

3.3. Do you want non-text events in chat?
Examples:

* command started / finished
* file changed
* diff preview
* approval request
* tool call
* error
* attachment
* screenshot
* generated artifact

4. Human approval and safety

4.1. Should agents be able to run shell commands without approval?
Possible modes:

* full trust
* approve dangerous commands only
* approve every command
* per-workspace policy

4.2. Should the phone interfaces support approvals?
For example, Telegram button: “Approve command”, “Deny”, “Approve once”, “Approve for session”.

4.3. Do you want sandboxing in v1?
Options:

* no sandbox, just run as a restricted Unix user
* per-chat working dir isolation
* Docker/container per session
* firejail/bubblewrap/nsjail-style isolation
* later only

5. Users and auth

5.1. Is this single-user for you only, or multi-user from the beginning?

5.2. How should browser auth work?

* simple password/session cookie
* OAuth
* Tailscale-only/private network, minimal auth
* reverse proxy auth

5.3. How should Telegram/Mattermost identity map to app users?
For example, whitelist Telegram user IDs and Mattermost usernames.

6. Interaction interfaces

6.1. Telegram: bot polling or webhook?
Polling is easier for self-hosting; webhook is cleaner if you already expose HTTPS.

6.2. Mattermost: bot account with WebSocket/events API, slash command, or plugin?
The simplest v1 is probably a bot account that listens in selected channels/DMs.

6.3. Should browser, Telegram, and Mattermost all see the same chats?
Example: start chat in browser, continue from Telegram.

7. Backend architecture preferences

7.1. Preferred async/runtime stack?
I assume:

* Rust
* Tokio
* Axum
* SQLx
* Postgres or SQLite
* SvelteKit or plain Svelte SPA

7.2. Storage preference for v1?

* SQLite: simple self-hosted single-node app
* Postgres: better if you already expect multi-user / scaling / event streams

7.3. Do you want the backend to expose a public API for future clients?
For example, REST + WebSocket/SSE with stable DTOs.

8. Deployment

8.1. Where will this run?

* home PC
* VPS
* NAS/home server
* Kubernetes
* Docker Compose

8.2. Does the backend need to access your local repos on your main machine, or can repos live on the server?

8.3. Do you need mobile access outside your LAN?
If yes: public HTTPS, Tailscale, Cloudflare Tunnel, reverse proxy, etc.

9. Agent adapters

9.1. For Codex/Qwen, do you want to wrap their CLI UX as-is, or target a more structured protocol?
Wrapping CLI is fastest. Structured protocol is cleaner but only possible if the tools expose one.

9.2. Should each agent adapter expose capabilities?
Example:

* supports streaming
* supports session resume
* supports approvals
* supports file patch events
* supports images/attachments
* requires PTY
* supports structured JSON output

10. First-version scope

For v1, which target sounds better?

A. Minimal useful remote console-chat
Chats, workspaces, browser UI, Telegram/Mattermost text IO, streaming-ish output, persistent process sessions.

B. Agent workbench
Everything in A plus diffs, approvals, file browser, command log, Git branch controls.

C. Robust automation platform
Everything in B plus sandboxing, multi-user RBAC, queues, retry, audit log, plugin SDK, deployment hardening.

My default recommendation would be A with a data model that does not block B.

---

1.1. These harnesses expose networking protocols, let's use them instead of wrapping CLI.
1.2. One persistent process, with ability to spawn one if none are launched for given workspace (directory)
1.3. No, since we use protocols exposed by harness
2.1. Yes
2.2. Only connect to existing directories
2.3. Yes. I assume harnesses provide prompts for such features as respective CLIs do, correct me if I'm wrong.
3.1. Let's treat agents as the source of truth. 
3.2. No, streaming only in compatible connectors (for now, only browser).
3.3. Yes, gracefully represented in respective connectors. 
4.1. Let's offload these decisions to harness itself
4.2. Yes
4.3. No, the sandboxing is handled by the harness wherever it runs 
5.1. Multi-user, with extensible auth backends as well (let's start with simple login/password and OIDC)
5.2. Login/password + OIDC (at least Authentik, which I use)
5.3. Login flow for messengers using webpage to authenticate using supported auth providers. 
6.1. let's add support for both
6.2. Bot account, with threads as agent sessions
6.3. Yes. That also poses an interesting problem of loading chat context in messengers when selecting a session that has prior messages. We need to dig into that separately. 
7.1. What you wrote.
7.2. Postgres
7.3. Yes, let's do that, and we can use that API for web frontend
8.1. I plan both home PC and VPS deployments.
8.2. Repos live whereever harness is launched, these can be remote machines as well.
8.3. Yes, VPS will have port exposed. We need to support both inside LAN and outside LAN.
9.1. Use respective structured protocols.
9.2. Yes
10. A with a data model that does not block B.

---

Yes. Based on your answers, I’d design this as a multi-interface remote agent control plane, not as “Telegram bot glued to Codex/Qwen.” The core abstraction should be: users talk to sessions; sessions are backed by agent protocol adapters; interaction adapters only render and route events.

A few protocol corrections first:

Codex’s best structured integration point is codex app-server, not raw CLI wrapping. It exposes a rich JSON-RPC protocol with threads, turns, items, approvals, conversation history, and streamed events. Its default transport is stdio JSONL; WebSocket exists but is marked experimental/unsupported, and non-loopback listeners need explicit auth before being exposed.  ￼

Qwen Code’s structured integration point is ACP / Agent Client Protocol, enabled via qwen --acp. ACP is also JSON-RPC-based and currently defines stdio as the main stable transport; streamable HTTP is still described as draft/in progress.  ￼

So the architecture should include a small colocated agent-runner daemon on every machine where harnesses and repos live. The central backend should not assume every harness safely exposes a network API.

⸻

Technical spec: remote agent wrapper

1. Product goal

Build a self-hostable app that lets authenticated users interact with coding-agent sessions from:

* browser UI;
* Telegram bot;
* Mattermost bot.

The backend is Rust, frontend is Svelte, storage is Postgres. Agent backends are extensible. Initial agent integrations:

* Codex via codex app-server;
* Qwen Code via ACP / qwen --acp.

The UX target is close to a desktop agent app: workspace list, session list, chat view, live browser streaming, command/file/tool events, approval prompts, and the ability to continue the same session from browser or messenger.

2. Core architectural decision

Use a control-plane / runner split.

Browser / Telegram / Mattermost
          │
          ▼
┌────────────────────────────┐
│ Rust Control Plane Backend │
│ - auth                     │
│ - users                    │
│ - session registry         │
│ - connector routing        │
│ - event normalization      │
│ - public API               │
└─────────────┬──────────────┘
              │ secure runner protocol
              ▼
┌────────────────────────────┐
│ Agent Runner               │
│ lives near repos + harness │
│ - starts Codex/Qwen        │
│ - speaks native protocols  │
│ - supervises processes     │
│ - reports events           │
└─────────────┬──────────────┘
              │ stdio / local ws
              ▼
      codex app-server
      qwen --acp

This split solves the remote-workspace problem. Repos may live on your home PC, VPS, or another machine. The runner runs on that machine and owns the local harness process. The central backend only sees normalized session events and sends user inputs/approval decisions.

3. Important implication: agent is source of truth, but backend needs a registry

You said “treat agents as the source of truth.” Good. But the backend still needs to store session identity and routing metadata, otherwise browser, Telegram, and Mattermost cannot agree what “the same chat” is.

So the backend should not store canonical full message history in v1. It should store:

* user-owned session registry;
* external agent session/thread IDs;
* workspace and runner binding;
* connector bindings;
* lightweight event cache for UI responsiveness;
* pending approvals;
* delivery state per connector.

For Codex, this maps naturally to app-server threads/turns/items. Codex app-server has thread/start, thread/resume, thread/list, thread/read, turn history listing, and streamed item/turn notifications.  ￼

For Qwen/ACP, the session lifecycle is initialize, then session/new or session loading/resume if supported, then session/prompt, with progress arriving through session/update notifications.  ￼

⸻

4. Components

4.1. control-plane-api

Rust service, probably:

* tokio
* axum
* sqlx
* postgres
* tower-http
* tracing
* opentelemetry
* openidconnect
* argon2
* uuid
* serde
* utoipa or aide for OpenAPI
* async-trait if you prefer trait objects

Responsibilities:

* auth and sessions;
* user/workspace/session metadata;
* browser REST/WebSocket API;
* Telegram webhook endpoint and/or polling worker;
* Mattermost bot worker;
* runner registration and connection management;
* authorization checks;
* normalized event fan-out.

4.2. agent-runner

Rust service installed on each machine that can run agents.

Responsibilities:

* register with control plane;
* advertise available workspaces;
* advertise available agent providers;
* spawn/reuse persistent harness processes per workspace/session;
* speak native protocol:
    * Codex app-server JSON-RPC;
    * ACP JSON-RPC for Qwen;
* normalize native events into app-level events;
* forward approval requests and decisions;
* restart crashed processes when safe;
* never expose raw harness listeners directly to the public internet.

The runner can connect outbound to the control plane over WebSocket. That makes home-PC deployment easier behind NAT.

4.3. web-ui

Svelte frontend.

Core screens:

* login;
* workspace list;
* session list;
* chat view;
* approval panel;
* event inspector drawer;
* settings:
    * auth providers;
    * connectors;
    * runners;
    * workspaces;
    * agent defaults.

Browser should use WebSocket for:

* streaming agent output;
* event updates;
* pending approvals;
* session status.

4.4. connector-telegram

Supports both Telegram Bot API modes:

* polling via getUpdates;
* webhook via setWebhook.

Telegram officially supports long polling through getUpdates and HTTPS webhooks through setWebhook.  ￼

For v1:

* DM mode;
* group mode with mention/reply gating;
* inline buttons for approvals;
* /login, /sessions, /use, /new, /status, /stop, /open;
* message coalescing instead of true streaming.

4.5. connector-mattermost

Use a bot account plus Mattermost REST API and WebSocket events. Mattermost bot accounts use access tokens for REST API access, and Mattermost’s server docs describe REST JSON API plus WebSocket connections primarily for event delivery.  ￼

For v1:

* one Mattermost thread maps to one agent session;
* bot listens for posts, thread replies, and button actions;
* bot posts compact event cards;
* approvals use interactive message buttons if available, otherwise command replies.

⸻

5. Domain model

Postgres tables, v1-friendly:

users (
  id uuid primary key,
  display_name text not null,
  primary_email text,
  created_at timestamptz not null
);
auth_identities (
  id uuid primary key,
  user_id uuid not null references users(id),
  provider text not null, -- password, oidc
  subject text not null,
  email text,
  created_at timestamptz not null,
  unique(provider, subject)
);
password_credentials (
  user_id uuid primary key references users(id),
  password_hash text not null,
  updated_at timestamptz not null
);
oidc_providers (
  id uuid primary key,
  name text not null,
  issuer_url text not null,
  client_id text not null,
  client_secret_ref text not null,
  scopes text[] not null
);

Authentik is a good first OIDC target because it supports OAuth2/OIDC and is self-hostable.  ￼

runners (
  id uuid primary key,
  name text not null,
  owner_user_id uuid references users(id),
  connection_mode text not null, -- outbound_ws, static_token, mtls
  status text not null,
  last_seen_at timestamptz
);
workspaces (
  id uuid primary key,
  runner_id uuid not null references runners(id),
  owner_user_id uuid not null references users(id),
  display_name text not null,
  absolute_path text not null,
  default_agent_provider text,
  created_at timestamptz not null,
  unique(runner_id, absolute_path)
);
agent_sessions (
  id uuid primary key,
  owner_user_id uuid not null references users(id),
  workspace_id uuid not null references workspaces(id),
  runner_id uuid not null references runners(id),
  provider text not null, -- codex, qwen
  external_session_id text, -- codex thread id, ACP session id
  title text,
  status text not null, -- idle, active, awaiting_approval, crashed, archived
  created_at timestamptz not null,
  updated_at timestamptz not null,
  archived_at timestamptz
);
connector_accounts (
  id uuid primary key,
  user_id uuid not null references users(id),
  connector text not null, -- telegram, mattermost
  external_user_id text not null,
  external_display_name text,
  verified_at timestamptz not null,
  unique(connector, external_user_id)
);
session_bindings (
  id uuid primary key,
  session_id uuid not null references agent_sessions(id),
  connector text not null, -- web, telegram, mattermost
  external_chat_id text,
  external_thread_id text,
  external_message_id text,
  created_by_user_id uuid not null references users(id),
  created_at timestamptz not null,
  unique(connector, external_chat_id, external_thread_id)
);
pending_approvals (
  id uuid primary key,
  session_id uuid not null references agent_sessions(id),
  provider_request_id text not null,
  kind text not null, -- command, file_change, tool, network, generic_input
  payload jsonb not null,
  status text not null, -- pending, accepted, declined, cancelled, expired
  created_at timestamptz not null,
  resolved_at timestamptz
);

Optional but useful:

event_cache (
  id bigserial primary key,
  session_id uuid not null references agent_sessions(id),
  provider_event_id text,
  event_type text not null,
  payload jsonb not null,
  created_at timestamptz not null
);

This is not canonical history. It is a UI/delivery cache. You can prune it.

⸻

6. Internal event model

The app should normalize agent-specific events into a small common model.

pub enum AppEvent {
    SessionStarted(SessionInfo),
    SessionStatusChanged(SessionStatus),
    UserMessage(UserMessageEvent),
    AgentMessageDelta(TextDeltaEvent),
    AgentMessageCompleted(MessageCompletedEvent),
    PlanUpdated(PlanEvent),
    ToolStarted(ToolEvent),
    ToolUpdated(ToolEvent),
    ToolCompleted(ToolEvent),
    CommandStarted(CommandEvent),
    CommandOutputDelta(CommandOutputEvent),
    CommandCompleted(CommandCompletedEvent),
    FileChangeProposed(FileChangeEvent),
    FileChangeApplied(FileChangeEvent),
    FileChangeRejected(FileChangeEvent),
    ApprovalRequested(ApprovalRequestEvent),
    ApprovalResolved(ApprovalResolvedEvent),
    Error(AgentErrorEvent),
}

For Codex, map:

* thread/* → session lifecycle;
* turn/* → request lifecycle;
* item/agentMessage/delta → text delta;
* commandExecution item → command events;
* fileChange item → file events;
* item/commandExecution/requestApproval and item/fileChange/requestApproval → approval requests.

Codex app-server explicitly models threads, turns, items, command execution, file changes, MCP tool calls, and approval requests.  ￼

For Qwen/ACP, map:

* session/update with agent_message_chunk → text delta;
* tool_call / tool_call_update → tool events;
* session/request_permission → approval request;
* prompt response stop reason → turn completed.

ACP specifies session/update notifications for message chunks, tool calls, plans, available command updates, and mode changes; permission requests are part of the client-side baseline methods.  ￼

⸻

7. Rust trait boundaries

7.1. Agent provider trait

#[async_trait::async_trait]
pub trait AgentProvider: Send + Sync {
    fn provider_id(&self) -> &'static str;
    fn capabilities(&self) -> AgentCapabilities;
    async fn ensure_process(
        &self,
        workspace: &WorkspaceRef,
        config: AgentLaunchConfig,
    ) -> anyhow::Result<Box<dyn AgentConnection>>;
    async fn list_external_sessions(
        &self,
        workspace: &WorkspaceRef,
    ) -> anyhow::Result<Vec<ExternalSessionSummary>>;
}

7.2. Agent connection trait

#[async_trait::async_trait]
pub trait AgentConnection: Send + Sync {
    async fn create_session(
        &self,
        req: CreateAgentSession,
    ) -> anyhow::Result<ExternalSession>;
    async fn resume_session(
        &self,
        external_session_id: &str,
    ) -> anyhow::Result<ExternalSession>;
    async fn read_session(
        &self,
        external_session_id: &str,
        opts: ReadSessionOptions,
    ) -> anyhow::Result<SessionSnapshot>;
    async fn send_user_input(
        &self,
        external_session_id: &str,
        input: AgentInput,
    ) -> anyhow::Result<TurnHandle>;
    async fn interrupt(
        &self,
        external_session_id: &str,
    ) -> anyhow::Result<()>;
    async fn answer_approval(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> anyhow::Result<()>;
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AppEvent>;
}

7.3. Interaction connector trait

#[async_trait::async_trait]
pub trait InteractionConnector: Send + Sync {
    fn connector_id(&self) -> &'static str;
    async fn start(&self) -> anyhow::Result<()>;
    async fn send_event(
        &self,
        target: ConnectorTarget,
        event: RenderedConnectorEvent,
    ) -> anyhow::Result<DeliveryReceipt>;
    async fn update_message(
        &self,
        receipt: DeliveryReceipt,
        event: RenderedConnectorEvent,
    ) -> anyhow::Result<()>;
}

7.4. Renderer trait

Separate transport from formatting:

pub trait EventRenderer: Send + Sync {
    fn render(
        &self,
        event: &AppEvent,
        capabilities: ConnectorCapabilities,
    ) -> Vec<RenderedConnectorEvent>;
}

Browser renderer can preserve rich structure. Telegram/Mattermost renderers collapse complex events into readable cards.

⸻

8. Agent adapter details

8.1. Codex adapter

Use codex app-server via stdio by default.

Process model:

workspace + provider + runner
        │
        ▼
one persistent codex app-server process
        │
        ├── thread A
        ├── thread B
        └── thread C

The adapter should:

1. spawn codex app-server inside the runner;
2. send initialize;
3. call thread/list to discover known sessions;
4. call thread/start for new sessions, with cwd;
5. call thread/resume for existing sessions;
6. call turn/start for messages;
7. forward streamed notifications;
8. handle server-initiated approval requests;
9. expose thread/read / thread/turns/list to web UI for history.

Codex supports cwd, approval policy, sandbox policy, model, effort, summary, personality, output schema, thread resume, thread read, and turn history APIs.  ￼

Important: in v1, do not use Codex thread/shellCommand from messengers. The docs say it runs outside the sandbox with full access and should only be exposed for explicit user-initiated commands.  ￼

8.2. Qwen adapter

Use qwen --acp.

Process model:

workspace + provider + runner
        │
        ▼
one persistent qwen --acp process
        │
        ├── ACP session A
        ├── ACP session B
        └── ACP session C

The adapter should use the Rust ACP crate if it is mature enough. The ACP docs list an agent-client-protocol Rust crate with both agent and client-side traits.  ￼

The adapter flow:

1. spawn qwen --acp;
2. send ACP initialize;
3. inspect capabilities;
4. create session with session/new { cwd, mcpServers };
5. resume/load when supported;
6. send messages with session/prompt;
7. collect session/update notifications;
8. answer session/request_permission;
9. normalize chunks, tool calls, plan updates, and approvals.

Qwen’s own Channels feature is useful as a design reference: it already routes Telegram/WeChat/DingTalk messages into a single agent process via ACP, with isolated sessions per user and per-channel working directory/model/instructions.  ￼

But for your app, don’t use Qwen Channels directly as the product layer, because you want one generic control plane for Codex + Qwen + future harnesses.

⸻

9. Approval handling

Your assumption is mostly correct: both harness families have approval mechanisms.

Codex app-server sends server-initiated JSON-RPC approval requests for command execution and file changes, with decisions like accept, accept for session, decline, and cancel.  ￼

Qwen Code has permission modes: Plan, Default, Auto-Edit, and YOLO. Default mode requires manual approval for file edits and shell commands; Auto-Edit auto-approves file edits but still requires shell approval; YOLO auto-approves both.  ￼

Backend behavior:

agent approval request
        │
        ▼
normalize to pending_approvals
        │
        ├── browser: modal/card with full details
        ├── telegram: inline buttons
        └── mattermost: interactive buttons / command fallback

Approval decision enum:

pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
    ProviderSpecific(serde_json::Value),
}

Do not reinterpret approval policy in your own backend in v1. Your backend should authenticate the human and route the decision; the harness owns the execution policy.

⸻

10. Messenger session model

This is the trickiest UX area.

Browser

Browser is canonical UI. It can show:

* full session list;
* full history loaded from the agent where possible;
* live streaming;
* rich event cards;
* diffs;
* approvals;
* reconnect/resume.

Telegram

Telegram is a constrained projection.

Suggested commands:

/start
/login
/logout
/workspaces
/sessions
/use <session-number>
/new <workspace> [agent]
/status
/open
/stop
/help

Session binding:

Telegram DM with user
    └── active selected app session
Telegram group topic/thread
    └── one bound app session

For prior history, do not spam the messenger with every old message. Use:

1. a compact “session card”;
2. last 3–5 visible turns;
3. “Open full history” link to browser;
4. optional /history 10.

This keeps messenger history readable.

Mattermost

Mattermost has better threading, so map:

Mattermost root post
    └── agent session
Mattermost thread replies
    └── user turns / bot events

For an existing session selected from Mattermost:

* create a new root post: “Bound to session X”;
* include title, workspace, agent, status, last update;
* include a link to browser full history;
* optionally backfill a compact summary.

This avoids trying to reconstruct old agent history as fake Mattermost posts.

⸻

11. Public API sketch

Auth

POST /api/auth/password/login
POST /api/auth/password/logout
GET  /api/auth/me
GET  /api/auth/oidc/:provider/login
GET  /api/auth/oidc/:provider/callback

Messenger linking:

POST /api/link/start
GET  /link/:code
POST /api/link/complete

Flow:

1. user sends /login to Telegram/Mattermost bot;
2. bot creates short-lived link code;
3. user opens browser link;
4. user authenticates via password or OIDC;
5. backend binds messenger external identity to user.

Runners and workspaces

GET  /api/runners
POST /api/runners
GET  /api/runners/:id/workspaces
POST /api/runners/:id/workspaces
PATCH /api/workspaces/:id

Sessions

GET  /api/sessions?workspace_id=&provider=&archived=
POST /api/sessions
GET  /api/sessions/:id
POST /api/sessions/:id/messages
POST /api/sessions/:id/interrupt
POST /api/sessions/:id/archive
POST /api/sessions/:id/resume
GET  /api/sessions/:id/history?cursor=

Approvals

GET  /api/approvals?status=pending
POST /api/approvals/:id/decision

Browser realtime

GET /api/ws

Client subscribes:

{
  "type": "subscribe_session",
  "session_id": "..."
}

Server emits normalized AppEvent.

⸻

12. Runner protocol

Use one secure bidirectional channel from runner to control plane.

runner ──outbound WebSocket──► control plane

This works for home PC behind NAT and VPS. It also avoids exposing Codex/Qwen internals.

Runner handshake:

{
  "type": "runner_hello",
  "runner_id": "...",
  "version": "0.1.0",
  "token": "...",
  "capabilities": {
    "agents": ["codex", "qwen"],
    "transports": ["stdio"],
    "workspaces_discovery": false
  }
}

Control plane command:

{
  "type": "agent_send_input",
  "request_id": "...",
  "session_id": "...",
  "external_session_id": "...",
  "input": {
    "type": "text",
    "text": "Run tests and summarize failures"
  }
}

Runner event:

{
  "type": "agent_event",
  "session_id": "...",
  "event": {
    "type": "CommandStarted",
    "command": "cargo test",
    "cwd": "/home/max/project"
  }
}

For local single-machine deployment, the runner can be embedded into the backend process behind the same trait.

⸻

13. Security model

Minimum v1 security:

* all browser access requires app auth;
* OIDC support from the beginning;
* password auth uses Argon2id;
* runner tokens stored hashed server-side;
* runner connection is outbound over TLS;
* no raw Codex/Qwen listener exposed publicly;
* per-user workspace ownership;
* messenger accounts must be linked through authenticated browser login;
* pending approvals can only be answered by users authorized for that session;
* all connector tokens stored through env/secret references, not plaintext config.

Qwen’s own channel docs make the same practical point for bot tokens: store them through environment references rather than directly in settings.  ￼

⸻

14. MVP scope

Include in v1

* Rust backend with Postgres.
* Svelte browser UI.
* User auth:
    * password;
    * OIDC.
* Runner daemon.
* Workspace registry using existing directories only.
* Codex adapter.
* Qwen ACP adapter.
* Browser session list and chat.
* Browser streaming.
* Telegram connector:
    * polling;
    * webhook;
    * login/linking;
    * session select;
    * message send;
    * approval buttons.
* Mattermost connector:
    * bot account;
    * WebSocket event listener;
    * session-per-thread binding;
    * approval buttons/fallback commands.
* Agent-sourced history retrieval where supported.
* Lightweight event cache.

Exclude from v1

* file browser;
* Git branch management;
* custom sandboxing;
* multi-agent orchestration;
* scheduled tasks;
* full audit-grade transcript storage;
* live streaming in Telegram/Mattermost;
* mobile push beyond messenger notifications;
* arbitrary MCP management UI.

⸻

15. Implementation plan

Phase 0 — Protocol spike

Goal: remove uncertainty.

Build two tiny runner-side prototypes:

1. codex-app-server-spike
    * spawn codex app-server;
    * initialize;
    * create thread with cwd;
    * send turn/start;
    * print normalized events;
    * handle one approval request.
2. qwen-acp-spike
    * spawn qwen --acp;
    * initialize ACP;
    * create session;
    * send session/prompt;
    * print session/update;
    * handle permission request.

Exit criterion: both agents can answer a message, run a tool/command, and surface approval requests.

Phase 1 — Core backend + runner

* Create Rust workspace:

crates/
  control-plane/
  agent-runner/
  core/
  db/
  protocol/
  connectors/
    telegram/
    mattermost/
  agents/
    codex/
    qwen_acp/

* Implement DB migrations.
* Implement runner registration.
* Implement one embedded local runner mode.
* Implement normalized AppEvent.
* Implement session create/send/resume flow.

Exit criterion: browserless API can create a workspace, create a Codex/Qwen session, send a message, and receive events over WebSocket.

Phase 2 — Browser UI

* Login screen.
* Workspace list.
* Session list.
* Chat view.
* Streaming output.
* Event cards.
* Approval modal/card.
* Session status indicators.

Exit criterion: usable from browser like a minimal Codex desktop-style chat list.

Phase 3 — Telegram

* Bot config.
* Polling mode.
* Webhook mode.
* /login linking.
* /sessions, /use, /new.
* Message routing.
* Approval inline buttons.
* Compact rendering.

Exit criterion: start in browser, continue from Telegram, approve command from phone.

Phase 4 — Mattermost

* Bot token config.
* WebSocket event listener.
* REST posting.
* Thread/session binding.
* Login linking.
* Interactive approval UI or command fallback.
* Compact history card.

Exit criterion: Mattermost thread acts as an agent session projection.

Phase 5 — Hardening

* reconnect handling;
* runner heartbeat;
* process crash recovery;
* connector retry queues;
* idempotent message delivery;
* event cache pruning;
* structured logs;
* OpenTelemetry traces;
* Docker Compose deployment;
* reverse proxy docs.

⸻

16. Main design risks

Risk 1: “Agent as source of truth” differs by provider

Codex has explicit thread/history APIs. ACP has session lifecycle concepts, but individual agents may differ in how well they support listing/loading/resuming sessions. ACP has standardized session setup and optional load/resume support, but clients must check capabilities before using those features.  ￼

Mitigation: your internal session registry must tolerate missing history/list capabilities.

Risk 2: messenger backfill can become noisy

Telegram and Mattermost are not good full-fidelity transcript viewers. Treat them as control surfaces and notification surfaces. Browser remains full-fidelity.

Risk 3: approvals across devices need strict state handling

Approval requests can expire, be resolved elsewhere, or be cleared by cancellation. Codex emits serverRequest/resolved when a pending approval is answered or cleared; your backend should mirror this into pending_approvals.status.  ￼

Risk 4: raw network exposure of harness protocols

Codex app-server WebSocket is experimental/unsupported, and ACP’s stable transport is stdio. Use runner-level networking instead of exposing harness transports directly.  ￼

⸻

17. Recommended v1 architecture summary

The cleanest v1 is:

control-plane-api
  owns users, auth, session registry, connector routing
agent-runner
  owns actual Codex/Qwen processes near repos
agent adapters
  CodexAppServerAdapter
  QwenAcpAdapter
interaction adapters
  WebInteraction
  TelegramInteraction
  MattermostInteraction
renderers
  BrowserRenderer: rich + streaming
  TelegramRenderer: compact + buttons
  MattermostRenderer: thread cards + buttons

The most important abstraction is not “agent trait” alone. It is the combination of:

AgentProvider
AgentConnection
InteractionConnector
EventRenderer
SessionBinding

That gives you freedom to add later:

* Claude Code via ACP or SDK;
* Gemini CLI via ACP;
* OpenCode;
* Discord;
* Slack;
* Matrix;
* mobile app;
* richer workbench UI.

For the first implementation, I’d start with Codex app-server + browser UI only, then add Qwen ACP, then Telegram, then Mattermost. That sequence validates the hardest protocol and event-model decisions before you spend time on messenger UX.

