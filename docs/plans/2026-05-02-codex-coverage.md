# Codex protocol + TUI coverage vs `tmp/codex` (2026-05-02)

## Scope
- Project crates checked: `crates/agenter-runner`, `crates/agenter-control-plane`
- Golden reference: `tmp/codex` app-server protocol + TUI request/notification flow
- Baseline goal: protocol correctness and behavior parity, not just “something renders.”

## Protocol correctness gaps (high confidence)

1. **Request-id equality is unnecessarily numeric-first and drops general JSON-RPC id behavior**
   - `CodexAppServer::read_response`/`take_pending_codex_jsonrpc_response` accepts only `u64` request ids and compares with `codex_jsonrpc_request_ids_equal`, which allows string ids only when they parse as integer or equal the string form of the expected number (`crates/agenter-runner/src/agents/codex.rs:2078-2091`).
   - `RequestId` in protocol is `String | Integer` (`tmp/codex/codex-rs/app-server-protocol/src/jsonrpc_lite.rs:17-21`), so arbitrary string ids are not faithfully supported.

2. **Initialize capabilities are incomplete and protocol-underspecified**
   - Runner sends only `{"experimentalApi": true}` (`crates/agenter-runner/src/agents/codex.rs:147-157`).
   - Protocol also supports `optOutNotificationMethods` in `InitializeCapabilities` (`tmp/codex/codex-rs/app-server-protocol/src/protocol/v1.rs:45-53`) and runtime code should preserve that optional contract where possible.

3. **Server request handling is effectively “known/unsupported” with no generic request fallback semantics**
   - Unknown server-to-client request envelopes are treated as unsupported for any object that is a request (`codex_rpc_is_codex_server_to_client_request`) and replied with `-32601` (`crates/agenter-runner/src/agents/codex.rs:1442-1448, 1450-1457`).
   - This is stricter than the protocol contract, where unrecognized methods may arrive during feature drift and should be surfaced as explicit fallback behavior rather than silently rejected.

4. **`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, and `item/permissions/requestApproval` are implemented, but request surface is still tiny compared to protocol**
   - Runner maps requests via slash/turn plumbing and ad-hoc handlers only; protocol client surface includes many other request families (`tmp/codex/codex-rs/protocol/common.rs:95-239, 620-651`).
   - Practical gap: no runnable bridge for these families in `agenter-runner` command path.

## Turn/notification routing risks

5. **Interactive request precedence is strict and can discard protocol messages**
   - In `run_codex_turn_on_server`, every inbound server request is first tested for approvals/questions/unsupported. If none match and the message is out of inferred `thread/turn` scope, it is dropped (`crates/agenter-runner/src/agents/codex.rs:852-999`).
   - TUI side keeps request/notification pathing explicit per method/thread and does not require this same hard scope discard before classification (`tmp/codex/codex-rs/tui/src/app/app_server_adapter.rs:143-214, 208-236`).

6. **Scope suppression is applied before all stream events once turn loop is running**
   - `normalize_codex_message_inner` supports many methods, but `normalize_codex_message_for_scope` rejects non-matching thread/turn ids (`crates/agenter-runner/src/agents/codex.rs:1101-1109, 2118-2125`).
   - That makes missing thread/turn ids (or mismatches in older/legacy flows) non-deterministic from Agenter’s perspective.

## Codex TUI parity findings (where Agenter currently diverges)

The project currently has **no native Codex TUI**; parity is against upstream event modeling and request semantics in `tmp/codex`.

7. **Server-request handling matrix is narrower**
   - TUI has first-class support for pending request lifecycle for approvals, MCP elicitation, user input, dynamic rejection, and structured queueing (`tmp/codex/codex-rs/tui/src/app/app_server_requests.rs`).
   - Agenter special-cases only approval/question-style branches and then sends `-32601` for most others (`crates/agenter-runner/src/agents/codex.rs:857-979`).

8. **Notification types handled in TUI are much richer than event surface in runner/web**
   - TUI request/notification dispatcher explicitly routes many variants (thread/started, thread/status, hook started/completed, turn/diff/updated, thread/name/updated, reasonings, model verification, fuzzy search sessions, mcp tool progress, realtime channels, account/session signals, etc.) in `server_notification_thread_events` and matching target routing (`tmp/codex/codex-rs/tui/src/app/app_server_adapter.rs:335-457, 486-691`).
   - Runner reduces almost all unmatched methods to `ProviderEvent` (`crates/agenter-runner/src/agents/codex.rs:1130-1183`).

9. **Auth refresh is special-cased in runner but loses Codex-native completion path**
   - Agenter emits `AppError` + `-32002` application error for `account/chatgptAuthTokens/refresh`, which is functional but not equivalent to TUI “normal request lifecycle” in all branches (`crates/agenter-runner/src/agents/codex.rs:857-877`).

10. **Some TUI-visible legacy requests are intentionally unsupported there but accepted here**
   - TUI marks `DynamicToolCall` and legacy approval methods as unsupported/degraded in request queueing (`tmp/codex/codex-rs/tui/src/app/app_server_requests.rs:112-134, 335-338`), while Agenter currently maps `execCommandApproval`/`applyPatchApproval` through `normalize_codex_approval_request` (`crates/agenter-runner/src/agents/codex.rs:1186-1206`).
   - This is a parity direction mismatch, not a strict defect by itself; document as behavior contract divergence.

## Control-plane + web pipeline fidelity

11. **App event contract is intentionally compact, so TUI-style typed rows are unavailable**
   - `AppEventType` excludes rich server-notification-specific types and mostly exposes only coarse `provider_event` for many flows (`web/src/api/types.ts:164-185`).
   - `normalizeBrowserEventEnvelope` preserves unknown types as `error` for unrecognized `type`, which can mask or flatten real protocol notifications rather than surfacing explicit variants (`web/src/lib/normalizers.ts:124-134, 304-306`).

12. **Provider usage ingestion only reacts to two categories**
   - `SessionUsageSnapshot` updates are derived only from `token_usage` and `rate_limits` provider categories (`crates/agenter-control-plane/src/state.rs:1746-1751`), and provider payloads outside those categories are ignored for usage persistence.

13. **Notification fidelity in UI depends on provider category only**
   - `chatEvents` renders `provider_event` generically (`web/src/lib/chatEvents.ts:335-343`) and only includes full structured data through payload details, which is lower-fidelity than TUI's typed event stream mapping.

## Concrete action order

1. Introduce typed `RequestId` match helper against protocol enum semantics and update response matching to avoid id-shape regressions.
2. Preserve more server-request lifecycle semantics (queue/defer/unsupported) and avoid pre-scope discards.
3. Expand `normalize_codex_message_inner` and protocol-method dispatch to emit dedicated typed events for the high-value methods TUI maps explicitly.
4. Extend web `AppEventType` and control-plane event normalization path for key TUI-covered notifications (thread lifecycle, turn diff, reasoning, mcp/tool/realtime, hook, guardian review, fuzzy search sessions, account/skill/rate limit transitions).
5. Add regression tests that pin Codex JSON-RPC message shapes for both numeric/string ids and additional request/notification families.
