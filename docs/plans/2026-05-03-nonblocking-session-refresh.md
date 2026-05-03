# Non-Blocking Session Refresh

Status: implemented
Date: 2026-05-03

## Goal

Runner session discovery must not block the control plane or browser UI for minutes while provider history is read and imported.

## Implemented Behavior

- Workspace/provider refresh starts a background job and returns `202 Accepted` with `refresh_id`.
- Refresh status is available at `GET /api/workspaces/{workspace_id}/providers/{provider_id}/sessions/refresh/{refresh_id}`.
- Runner refresh commands acknowledge quickly; provider discovery runs in a background task and reports results or errors with the original request id.
- Codex startup discovery sends metadata only and does not read every thread history during runner connection.
- Control-plane `SessionsDiscovered` imports run off the runner WebSocket receive loop. DB-backed runner event ack is delayed until the import task reports success.
- Forced DB imports replace a session projection in bulk and store an imported-history fingerprint so unchanged histories can skip rewrite work.
- `DiscoveredSessionHistoryStatus::NotLoaded` distinguishes metadata-only discovery from failed history reads.
- Runner refresh is now an explicit operation state machine over `RunnerEvent::OperationUpdated` with `queued`, `accepted`, `discovering`, `reading_history`, `sending_results`, `importing`, `succeeded`, `failed`, and `cancelled` states.
- Refresh status responses include operation progress and a bounded log so the browser can show live refresh progress without treating the transient refresh state as a chat websocket failure.
- The sidebar renders refresh progress inline with a progress bar, collapsible log, retry for failed refreshes, and terminal summaries.
- Chat websocket subscription errors that require intentional close no longer schedule an immediate reconnect loop.

## Verification

- `cargo test -p agenter-control-plane refresh_workspace_sessions_sends_runner_command_and_rewrites_history`
- `cargo test -p agenter-control-plane import_`
- `cargo test -p agenter-protocol round_trips_operation_updated_event`
- `cargo test -p agenter-control-plane refresh_operation_update_advances_job_and_records_log`
- `npm run test -- sessions.test.ts` from `web/`

Run the full Rust and web gates from `docs/harness/VERIFICATION.md` before considering a release branch complete.
