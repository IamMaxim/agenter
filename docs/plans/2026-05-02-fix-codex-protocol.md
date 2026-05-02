# Codex protocol coverage vs Codex app-server/TUI (`tmp/codex`)

**Purpose:** Inventory gaps between Agenter (`crates/agenter-runner`, `crates/agenter-control-plane`) and upstream Codex’s typed app-server protocol and TUI behavior (`codex-rs/app-server-protocol`, `codex-rs/tui`), so we can fix incorrect or missing bridging deliberately.

**Reference sources:**

- Canonical wire surface: `tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs` (`ClientRequest`, `ServerRequest`, `ServerNotification`).
- Runner bridge: `crates/agenter-runner/src/agents/codex.rs` (`CodexAppServer`, `run_codex_turn_on_server`, `normalize_codex_*`).
- TUI parity target: `tmp/codex/codex-rs/tui/src/app/app_server_adapter.rs`, `app_server_requests.rs`.

---

## Server → client requests (severe)

1. **`item/tool/call` (`DynamicToolCall`) — rejected as “unsupported”.** Codex sends this request with a typed response contract; TUI resolves it (`app_server_requests.rs`, tests in `codex.rs` expect `-32601`). Agenter replies with JSON-RPC `-32601` instead of bridging to a capability (hosted tool invocation, surfaced question, or at minimum a coherent decline). Sessions that rely on dynamic tools therefore diverge sharply from Codex UX.

2. **`account/chatgptAuthTokens/refresh` — rejected as unsupported.** In the TUI this is a handled path (paired with onboarding/auth flows). Agenter denies it outright, so authenticated flows that rely on token refresh behave differently remote vs local TUI.

3. **Deprecated approval methods `execCommandApproval` / `applyPatchApproval` — not bridged.** Both remain in upstream `ServerRequest` for legacy turns (`common.rs`; TUI handlers in `app_server_requests.rs`). `normalize_codex_approval_request` only recognizes `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, and `item/permissions/requestApproval`. Legacy approval JSON-RPC arrives with CamelCase-ish method names (`execCommandApproval`, etc.). Those hit `unsupported_codex_server_request` and get `-32601` instead of surfacing Agenter approvals.

4. **Over-broad classification as “unsupported server requests”.** `unsupported_codex_server_request` returns `Some` for any inbound object with `"id"` and `"method"` that was not classified as approval/question (`codex.rs`). After any new Codex protocol request type is introduced upstream, Agenter will auto-reply with error until mapped; there is no allowlist/opt-in and no degraded “defer to human” UX beyond the generic rejection.

---

## Transport / correlation (severe)

5. **`read_response` ignores all traffic until numeric `id` matches.** Implemented as `message.get("id").and_then(Value::as_u64) != Some(request_id)` (`CodexAppServer::read_response`). Codex defines `RequestId` as integer **or string** (`app-server-protocol/src/jsonrpc_lite.rs`). If the peer returns a response whose `id` is a JSON string, Agenter hangs until startup timeout rather than acknowledging the correlation.

6. **Same filtering drops interleaved outbound notifications.** While waiting inside `read_response`, any Codex notifications (methods without matching pending client `id`), or outbound server requests awaiting user action on another channel, are read from stdout and silently discarded—not queued for later processing. Codex transports everything on one newline-delimited JSON stream; Agenter multiplexing only reliably works in `run_codex_turn_on_server`’s unconsumed loop **after** the handshake methods return—risking lost events during initialize / thread/start / resume / turn/start / provider commands whenever the SDK emits unsolicited messages before the matching response arrives.

---

## Turn scoping vs interactive requests (severe)

7. **Approvals/questions are gated behind `codex_message_belongs_to_scope`.** Incoming messages are discarded when thread/turn parameters disagree with inferred `CodexTurnScope`, **before** `normalize_codex_approval_request` / `normalize_codex_question_request` (`run_codex_turn_on_server`). The TUI does not silently drop approvals for being “outside” a synthesized turn correlation; mismatched routing can starve Codex of responses and wedge the rollout. Particularly risky when server requests omit redundant turn identifiers or thread IDs mismatch early scope inference.

---

## Notifications: partial mapping vs degraded `ProviderEvent`

8. **Many first-class Codex notifications are collapsed into `_ => provider_event` fall-through.** Dedicated handling exists for a narrow set (`agentMessage` deltas/completions indirectly, token usage/rate limits, plan updates, limited command/file-change streaming, generic thread/turn/session status). Notifications the TUI models explicitly—including but not limited to:
   - `thread/started`, `thread/archived`, `thread/unarchived`, `thread/closed`, `thread/name/updated`, `skills/changed`
   - `turn/diff/updated`
   - `hook/started`, `hook/completed`
   - `item/autoApprovalReview/started`, `item/autoApprovalReview/completed`
   - `serverRequest/resolved`
   - `item/mcpToolCall/progress`, `item/commandExecution/terminalInteraction`
   - `item/reasoning/summaryTextDelta`, `item/reasoning/summaryPartAdded`, `item/reasoning/textDelta`
   - `rawResponseItem/completed`
   - `mcpServer/oauthLogin/completed`, `mcpServer/startupStatus/updated`
   - `model/verification`, assorted warning/deprecation channels
   - experimental realtime/audio cluster under `thread/realtime/*`
   - fuzzy search session lifecycle (`fuzzyFileSearch/sessionUpdated`, `/sessionCompleted`)
   - sandbox warnings (`windows/*`, etc.)
   
   arrive as coarse `ProviderEvent` rows (`provider_event_category`). Web UI/state cannot mirror TUI fidelity without inspecting raw `provider_payload`.

9. **Legacy wire names for assistant text deltas:** Agenter recognizes `agentMessage/delta`, `agentMessage/completed`, and `agentMessage/complete`; upstream `ServerNotification` uses `item/agentMessage/delta` and completion semantics via items (`normalize_codex_message_inner` mixes both). Not wrong, but divergence from Codex enums means regressions slip in when Codex trims legacy aliases.

---

## Client → server: initialize + capabilities divergence

10. **`initialize` omits knobs Codex exposes to clients.** Runner sends `{ experimentalApi: true }` only (`CodexAppServer::initialize`). The protocol carries fields like optional `optOutNotificationMethods`, experimental toggles mirrored in Codex IDE/TUI adapters. Omitting parity means Agenter differs in which notifications Codex emits and how noisy the stream is versus TUI defaults.

---

## Slash / thread operations vs richer `ClientRequest` surface

11. **Provider slash commands proxy only `codex.{compact,review,steer,fork,archive,unarchive,rollback,shell}`.** The upstream `client_request_definitions` block exposes many more knobs (interrupt/steering variants already partially covered; `thread/inject_items`, MCP marketplace/plugin tooling, realtime voice stack, fuzzy search sessions, `thread/approveGuardianDeniedAction`, `thread/metadata/update`, sandbox setup, standalone `command/exec` sessions, filesystem helpers, collaborative memory modes). None of those are surfaced through runner slash plumbing today, so parity with Codex-local workflows is inherently partial.

---

## Control-plane ingestion nuance (`agenter-control-plane`)

12. **Usage overlays only hydrate from `ProviderEvent` categories.** `publish_event` merges token/rate-limit context via `usage_snapshot_from_provider_event` when `category` is `"token_usage"` or `"rate_limits"` (`state.rs`). Runner emits those categories for Codex notifications. Other specialized `AppEvent` variants emitted by Codex bridging (plans, deltas, tooling) bypass this path—fine for usage, but any future Codex-hosted usage reporting that moves off those payloads will silently stop updating dashboards unless extended.

---

## Follow-up work (recommended order — superseded)

The concrete phased plan below replaces this bullets-only list.

## Execution status (implemented 2026-05-02)

- [x] Normalize request-id matching to protocol shapes (`Integer` + numeric-string fallback) and propagate typed IDs through `send_request/read_response/take_pending_codex_jsonrpc_response`.
- [x] Preserve protocol traffic while awaiting synchronous responses via queueing unmatched frames in `CodexAppServer`.
- [x] Handle `account/chatgptAuthTokens/refresh` as an explicit remote-auth dead-end with structured error guidance.
- [x] Keep interactive request paths (`approval`, `question`, explicit unsupported fallback) before scope filtering in rollout loop.
- [x] Guard `run_codex_turn_on_server` against unexpected response-like frames before thread/turn scope filtering, while keeping interactive request handling isolated from scope.
- [x] Add regression coverage for mixed-id response matching and approval mapping for legacy approval methods.
- [x] Expand notification parity for high-value TUI-only method coverage (`turn/diff/updated`, `item/reasoning/*`, `serverRequest/resolved`, MCP/realtime updates).
- [x] Extend frontend contracts (`ProviderEvent`/new AppEvent variants) for the expanded notification surface.

---

## Implementation roadmap (step-by-step)

Phases build on earlier layers: fixing the stdio multiplexing bug first avoids chasing phantom notification gaps in later QA.

### Phase 1 — Robust JSON-RPC line handling (`agenter-runner`)

1. **`RequestId` correlation in `CodexAppServer::read_response`** (`codex.rs`):
   - Stop matching only `as_u64()`. Normalize inbound `id` to a comparable value (reuse shape Codex emits: integers and strings per `codex-app-server-protocol` `RequestId`).
   - Prefer matching whatever id was assigned on the outbound request (currently monotonic numeric `next_id`; keep sending numbers for simplicity, but still accept string-echo replies if Codex starts emitting them).

2. **Drain-and-queue semantics while awaiting a synchronous response:**
   - Introduce a bounded internal queue fed from the stdout reader whenever `next_message()` parses successfully.
   - `read_response` waits for `{ id matches pending } AND (has result||error)` specifically, not arbitrary messages.
   - All other inbound objects (notifications, unsolicited server→client requests) enqueue for the caller that drives the rollout loop (`run_codex_turn_on_server` or future shared pump).
   - Ensure `initialize`, `thread/start`, `thread/resume`, `thread/list`, `thread/read`, `model/list`, `collaborationMode/list`, provider slash RPCs share this pump so nothing is silently dropped during boot.

3. **Tests:** Add regression tests covering (a) interleaved notifications before a matching response row, (b) string-vs-number `RequestId`, (c) starvation / queue overflow policy (prefer drop-oldest with loud tracing if bounding is unavoidable — document chosen policy in this plan).

**Exit criterion:** Startup and turn-driving paths no longer lose Codex outbound traffic masked as “waiting for response”; hung correlation on string ids is eliminated.

---

### Phase 2 — Correct interactive request routing (`agenter-runner`)

4. **Reorder `run_codex_turn_on_server` processing** so approvals, questions, and other server→client requests that require a correlated JSON-RPC reply are evaluated **even when scope metadata disagrees**.
   - Keep optional **soft** threading: log mismatches prominently, attach `provider_payload`.
   - Policy: interactive requests (`id` present, `method` present, not a Response object) **must receive** either a routed handler response or an explicit structured JSON-RPC error; never silent `continue`.

5. **Tighten `unsupported_codex_server_request` eligibility** (`codex.rs`):
   - Only treat as unsupported after explicit match arms for known-but-unimplemented methods.
   - Optionally require shape hints (presence of `"params"` and absence of `"result"`/`"error"`) consistent with inbound requests to avoid accidental misclassification — align with Codex `JSONRPCRequest`/`JSONRPCResponse` distinctions.

**Exit criterion:** No interactive Codex requests go unanswered because of stale `CodexTurnScope` inference during a rollout.

---

### Phase 3 — `account/chatgptAuthTokens/refresh` — operator guidance (runner + frontend)

**Requirement:** Refresh **cannot** be completed from the browser. Treat as a **fatal-to-remote-auth** marker: persist an explicit `AppEvent::Error`, show it in chat, instruct SSH to runner host + run Codex/login manually.

6. **Runner — dedicated branch before generic unsupported handling** (`run_codex_turn_on_server`, `codex.rs`):
   - Detect JSON-RPC requests where `"method"` is exactly `account/chatgptAuthTokens/refresh` (and legacy camelCase serialization if Codex emits it anywhere — grep upstream fixtures; if unreachable, omit).
   - Emit `AppEvent::Error(AgentErrorEvent {
        session_id: Some(active_session_id),
        code: Some("codex_auth_refresh_required".to_owned()),
        message: concise operator-facing prose (below),
        provider_payload: Some(cloned request envelope),
      })`.
   - Proposed **`message`** string (runner-owned, single paragraph for API stability):
     > Codex login or token refresh is required on the runner host (401 / token refresh requested). SSH into the workspace runner machine, authenticate with Codex CLI in that environment (same user/service account running `agenter-runner`), then retry this chat turn.
   - **Reply to Codex** with JSON-RPC **`error`** (not a fabricated `ChatgptAuthTokensRefreshResponse`; upstream requires real `access_token` / `chatgpt_account_id`).
     - Prefer a deterministic application error (`code` −32001-style or Codex-internal reason string in `error.data.method`) documenting “remote runner cannot mint tokens”; keep message short enough for Codex logs.
     - **Do not** send success with empty credentials.

7. **Control-plane (`agenter-control-plane`):**
   - No schema change strictly required (`AgentErrorEvent` already persists). Optionally `publish_event`: if matching `codex_auth_refresh_required`, annotate tracing at `WARN` level for ops dashboards — nice-to-have.

8. **Web UI (`web/src/lib/chatEvents.ts` + whichever row renders `kind: 'error'`):**
   - When `payload.code === 'codex_auth_refresh_required'` (or error title contains keyword fallback), prepend a titled callout rendering the same guidance with emphasis on SSH + **`codex` / app-server login local to runner**, not Control Plane OAuth.
   - Optional: styled “Operator action” inset so operators distinguish infra auth from transient model errors (`InlineEventRow` / error row styling — keep consistent with existing `error` bubble).

**Exit criterion:** When Codex issues refresh, operators see pinned chat-side instructions; Codex receives an explicit denial so the rollout stops retrying blindly without local auth.

---

### Phase 4 — Legacy approvals (`ExecCommandApproval`, `ApplyPatchApproval`)

9. Extend `normalize_codex_approval_request` to recognize **`execCommandApproval`** and **`applyPatchApproval`** wire methods (CamelCase variants from serde, confirm against `serde_json::to_value` of `ServerRequest` in Codex fixtures).
10. Map to existing Agenter semantics:
    - `ExecCommandApproval` → same UX as modern command approvals (`ApprovalKind::Command`).
    - `ApplyPatchApproval` → `ApprovalKind::FileChange`.
11. Reuse approval response builders; if payload shapes diverge (`v1` params), add adapters similar to cache-driven file-change presentation (`codex_approval_context`).
12. **Tests:** Fixtures from Codex snapshot `app-server-protocol` serde roundtrips embedded as JSON in agenter-runner tests.

**Exit criterion:** Legacy threads stop receiving automated `-32601` for approval paths Codex still emits.

---

### Phase 5 — `item/tool/call` (`DynamicToolCall`) — degraded path

13. Prefer **explicit** branch (still before generic unsupported):
    - Emit `AppEvent::Error` OR `ProviderEvent` with stable category **`codex_capability_gap`** explaining dynamic tools unsupported remotely.
    - JSON-RPC **`error`** to Codex mirrors current test expectation semantics (currently `-32601` in-runner test) — reconcile copy with product: either keep `-32601` with clarified message body or standardized custom code documented in AGENTS/decisions once chosen.
14. Longer term (separate milestone): expose dynamic tool execution through Agenter-hosted executor **only** after capability review — out of immediate scope unless product demands parity.

---

### Phase 6 — Notifications & fidelity (runner + incremental UI)

15. Identify top-N missing notifications from Gap **#8** that affect observable UX (prioritize **`item/reasoning/*`**, **`serverRequest/resolved`**, **`turn/diff/updated`**, MCP progress if multi-step approvals depend on UI state).
16. For each chosen method: introduce typed `AppEvent` variant OR extend `ProviderEvent` contract with **`method`** field (non-breaking additive) consumed by ChatRoute for richer formatting — record decision under `docs/decisions/` once shape chosen.
17. **`initialize` parity**: mirror Codex TUI/opt-out knobs where low-cost (whitelist `optOutNotificationMethods`), behind env flag until UI opts in.

---

### Phase 7 — Client surface parity (incremental product)

18. **`turn/interrupt`** — expose via Control Plane HTTP + runner forwarding (parity with Ctrl+C in Codex TUI).
19. Thread metadata / MCP admin / marketplace — backlog only unless product prioritizes remote admin from Agenter shell.

---

## Verification (per harness)

After each phase, run checks from `docs/harness/VERIFICATION.md`; add targeted integration spikes (documented runbook) exercising refresh + denial path and legacy approvals against a pinned Codex CLI where feasible.

---

_Last updated: 2026-05-02 — compare against snapshots under `tmp/codex` and Agenter crates above._
