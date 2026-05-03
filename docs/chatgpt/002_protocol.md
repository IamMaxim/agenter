I'm currently implementing a frontend that allows user to orchestrate various agent harnesses (codex, gemini cli, qwen code, open code). There's a Rust adapter that launches these CLIs in protocol mode and interacts with them. I quickly stumbled on a problem that all harnesses use slightly different protocols and semantics are often unclear, which leads to tons of small friction points: something looks off, something doesn't work properly, you got the idea. Let's analyze protocols of these harnesses and come up with a senior-level architecture to gracefully map all of them into a single universal protocol that my frontend speaks to. I expect this protocol to be the superset of all of harnesses protocols, with some features disabled for particular ones. We can map implementation-specific event types into our universal event types where that makes sense. Also keep in mind the state machines: approvals, plan implementations, etc. may need special handling with recent turn caches, runner/control plane state machines, etc. The system should be able to gracefully handle frontend reloads, frontend-control plane and control plane-runner connection interruptions, etc. Ask questions when you encounter any.

---

## Where I’d land

Build a **universal agent protocol as an event-sourced, capability-gated superset**, with **ACP as the default shape**, but **not limited to ACP**. Gemini, Qwen, and OpenCode already expose ACP-style protocol modes, while Codex’s richest interface is its app-server JSON-RPC protocol with explicit threads, turns, items, approvals, diffs, plans, and lifecycle events. ACP is a very good interoperability baseline, but your frontend protocol should be a semantic domain model above all native harnesses, not a thin translation layer. ([Agent Client Protocol][1])

The core idea:

```text
Frontend
  ⇅ Universal Agent Protocol, versioned, event-sourced
Control Plane
  ⇅ adapter API: semantic events + commands + capabilities
Runner / Harness Adapter
  ⇅ native protocol: Codex app-server, ACP, OpenCode server, stream-json, etc.
CLI Harness Process
```

The frontend should never care whether a “tool call” came from Codex `commandExecution`, ACP `tool_call`, OpenCode `bash`, Qwen `tool_use`, or Gemini plan-mode tooling. It should care about:

```text
session → turn → items → content blocks / tool calls / approvals / plan / artifacts / terminal state
```

Everything native should still be preserved as `raw_native_event` for debugging, replay, and future remapping.

---

## 1. Protocol landscape

| Harness          |                                Best primary protocol | What maps cleanly                                                                                                               | Important caveats                                                                                                                                                                                                                                                                |
| ---------------- | ---------------------------------------------------: | ------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **ACP baseline** |                              JSON-RPC 2.0 over stdio | initialize/auth, session new/load/prompt/cancel, session updates, tool calls, permissions, plans, modes                         | ACP is good for common semantics, but not every implementation supports the same methods or lifecycle richness. `session/update` is broad and agents emit different shapes. ([Agent Client Protocol][1])                                                                         |
| **Codex**        |                        **Codex app-server JSON-RPC** | threads, turns, items, plan updates, diff updates, approvals, reasoning, command execution, file changes, MCP calls, web search | This is richer than generic ACP. Codex also has `codex exec --json`, but that is better for batch/noninteractive use than full orchestration. ([GitHub][2])                                                                                                                      |
| **Gemini CLI**   |                                       `gemini --acp` | ACP sessions, prompts, cancel, modes, file-system proxy, MCP client exposure                                                    | Plan mode is a first-class mode with read-only planning, plan files, user approval, then implementation. Treat this as a plan state machine, not just text. ([Gemini CLI][3])                                                                                                    |
| **Qwen Code**    | `qwen --acp`; fallback `--output-format stream-json` | ACP sessions/prompts; approval modes; stream events in headless mode                                                            | Qwen’s headless `stream-json` and ACP surfaces are useful but semantically different. The project has had internal divergence between interactive, noninteractive, and ACP tool/permission paths, so your adapter should be conservative and normalize aggressively. ([Qwen][4]) |
| **OpenCode**     |   `opencode acp`; optional `opencode serve` HTTP/SSE | ACP for editor-like interaction; HTTP/SSE for sessions, events, diffs, aborts, permission replies                               | OpenCode’s server API is powerful and exposes session/message/permission endpoints. Its permission defaults are often permissive, so configure it deliberately if your universal UI wants approval gates. ([opencode.ai][5])                                                     |

So the architectural recommendation is:

```text
Use ACP adapter for Gemini, Qwen, OpenCode by default.
Use native Codex app-server adapter for Codex.
Optionally support Codex-ACP as a reduced-capability adapter profile.
Optionally support OpenCode HTTP/SSE as an enhanced adapter profile.
```

---

## 2. Universal protocol: semantic model

Your universal protocol should be **event-sourced** and **capability-declared**.

Every event should have:

```ts
type UniversalEvent = {
  event_id: string;          // globally unique
  seq: bigint;               // monotonically increasing per session or workspace
  session_id: string;
  turn_id?: string;
  item_id?: string;
  ts: string;                // ISO timestamp from control plane
  source: "codex" | "gemini" | "qwen" | "opencode" | "control_plane";
  native?: {
    protocol: string;
    method?: string;
    type?: string;
    id?: string;
    payload: unknown;
  };
  data: unknown;
};
```

Use **stable universal IDs**, but keep native IDs:

```ts
type IdMap = {
  universal_id: string;
  native_id?: string;
  native_parent_id?: string;
};
```

This matters because some protocols have explicit turn IDs, some do not. ACP sessions have prompt calls and updates, but you should synthesize a universal `turn_id` per `session/prompt` request. Codex already gives you thread/turn/item primitives. ([GitHub][2])

### Core entities

```text
Workspace
  Session
    Turn
      Item
        ContentBlock
        ToolCall
        ApprovalRequest
        Artifact
        Diff
        Plan
```

### Session

```ts
type Session = {
  session_id: string;
  harness: HarnessKind;
  adapter_profile: string;
  cwd: string;
  status:
    | "starting"
    | "ready"
    | "running_turn"
    | "recovering"
    | "error"
    | "closed";
  capabilities: CapabilitySet;
  current_mode?: ModeState;
  native_session_id?: string;
};
```

### Turn

```ts
type Turn = {
  turn_id: string;
  session_id: string;
  prompt_id?: string;
  status:
    | "queued"
    | "starting"
    | "running"
    | "awaiting_approval"
    | "awaiting_user_input"
    | "cancelling"
    | "completed"
    | "failed"
    | "cancelled"
    | "interrupted"
    | "detached";
  started_at?: string;
  completed_at?: string;
  stop_reason?: StopReason;
};
```

Use `detached` for the hard case: the control plane lost contact with the runner while the harness may still be doing something.

### Item

```ts
type Item = {
  item_id: string;
  turn_id: string;
  kind:
    | "user_message"
    | "assistant_message"
    | "reasoning"
    | "plan"
    | "tool_call"
    | "file_change"
    | "command"
    | "mcp_tool_call"
    | "web_search"
    | "image"
    | "context_compaction"
    | "mode_change"
    | "system";
  status:
    | "pending"
    | "running"
    | "streaming"
    | "awaiting_approval"
    | "completed"
    | "failed"
    | "cancelled";
  title?: string;
  blocks: ContentBlock[];
  native?: NativeRef;
};
```

Codex maps especially cleanly here because its app-server protocol already has item lifecycle events, including `item/started`, deltas, and `item/completed`, plus item types such as agent messages, reasoning, command execution, file change, MCP tool call, web search, image view, and context compaction. ([GitHub][2])

### Content blocks

```ts
type ContentBlock =
  | { type: "text"; text: string; annotations?: Annotation[] }
  | { type: "markdown"; markdown: string }
  | { type: "reasoning_summary"; text: string }
  | { type: "terminal_output"; stream: "stdout" | "stderr"; text: string }
  | { type: "json"; value: unknown }
  | { type: "image_ref"; uri: string; mime?: string }
  | { type: "diff"; diff_id: string }
  | { type: "file_ref"; path: string; range?: TextRange };
```

ACP’s tool-call model already supports content, locations, raw input/output, statuses, and tool kinds like read, edit, delete, move, search, execute, think, fetch, and other, so this block model can absorb ACP tool updates without losing meaning. ([Agent Client Protocol][6])

---

## 3. Capability model

Do not use booleans like “isCodex” in the frontend. Use capabilities.

```ts
type CapabilitySet = {
  protocol: {
    can_load_session: boolean;
    can_close_session: boolean;
    can_resume_active_turn: boolean;
    can_fork_session: boolean;
    can_cancel_turn: boolean;
    can_replay_events: boolean;
  };

  content: {
    assistant_delta: boolean;
    reasoning_delta: boolean;
    reasoning_summary: boolean;
    images: boolean;
    markdown: boolean;
    usage: boolean;
    cost: boolean;
  };

  tools: {
    reports_tool_calls: boolean;
    reports_raw_input: boolean;
    reports_raw_output: boolean;
    reports_file_locations: boolean;
    reports_diff_snapshots: boolean;
    reports_shell_exit_code: boolean;
  };

  approvals: {
    supports_approval_requests: boolean;
    supports_approve_once: boolean;
    supports_always_allow: boolean;
    supports_deny: boolean;
    supports_policy_patch: boolean;
    native_approval_request_blocks_runner: boolean;
  };

  plan: {
    supports_plan: boolean;
    plan_is_structured: boolean;
    plan_updates_are_replace_all: boolean;
    supports_plan_approval: boolean;
    supports_plan_mode: boolean;
  };

  modes: {
    supports_modes: boolean;
    supports_model_switch: boolean;
    available_modes: ModeDescriptor[];
  };

  integration: {
    supports_mcp_client: boolean;
    supports_client_filesystem_proxy: boolean;
    supports_client_terminal_proxy: boolean;
    supports_http_server: boolean;
  };
};
```

ACP initialization already negotiates protocol version, client capabilities, agent capabilities, and auth methods. You can extend this idea in your own protocol and allow native adapters to expose a richer normalized capability descriptor. ([Agent Client Protocol][7])

For unknown or future native behavior, keep your extension surface open. ACP itself reserves extension methods beginning with `_`, which is a good pattern to mirror in your universal protocol for experimental frontend features. ([pkg.go.dev][8])

---

## 4. Universal commands

The frontend should send commands to the control plane, not directly to the harness.

```ts
type UniversalCommand =
  | StartSession
  | LoadSession
  | CloseSession
  | StartTurn
  | CancelTurn
  | SendUserInput
  | ResolveApproval
  | SetMode
  | SetModel
  | ForkSession
  | RequestDiff
  | RevertChange
  | Subscribe
  | GetSnapshot;
```

Every command should include an idempotency key:

```ts
type CommandEnvelope<T> = {
  command_id: string;
  idempotency_key: string;
  session_id?: string;
  turn_id?: string;
  command: T;
};
```

This is non-negotiable if you want graceful reload/reconnect behavior. If the frontend resends `ResolveApproval` after a WebSocket reconnect, the control plane must return the already-known result rather than double-sending the approval to the harness.

---

## 5. Universal event taxonomy

Use a relatively small set of high-level event names.

```text
session.created
session.loaded
session.ready
session.mode_changed
session.capabilities_updated
session.closed
session.error

turn.started
turn.status_changed
turn.completed
turn.failed
turn.cancelled
turn.interrupted
turn.detached

item.created
item.updated
item.completed
item.failed

content.delta
content.completed

tool.started
tool.updated
tool.output_delta
tool.completed
tool.failed

approval.requested
approval.updated
approval.resolved
approval.expired
approval.orphaned

plan.updated
plan.approval_requested
plan.approved
plan.rejected
plan.implementation_started
plan.completed

diff.updated
artifact.created
usage.updated

connection.runner_attached
connection.runner_detached
connection.frontend_subscribed
```

A key rule: **events are facts, commands are wishes**.

For example, after the frontend sends `CancelTurn`, emit:

```text
turn.status_changed: cancelling
```

Only emit:

```text
turn.cancelled
```

when the native harness confirms cancellation or the runner has killed the process and finalized the turn. ACP explicitly says cancellation should stop LLM work, abort tools, send pending updates, and respond with a cancelled stop reason, so your universal state machine should allow a `cancelling` limbo state. ([Agent Client Protocol][7])

---

## 6. Approval model

Approvals are where most adapter bugs will happen. Treat them as first-class state machines.

```ts
type ApprovalRequest = {
  approval_id: string;
  session_id: string;
  turn_id: string;
  item_id?: string;
  kind:
    | "shell_command"
    | "file_edit"
    | "file_delete"
    | "file_move"
    | "external_directory"
    | "network"
    | "mcp_tool"
    | "plan_implementation"
    | "question"
    | "other";

  title: string;
  description?: string;

  risk: {
    level: "low" | "medium" | "high" | "unknown";
    reasons: string[];
  };

  subject?: {
    command?: string;
    cwd?: string;
    files?: string[];
    diff_id?: string;
    tool_name?: string;
    raw_input?: unknown;
  };

  options: ApprovalOption[];

  status:
    | "pending"
    | "presented"
    | "resolving"
    | "approved"
    | "denied"
    | "cancelled"
    | "expired"
    | "orphaned";

  native?: NativeRef;
};
```

Use canonical approval options:

```ts
type ApprovalOption =
  | { id: "approve_once"; label: string }
  | { id: "approve_always"; label: string; scope: ApprovalScope }
  | { id: "deny"; label: string }
  | { id: "deny_with_feedback"; label: string }
  | { id: "cancel_turn"; label: string };
```

Map native options into these canonical options, but keep the native option ID as metadata.

### Approval states

```text
created
  → pending
  → presented
  → resolving
  → approved | denied | cancelled | expired | orphaned
```

Important implementation detail: **native approval requests often block the harness protocol request**. ACP `session/request_permission` is a JSON-RPC request from agent to client; if the prompt turn is cancelled, the client must respond with a cancelled outcome. ([Agent Client Protocol][7])

That means the runner must own the live native connection. The control plane should not merely “record approval pending”; it must eventually send a native response, even if that response is “cancelled because the control plane lost authority.”

### Distinguish approvals from questions

Gemini plan mode can ask clarifying questions. OpenCode has a `question` permission/tool category. These should not always become “danger approvals.” Split:

```text
approval.requested       // allow/deny a risky action
user_input.requested     // agent needs clarification or form input
plan.approval_requested  // approve transition from plan to implementation
```

This keeps the frontend UX sane.

---

## 7. Plan model

Plans need their own reducer. Do not treat them as normal assistant markdown.

```ts
type PlanState = {
  plan_id: string;
  turn_id: string;
  status:
    | "none"
    | "discovering"
    | "draft"
    | "awaiting_approval"
    | "revision_requested"
    | "approved"
    | "implementing"
    | "completed"
    | "cancelled";

  entries: PlanEntry[];
  artifact_refs: string[];
  source: "native_structured" | "markdown_file" | "todo_tool" | "synthetic";
};
```

```ts
type PlanEntry = {
  id: string;
  title: string;
  status:
    | "pending"
    | "in_progress"
    | "completed"
    | "failed"
    | "cancelled";
  detail?: string;
};
```

For ACP plan updates, use **replace-all semantics** when the native protocol says the plan update is complete. Your reducer should replace the current plan entries instead of appending, unless the adapter marks the event as partial. ACP’s plan feature is designed around full plan updates. ([Agent Client Protocol][9])

### Suggested canonical plan lifecycle

```text
none
  → discovering
  → draft
  → awaiting_approval
  → approved
  → implementing
  → completed
```

Allow side paths:

```text
draft → revision_requested → draft
awaiting_approval → cancelled
implementing → failed
implementing → cancelled
```

### Harness-specific mapping

**Codex**
Map native `turn/plan/updated` to `plan.updated`. Codex also has explicit item types for plan, command execution, file changes, etc., so preserve the plan as both an item and the session’s active `PlanState`. ([GitHub][2])

**Gemini**
Map plan mode to:

```text
session.mode_changed: plan
plan.updated: source = markdown_file or native_structured
plan.approval_requested
plan.implementation_started
session.mode_changed: implementation/default
```

Gemini’s plan mode is read-only, writes a plan file, lets the user approve/iterate/cancel, and then proceeds to implementation after approval. ([GitHub][10])

**Qwen**
Map Qwen plan/default/auto-edit/yolo approval modes into universal modes and approval policy. Qwen’s documented modes include plan, default, auto-edit, and yolo, with different approval behavior for edits and shell commands. ([GitHub][11])

**OpenCode**
Map `todowrite` into `plan.updated` with `source = "todo_tool"` if no richer plan is available. Map OpenCode permission `question` to `user_input.requested`, not necessarily `approval.requested`. OpenCode exposes built-in tools including bash, edit, write, read, grep, glob, apply_patch, todowrite, webfetch, websearch, and question. ([opencode.ai][12])

---

## 8. Adapter design

Each adapter should implement the same internal Rust trait.

```rust
#[async_trait]
pub trait HarnessAdapter {
    async fn start(&mut self, req: StartSessionRequest) -> Result<SessionStarted>;
    async fn load(&mut self, req: LoadSessionRequest) -> Result<SessionLoaded>;
    async fn start_turn(&mut self, req: StartTurnRequest) -> Result<TurnStarted>;
    async fn send_user_input(&mut self, req: UserInputRequest) -> Result<()>;
    async fn resolve_approval(&mut self, req: ResolveApprovalRequest) -> Result<()>;
    async fn cancel_turn(&mut self, req: CancelTurnRequest) -> Result<()>;
    async fn set_mode(&mut self, req: SetModeRequest) -> Result<()>;
    async fn close(&mut self, req: CloseSessionRequest) -> Result<()>;

    fn capabilities(&self) -> CapabilitySet;
    fn event_stream(&mut self) -> BoxStream<'_, Result<UniversalEvent>>;
}
```

Internally, split the adapter into three pieces:

```text
NativeTransport
  stdio JSON-RPC / JSONL / WebSocket / HTTP / SSE

NativeCodec
  decode native messages
  encode native requests
  handle request/response correlation

SemanticReducer
  native event/request → universal event
  universal command → native request
  recent-turn correlation
```

This separation helps a lot because OpenCode may be ACP over stdio or HTTP/SSE, Qwen may be ACP or stream-json, and Codex app-server is its own JSON-RPC dialect. Codex app-server is JSON-RPC-like over JSONL, but its docs note the `"jsonrpc": "2.0"` field is omitted; it also has overloaded/backpressure behavior that clients should retry. ([GitHub][2])

---

## 9. Mapping rules by harness

### 9.1 ACP adapter: Gemini, Qwen, OpenCode

ACP gives you this basic native flow:

```text
initialize
session/new or session/load
session/prompt
session/update notifications
session/request_permission requests
session/cancel
```

ACP explicitly describes initialization, auth, session creation/loading, prompting, updates, permission requests, cancellation, and prompt responses with stop reasons. ([Agent Client Protocol][1])

Map it like this:

| ACP native                     | Universal                                           |
| ------------------------------ | --------------------------------------------------- |
| `initialize` response          | `session.capabilities_updated`                      |
| `session/new` response         | `session.created`                                   |
| `session/load` response        | `session.loaded`                                    |
| `session/prompt` request sent  | synthesize `turn.started`                           |
| `session/update` message chunk | `content.delta`                                     |
| `session/update` tool call     | `tool.started` / `tool.updated`                     |
| `session/update` plan          | `plan.updated`                                      |
| `session/request_permission`   | `approval.requested`                                |
| permission response            | `approval.resolved`                                 |
| `session/cancel`               | `turn.status_changed: cancelling`                   |
| prompt response stop reason    | `turn.completed` / `turn.cancelled` / `turn.failed` |
| `current_mode_update`          | `session.mode_changed`                              |

ACP also has mode support through available modes, current mode updates, and `session/set_mode`, so your universal `SetMode` command can map directly where supported. ([Agent Client Protocol][13])

Critical ACP adapter behavior:

```text
One session/prompt call = one universal turn.
All session/update notifications during that native prompt are assigned to that turn.
If a permission request arrives during the prompt, attach it to the active turn.
If updates arrive after cancel, still accept and reduce them.
```

ACP specifically notes clients should continue accepting tool updates after cancel, because agents may still send pending updates while winding down. ([Agent Client Protocol][7])

### 9.2 Codex adapter

Prefer Codex app-server over `codex exec --json` for interactive orchestration.

Map:

| Codex app-server             | Universal                        |
| ---------------------------- | -------------------------------- |
| Thread                       | Session                          |
| Turn                         | Turn                             |
| Item                         | Item                             |
| `agentMessage`               | assistant message item           |
| `reasoning`                  | reasoning item                   |
| `plan` / `turn/plan/updated` | plan item + `plan.updated`       |
| `commandExecution`           | command/tool item                |
| `fileChange`                 | file change item + diff/artifact |
| `mcpToolCall`                | MCP tool item                    |
| `webSearch`                  | web search item                  |
| `turn/diff/updated`          | `diff.updated`                   |
| request approval             | `approval.requested`             |
| `turn/completed`             | `turn.completed`                 |
| `turn/interrupted`           | `turn.interrupted`               |

Codex app-server’s documented lifecycle is initialize, thread start/resume/fork, turn start, stream item events, then finish the turn as completed/interrupted/failed. ([GitHub][2])

Codex approvals should map very cleanly because native shell and file approvals include explicit approval requests, decisions, and resolved events. ([GitHub][2])

Use `codex exec --json` only for:

```text
batch jobs
CI-like execution
fire-and-forget turns
testing native event normalization
```

Its JSONL event stream includes thread, turn, item, and error events, but it is not the same as running a full bidirectional orchestration server. ([developers.openai.com][14])

### 9.3 Gemini adapter

Use `gemini --acp`.

Gemini’s ACP mode is intended for programmatic control by IDEs and dev tools, with JSON-RPC over stdio. It supports ACP methods including initialize, authenticate, new/load session, prompt, cancel, session mode, an unstable model switch, and file-system proxy behavior. ([Gemini CLI][3])

Special handling:

```text
Gemini plan mode → Universal plan state machine.
Gemini ask_user → user_input.requested.
Gemini plan file → artifact.created + plan.updated.
Gemini MCP client exposure → capability supports_mcp_client.
```

Do not model Gemini plan approval as an ordinary “allow file edit” approval. It is a transition from planning to implementation.

### 9.4 Qwen adapter

Use ACP first.

Fallback to Qwen headless stream-json only for noninteractive or degraded support. Qwen documents `--output-format stream-json`, partial message events, and project-scoped resumable session data, but its `stream-json` input is described as under construction and intended for SDK use. ([Qwen][15])

Special handling:

```text
Qwen plan/default/auto-edit/yolo → universal mode + approval policy.
Qwen tool approvals → approval.requested.
Qwen partial messages → content.delta.
Qwen result object → turn.completed + usage/result summary.
```

Because Qwen has had implementation differences between interactive, noninteractive, and ACP paths, treat native events as potentially lossy or inconsistent and make your adapter resilient. ([GitHub][16])

### 9.5 OpenCode adapter

Use `opencode acp` for the baseline. OpenCode’s docs describe ACP support as a subprocess communicating over stdio JSON-RPC. ([opencode.ai][5])

Optionally add an enhanced OpenCode server adapter using `opencode serve` for richer management. The server exposes sessions, messages, aborts, diffs, reverts, permission replies, and SSE events. ([opencode.ai][17])

Special handling:

```text
OpenCode permission ask/allow/deny → universal approval policy.
OpenCode once/always/reject → approve_once / approve_always / deny.
OpenCode todowrite → plan.updated.
OpenCode session abort → cancel_turn.
OpenCode diff/revert endpoints → RequestDiff / RevertChange.
```

Be careful with default permissions. OpenCode’s config can allow, ask, or deny by permission category, and many permissions default to allow. For a unified approval UX, start OpenCode with stricter permission configuration and let your control-plane policy decide. ([opencode.ai][18])

---

## 10. State machines

### Session state machine

```text
new
  → starting
  → ready
  → running_turn
  → ready
  → closed

starting → error
ready → recovering
running_turn → recovering
recovering → ready | detached | error
```

Rules:

```text
Only one active turn per native harness session unless the adapter explicitly supports concurrency.
Multiple universal sessions may run concurrently.
A session can be ready while it still has unresolved historical approvals, but not while it has unresolved active-turn native approvals.
```

### Turn state machine

```text
queued
  → starting
  → running
  → awaiting_approval
  → running
  → awaiting_user_input
  → running
  → completing
  → completed
```

Failure/cancel paths:

```text
running → cancelling → cancelled
running → failed
running → interrupted
running → detached
awaiting_approval → cancelling → cancelled
awaiting_approval → detached
```

`interrupted` should mean the harness stopped gracefully but did not finish the intended work. `detached` should mean the control plane no longer knows whether the native turn is still running.

### Tool state machine

```text
created
  → awaiting_approval
  → running
  → output_streaming
  → completed
```

Side paths:

```text
awaiting_approval → declined
running → failed
running → cancelled
running → orphaned
```

### Approval state machine

```text
created
  → pending
  → presented
  → resolving
  → approved | denied | cancelled | expired | orphaned
```

Use `orphaned` only when you can no longer answer the native request because the runner or native process died.

### Plan state machine

```text
none
  → discovering
  → draft
  → awaiting_approval
  → approved
  → implementing
  → completed
```

Side paths:

```text
draft → revision_requested → draft
awaiting_approval → cancelled
implementing → failed
implementing → cancelled
```

---

## 11. Recent turn cache

You correctly called out recent turn caches. I would make them explicit and persistent.

For every active turn, keep:

```ts
type RecentTurnCache = {
  session_id: string;
  turn_id: string;

  native_prompt_request_id?: string;
  native_turn_id?: string;

  active_items_by_native_id: Map<string, string>;
  active_tools_by_native_id: Map<string, string>;
  pending_approvals_by_native_request_id: Map<string, string>;

  latest_plan_id?: string;
  latest_diff_id?: string;
  current_mode?: ModeState;

  content_buffers: Map<string, ContentBuffer>;
  outstanding_rpc: Map<string, OutstandingRpc>;

  last_native_event_offset?: string;
  last_universal_seq: bigint;
};
```

This should be persisted, not just kept in memory, because it is needed after:

```text
frontend reload
control-plane restart
runner reconnect
approval modal disappearing and reappearing
native event replay
```

The cache is not your source of truth. The durable event log is. The cache is a correlation accelerator and recovery aid.

### Why this matters

Native protocols often emit events in sequences like:

```text
tool item started
approval requested
approval resolved
tool output delta
tool completed
```

But in degraded cases you may see:

```text
approval requested
tool item update arrives late
frontend reconnects
approval decision resent
```

The reducer should be able to attach the approval to the correct item after the fact.

---

## 12. Durable event log and snapshots

Use an append-only event log:

```text
agent_events
  seq bigint primary key
  workspace_id
  session_id
  turn_id nullable
  event_type
  event_json
  native_json
  created_at
```

Then maintain materialized views:

```text
sessions_view
turns_view
items_view
approvals_view
plans_view
diffs_view
artifacts_view
```

Frontend subscription should work like:

```ts
subscribe({
  workspace_id,
  session_id,
  after_seq,
  include_snapshot: true
})
```

Response:

```ts
{
  snapshot: SessionSnapshot,
  events: UniversalEvent[],
  latest_seq: "12345"
}
```

On frontend reload:

```text
1. frontend reconnects
2. sends last seen seq
3. control plane sends current snapshot
4. control plane replays missed events
5. frontend resumes streaming
```

This makes reloads boring.

---

## 13. Connection interruption handling

### Frontend ↔ control plane interruption

This should not affect native execution.

```text
Frontend disconnects:
  control plane keeps turn running
  approvals remain pending
  events continue being stored

Frontend reconnects:
  fetch snapshot + replay after last_seq
  pending approval modals are reconstructed from state
```

Never tie native process lifetime to frontend WebSocket lifetime.

### Control plane ↔ runner interruption

Use a runner-local write-ahead log.

```text
Runner receives native event
  → writes local WAL
  → sends event to control plane
  → waits for ack, or retries
```

If connection drops:

```text
runner keeps reading native process
runner buffers events in WAL
pending native approvals stay blocked
runner reconnects
runner replays unacked events
```

If the runner has an unresolved native approval and loses control-plane connection, choose a policy:

```text
wait_indefinitely
deny_after_timeout
cancel_turn_after_timeout
```

For dev tools, I would default to `wait_indefinitely` for a short grace period, then `cancel_turn_after_timeout` only if configured. Silent auto-denial can corrupt the user’s mental model.

### Runner ↔ harness interruption

If the native child process exits or the stdio pipe breaks:

```text
turn.running → turn.failed
approval.pending → approval.orphaned
tool.running → tool.orphaned or failed
session.ready → session.error
```

Then attempt recovery only if the adapter has capability:

```text
can_load_session
can_resume_active_turn
can_replay_events
```

Codex has explicit thread resume/fork/list/read concepts in app-server, while ACP has session load where supported. ([GitHub][2])

But do not pretend an in-flight turn resumed unless the harness confirms it. Most CLI protocols can restore session history more reliably than they can restore an active blocked tool call.

### Control-plane restart

On restart:

```text
1. reload sessions marked running/recovering
2. reconnect to runners or mark runner_detached
3. replay runner WALs
4. rebuild materialized state from event log if needed
5. re-present unresolved approvals
```

### Native JSON-RPC request recovery

For native request/response protocols, unresolved native requests are dangerous.

Example:

```text
native harness → runner: request_permission(id=42)
runner → control plane: approval.requested
control plane dies
```

The runner must retain ownership of native request `42`. If the control plane returns, it can still answer. If the runner dies, request `42` is gone; mark the universal approval `orphaned` and fail/cancel the turn.

---

## 14. Policy engine

Do not let each harness be the policy authority. Let the control plane be the policy authority.

```ts
type PolicyDecision =
  | { action: "allow" }
  | { action: "ask"; reason: string }
  | { action: "deny"; reason: string }
  | { action: "rewrite"; patched_request: unknown };
```

Inputs:

```text
workspace trust level
harness
cwd
sandbox mode
tool kind
file path
command
diff
network target
user preferences
previous approve_always grants
```

Then map the result into each native protocol.

Examples:

```text
Codex:
  --ask-for-approval on-request
  --sandbox workspace-write

Qwen:
  approval-mode default or auto-edit depending on desired policy

Gemini:
  ACP permission handling + plan mode semantics

OpenCode:
  configure permissions to ask/deny instead of permissive defaults
```

Codex exposes approval and sandbox flags such as `--ask-for-approval` and `--sandbox`, while OpenCode has permission categories and allow/ask/deny config, so adapter startup config is part of policy enforcement. ([developers.openai.com][19])

---

## 15. Security and audit

For every approval and tool execution, persist:

```text
who approved
when
which frontend connection
native request id
canonical risk summary
raw native payload
exact command or diff
cwd
environment redaction policy
result
```

For shell commands, store:

```text
command
cwd
exit code
stdout/stderr refs
duration
sandbox profile
```

For file edits, store:

```text
path
before hash
after hash
diff
approval id
```

Avoid trusting native “safe” labels. The control plane should recompute risk where possible.

---

## 16. Frontend UX model

The frontend can render from universal state only:

```text
Session header
  harness, model, cwd, mode, connection status

Timeline
  user messages
  reasoning summaries
  assistant messages
  tool calls
  file edits
  command executions
  web searches
  plan updates

Right rail
  active plan
  pending approvals
  diffs
  artifacts
  logs

Footer
  prompt input
  mode selector
  cancel button
```

Feature flags come from capabilities:

```text
No plan support?
  hide plan rail or show synthesized todo only.

No native diff support?
  show file artifacts but disable inline native diff.

No approve_always?
  show only approve once / deny.

No active-turn resume?
  after runner loss show “turn detached” and offer restart/resume session, not resume turn.
```

---

## 17. Suggested universal protocol shape

Use WebSocket or SSE+HTTP between frontend and control plane.

### Frontend → control plane

```json
{
  "type": "command",
  "command_id": "cmd_123",
  "idempotency_key": "approval_appr_1_decision_v1",
  "command": {
    "type": "approval.resolve",
    "session_id": "sess_1",
    "turn_id": "turn_1",
    "approval_id": "appr_1",
    "decision": {
      "kind": "approve_once"
    }
  }
}
```

### Control plane → frontend

```json
{
  "type": "event",
  "seq": "1042",
  "event_id": "evt_1042",
  "session_id": "sess_1",
  "turn_id": "turn_1",
  "event": {
    "type": "approval.resolved",
    "approval_id": "appr_1",
    "status": "approved",
    "decision": {
      "kind": "approve_once"
    }
  }
}
```

### Approval requested

```json
{
  "type": "event",
  "seq": "1038",
  "event_id": "evt_1038",
  "session_id": "sess_1",
  "turn_id": "turn_1",
  "item_id": "item_cmd_7",
  "source": "codex",
  "event": {
    "type": "approval.requested",
    "approval": {
      "approval_id": "appr_1",
      "kind": "shell_command",
      "title": "Run shell command",
      "subject": {
        "command": "cargo test",
        "cwd": "/repo"
      },
      "options": [
        { "id": "approve_once", "label": "Approve once" },
        { "id": "deny", "label": "Deny" }
      ],
      "status": "pending"
    }
  },
  "native": {
    "protocol": "codex_app_server",
    "type": "requestApproval",
    "id": "42",
    "payload": {}
  }
}
```

---

## 18. Schema/versioning strategy

Use:

```text
Universal protocol version: uap/1
Adapter profile version: codex-app-server/2026-xx
Native schema version: recorded per session
```

For Codex, generate or vendor native schemas per CLI version where possible; Codex app-server docs describe generating TypeScript and JSON schema from the CLI. ([GitHub][2])

For Rust:

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UniversalEventKind {
    SessionCreated(SessionCreated),
    TurnStarted(TurnStarted),
    ContentDelta(ContentDelta),
    ToolUpdated(ToolUpdated),
    ApprovalRequested(ApprovalRequested),
    PlanUpdated(PlanUpdated),
    DiffUpdated(DiffUpdated),
    // ...
    UnknownNative(UnknownNativeEvent),
}
```

For native protocols:

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum NativeMessage {
    Known(KnownNativeMessage),
    Unknown(serde_json::Value),
}
```

Unknown native messages should become:

```text
native.unknown
```

not crashes.

---

## 19. Testing strategy

You want four test layers.

### 1. Golden native traces

Record real native sessions:

```text
codex: shell approval, file approval, plan update, cancel
gemini: plan mode, ask_user, implementation approval
qwen: default approval, auto-edit, plan mode, stream-json partials
opencode: bash permission, edit permission, todowrite, abort
```

Then assert universal event output.

### 2. Reducer property tests

Test invariants:

```text
no completed turn has pending active approval
approval cannot resolve twice unless idempotent same decision
seq is monotonic
tool cannot complete before creation unless reducer synthesizes creation
plan replace-all does not duplicate entries
cancelled turn eventually closes pending native approvals
```

### 3. Chaos tests

Inject:

```text
frontend reconnect during approval
control-plane restart during tool output
runner disconnect during native permission request
harness process crash during shell command
duplicate native events
out-of-order native updates
partial JSONL frame
```

### 4. Cross-harness conformance tests

Same scripted user story across all harnesses:

```text
“Inspect repo, make a plan, ask before edits, implement, run tests.”
```

Expected universal milestones:

```text
turn.started
plan.updated or synthetic plan
approval.requested for edit or shell, depending on mode
tool.completed
diff.updated or artifact.created
turn.completed
```

---

## 20. Implementation roadmap

I would build in this order:

1. **Universal event log + snapshot reducer**
   Do this before perfecting adapters. It becomes your debugging backbone.

2. **Generic ACP adapter**
   Covers Gemini, Qwen, and OpenCode baseline.

3. **Codex native app-server adapter**
   Gives you the richest lifecycle and validates that the universal protocol is truly a superset.

4. **Approval engine**
   Normalize approve once, always allow, deny, cancel, feedback, and plan approval.

5. **Plan reducer**
   Handle structured plans, markdown plans, todo-derived plans, and synthetic plans.

6. **Runner WAL and reconnect protocol**
   This is essential before serious frontend work.

7. **Enhanced OpenCode server adapter**
   Add only if you need server-side sessions, diffs, reverts, and SSE beyond ACP.

8. **Qwen stream-json fallback**
   Useful for headless/batch, but do not make it the primary orchestration path.

---

## 21. The most important design choices

### Use Codex native protocol, not only ACP

If you force Codex through ACP for uniformity, you’ll lose useful structure. It is better to normalize Codex’s richer native events into your superset and let other harnesses simply have fewer capabilities.

### Make the frontend protocol event-sourced

Frontend reloads, reconnects, pending approvals, and long-running turns all become tractable if the frontend is just a projection of durable events.

### Treat approvals as blocking protocol obligations

An approval is not just UI state. It may correspond to a native JSON-RPC request that must be answered exactly once.

### Separate plan approval from tool approval

“Approve this plan and start implementation” is semantically different from “allow this shell command.”

### Capability-gate everything

No frontend branch should say:

```ts
if (harness === "gemini")
```

It should say:

```ts
if (capabilities.plan.supports_plan_mode)
```

### Preserve raw native events

You will need them constantly. Protocols evolve, adapters will be wrong, and ambiguous semantics will surface. Raw native payloads are your escape hatch.

---

## Open questions I’d want answered

1. Do you want Codex to use its native app-server protocol, or are you trying to force every harness through ACP even if that reduces fidelity?

2. Should the harness execute tools itself, or should your control plane provide file-system/terminal tools and make the harness request them through your system? The second option gives better audit and policy control, but requires more adapter work.

3. Is one active turn per session acceptable? I would strongly recommend that as the default, with parallelism achieved through multiple sessions.

4. For pending approvals during control-plane/runner outages, should the default be “wait indefinitely,” “deny after timeout,” or “cancel turn after timeout”? My recommendation is wait through transient reconnects, then cancel only by explicit configured policy.

[1]: https://agentclientprotocol.com/protocol/overview "Overview - Agent Client Protocol"
[2]: https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md "codex/codex-rs/app-server/README.md at main · openai/codex · GitHub"
[3]: https://geminicli.com/docs/cli/acp-mode/ "ACP Mode | Gemini CLI"
[4]: https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/ "Qwen Code Configuration | Qwen Code Docs"
[5]: https://opencode.ai/docs/acp/ "ACP Support | OpenCode"
[6]: https://agentclientprotocol.com/protocol/tool-calls "Tool Calls - Agent Client Protocol"
[7]: https://agentclientprotocol.com/protocol/schema "Schema - Agent Client Protocol"
[8]: https://pkg.go.dev/github.com/coder/acp-go-sdk "acp package - github.com/coder/acp-go-sdk - Go Packages"
[9]: https://agentclientprotocol.com/protocol/agent-plan?utm_source=chatgpt.com "Agent Plan"
[10]: https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/plan-mode.md "gemini-cli/docs/cli/plan-mode.md at main · google-gemini/gemini-cli · GitHub"
[11]: https://github.com/QwenLM/qwen-code/blob/main/docs/users/features/approval-mode.md "qwen-code/docs/users/features/approval-mode.md at main · QwenLM/qwen-code · GitHub"
[12]: https://opencode.ai/docs/tools/ "Tools | OpenCode"
[13]: https://agentclientprotocol.com/protocol/session-modes "Session Modes - Agent Client Protocol"
[14]: https://developers.openai.com/codex/noninteractive "Non-interactive mode – Codex | OpenAI Developers"
[15]: https://qwenlm.github.io/qwen-code-docs/en/users/features/headless/ "Headless Mode | Qwen Code Docs"
[16]: https://github.com/QwenLM/qwen-code/issues/3247 "refactor: Unify Tool Execution Logic Across Interactive, Non-Interactive, and ACP Modes · Issue #3247 · QwenLM/qwen-code · GitHub"
[17]: https://opencode.ai/docs/server/ "Server | OpenCode"
[18]: https://opencode.ai/docs/permissions/ "Permissions | OpenCode"
[19]: https://developers.openai.com/codex/cli/reference "Command line options – Codex CLI | OpenAI Developers"
