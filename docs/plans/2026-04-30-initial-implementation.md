# Initial Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Agenter from documentation-only harness into a minimal useful remote browser chat backed by runner-hosted Codex and Qwen protocol adapters, then add messenger projections.

**Architecture:** Use a Rust control-plane service, Rust runner daemon, shared typed protocol/domain crates, Postgres storage, and a Svelte browser UI. Implement protocol spikes first, then stabilize shared types and API boundaries before adding full connectors.

**Tech Stack:** Rust, Tokio, Axum, SQLx, Postgres, WebSocket, Svelte, TypeScript, Telegram Bot API, Mattermost REST/WebSocket, Codex app-server, Qwen ACP.

---

## Current State

- Repository contains harness documentation and the initial source discussion in `docs/chatgpt/001_initial.md`.
- Approved architecture decisions are recorded in `docs/decisions/`.
- Rust workspace skeleton exists with control-plane, runner, core, protocol, and db crates.
- Baseline Rust verification is active: `cargo fmt --all -- --check`, `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`.
- Provider protocol spike binaries are the next implementation target.

## Milestone 0: Protocol Spikes and Repo Baseline

Exit criterion: both provider protocols have local spike runbooks and either working spike evidence or clearly recorded blockers. The repository has a Rust workspace skeleton only after the spike plan is documented.

### Task 0.1: Add Protocol Spike Runbooks

**Files:**

- Create: `docs/runbooks/codex-app-server-spike.md`
- Create: `docs/runbooks/qwen-acp-spike.md`
- Modify: `docs/harness/MEMORY.md`

- [x] Write the Codex runbook with prerequisites, exact commands to locate `codex`, start `codex app-server`, send initialize, create/resume a thread, send a turn, trigger one approval request, and record observed JSON-RPC shapes.
- [x] Write the Qwen runbook with prerequisites, exact commands to locate `qwen`, start `qwen --acp`, initialize ACP, create/resume a session when supported, send a prompt, trigger one permission request, and record observed JSON-RPC shapes.
- [x] Add a recent note to `docs/harness/MEMORY.md` linking both runbooks.
- [x] Run `find . -maxdepth 4 -type f | sort` and check that docs have no placeholder sections.
- [x] Commit with `docs: add provider protocol spike runbooks`.

### Task 0.2: Create Rust Workspace Skeleton

**Files:**

- Create: `Cargo.toml`
- Create: `crates/agenter-core/Cargo.toml`
- Create: `crates/agenter-core/src/lib.rs`
- Create: `crates/agenter-protocol/Cargo.toml`
- Create: `crates/agenter-protocol/src/lib.rs`
- Create: `crates/agenter-db/Cargo.toml`
- Create: `crates/agenter-db/src/lib.rs`
- Create: `crates/agenter-control-plane/Cargo.toml`
- Create: `crates/agenter-control-plane/src/main.rs`
- Create: `crates/agenter-runner/Cargo.toml`
- Create: `crates/agenter-runner/src/main.rs`
- Modify: `docs/harness/VERIFICATION.md`

- [x] Add a workspace root `Cargo.toml` with resolver 2 and shared workspace dependency versions.
- [x] Add minimal library crates for `agenter-core`, `agenter-protocol`, and `agenter-db`.
- [x] Add minimal binary crates for `agenter-control-plane` and `agenter-runner`.
- [x] Add `cargo fmt --all -- --check`, `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` as active Rust phase verification in `docs/harness/VERIFICATION.md`.
- [x] Run the Rust verification commands and fix failures.
- [x] Commit with `chore: scaffold rust workspace`.

### Task 0.3: Implement Provider Spike Binaries

**Files:**

- Create: `crates/agenter-runner/src/bin/codex_app_server_spike.rs`
- Create: `crates/agenter-runner/src/bin/qwen_acp_spike.rs`
- Modify: `docs/runbooks/codex-app-server-spike.md`
- Modify: `docs/runbooks/qwen-acp-spike.md`

- [x] Add a Codex spike binary that starts `codex app-server` in a supplied workspace path, writes JSON-RPC requests on stdin, reads JSONL responses, logs method names, and exits cleanly.
- [x] Add a Qwen spike binary that starts `qwen --acp` in a supplied workspace path, writes ACP JSON-RPC requests on stdin, reads responses/notifications, logs method names, and exits cleanly.
- [x] Keep provider method names and payloads isolated in the spike binaries until observed behavior is recorded.
- [x] Update each runbook with the command to run the corresponding spike binary and a section for observed output.
- [x] Run `cargo fmt --all -- --check` and `cargo check --workspace`.
- [x] Commit with `test: add provider protocol spike binaries`.

Active execution notes:

- Task 0.3 added runner-only Tokio/serde_json spike binaries and kept provider payloads local to those binaries.
- Live Codex/Qwen provider execution remains manual because it requires installed and authenticated local provider CLIs; runbooks record the exact spike commands and expected output shape.
- Task 1.1 defines shared core IDs as UUID newtypes and normalized app events as adjacently tagged serde JSON for later runner/browser protocol envelopes.
- Task 1.2 defines typed runner and browser WebSocket payload DTOs only; actual WebSocket server/client behavior remains Task 1.4.
- Task 1.3 added SQLx Postgres migration and repository primitives. `DATABASE_URL` was not configured locally, so `agenter-db` SQLx integration tests are marked ignored; run with `DATABASE_URL=postgres://... cargo test -p agenter-db -- --ignored` to execute migrations and repository assertions against a disposable Postgres database.

## Milestone 1: Core Domain, Storage, and Runner Protocol

Exit criterion: the control plane and runner can register a runner, register workspaces, create an app session, send a test input through the runner protocol, and stream normalized fake-provider events over browser WebSocket.

### Task 1.1: Define Core Domain Types

**Files:**

- Modify: `crates/agenter-core/src/lib.rs`
- Create: `crates/agenter-core/src/events.rs`
- Create: `crates/agenter-core/src/ids.rs`
- Create: `crates/agenter-core/src/session.rs`
- Create: `crates/agenter-core/src/workspace.rs`
- Create: `crates/agenter-core/src/approval.rs`

- [x] Define typed IDs for users, runners, workspaces, sessions, approvals, and connector bindings.
- [x] Define `AppEvent`, `SessionStatus`, `AgentProviderId`, `AgentCapabilities`, `ApprovalDecision`, and event payload structs from the spec.
- [x] Add serde serialization tests for representative `AppEvent` variants.
- [x] Run `cargo test -p agenter-core`.
- [x] Commit with `feat: define core domain events`.

### Task 1.2: Define Runner Protocol Messages

**Files:**

- Modify: `crates/agenter-protocol/src/lib.rs`
- Create: `crates/agenter-protocol/src/runner.rs`
- Create: `crates/agenter-protocol/src/browser.rs`

- [x] Define runner handshake, heartbeat, command, response, and event envelopes with request IDs.
- [x] Define browser WebSocket subscribe and event envelopes.
- [x] Add serde round-trip tests for runner hello, agent input command, approval answer command, and agent event.
- [x] Run `cargo test -p agenter-protocol`.
- [x] Commit with `feat: define runner protocol`.

### Task 1.3: Add Database Migrations and Repository Layer

**Files:**

- Create: `migrations/0001_initial.sql`
- Modify: `crates/agenter-db/src/lib.rs`
- Create: `crates/agenter-db/src/models.rs`
- Create: `crates/agenter-db/src/repositories.rs`

- [x] Create the initial Postgres schema for users, auth identities, password credentials, OIDC providers, runners, runner tokens, workspaces, agent sessions, connector accounts, session bindings, pending approvals, event cache, and connector deliveries.
- [x] Implement repository functions for creating a user, registering a runner, upserting a workspace, creating a session, appending an event cache row, and creating/resolving an approval.
- [x] Add SQLx-backed tests that can run against `DATABASE_URL`, and mark them ignored when no database is configured.
- [x] Run `cargo test -p agenter-db`; if no database is configured, record the skipped integration status in the active plan.
- [x] Commit with `feat: add initial postgres schema`.

### Task 1.4: Build Minimal Control Plane and Fake Runner Flow

**Files:**

- Modify: `crates/agenter-control-plane/src/main.rs`
- Create: `crates/agenter-control-plane/src/http.rs`
- Create: `crates/agenter-control-plane/src/state.rs`
- Create: `crates/agenter-control-plane/src/runner_ws.rs`
- Create: `crates/agenter-control-plane/src/browser_ws.rs`
- Modify: `crates/agenter-runner/src/main.rs`

- [x] Add Axum startup with health endpoint `GET /healthz`.
- [x] Add runner WebSocket endpoint with authenticated in-memory token for development.
- [x] Add browser WebSocket endpoint with subscribe-session support.
- [x] Add a fake runner mode that connects, sends `runner_hello`, accepts an agent input command, and emits deterministic normalized events.
- [x] Add an integration smoke test or runbook proving fake runner to browser event flow.
- [x] Run `cargo check --workspace` and `cargo test --workspace`.
- [x] Commit with `feat: wire fake runner event flow`.

Active execution notes:

- Task 1.4 wires an in-memory Axum control plane with `/healthz`, `/api/runner/ws`, and `/api/browser/ws`; the dev runner token defaults to `dev-runner-token`.
- The fake runner mode is enabled with `agenter-runner --fake` or `AGENTER_RUNNER_MODE=fake`, connects to `ws://127.0.0.1:7777/api/runner/ws` by default, and emits deterministic normalized events for smoke session `11111111-1111-1111-1111-111111111111`.
- The automated smoke proof is `cargo test -p agenter-control-plane http::tests::smoke_routes_runner_events_to_subscribed_browser`; manual steps are recorded in `docs/runbooks/fake-runner-browser-smoke.md`.
- Task 1.4 verification passed with `cargo fmt --all -- --check`, `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`; DB integration tests remain ignored without `DATABASE_URL` as recorded in Task 1.3.

## Milestone 2: Auth and Browser MVP

Exit criterion: a user can log in, see workspaces and sessions, open a browser chat, send a prompt through the fake runner or first real adapter, and see streamed events.

### Task 2.1: Add Password Auth and Session Cookies

**Files:**

- Modify: `crates/agenter-control-plane/src/http.rs`
- Create: `crates/agenter-control-plane/src/auth.rs`
- Modify: `crates/agenter-db/src/repositories.rs`

- [x] Implement Argon2id password registration bootstrap for the first local admin.
- [x] Implement password login, logout, and `GET /api/auth/me`.
- [x] Add authorization extraction for protected APIs and WebSockets.
- [x] Add tests for password hash verification and unauthorized API rejection.
- [x] Run `cargo test --workspace`.
- [x] Commit with `feat: add password authentication`.

Active execution notes:

- Task 2.1 adds Argon2id password hashing and optional dev bootstrap credentials through `AGENTER_BOOTSTRAP_ADMIN_EMAIL` plus `AGENTER_BOOTSTRAP_ADMIN_PASSWORD`.
- Browser auth currently uses dev-grade opaque in-memory session cookies stored in control-plane process state; restart invalidates sessions and full DB-backed session persistence remains future work.
- Browser WebSocket now requires the same session cookie extraction as protected browser HTTP APIs. Runner WebSocket continues to use runner token auth.
- Verification passed with `cargo fmt --all -- --check`, `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`; SQLx integration tests remain ignored without `DATABASE_URL`.

### Task 2.2: Scaffold Svelte Browser UI

**Files:**

- Create frontend package files under `web/`
- Modify: `docs/harness/VERIFICATION.md`

- [x] Choose SvelteKit or plain Svelte SPA and record the decision in `docs/decisions/`.
- [x] Scaffold login, workspace list, session list, and chat routes.
- [x] Add API client modules for auth, sessions, and WebSocket events.
- [x] Add frontend verification commands to `docs/harness/VERIFICATION.md`.
- [x] Run frontend checks and build.
- [x] Commit with `feat: scaffold browser ui`.

Active execution notes:

- Task 2.2 chooses a plain Svelte TypeScript SPA with Vite. The Rust control plane remains the owner of auth, REST APIs, and WebSocket APIs; the frontend is static and can be served by the control plane or a reverse proxy.
- Backend workspace/session list and message endpoints are not implemented yet, so the scaffolded routes show coherent pending states on `404`; Task 2.3 will wire the full chat UX.

### Task 2.3: Implement Browser Chat UX

**Files:**

- Modify files under `web/`
- Modify control-plane session APIs as needed

- [x] Implement session list and chat view with streaming text deltas.
- [x] Render command, file, tool, approval, and error cards from normalized events.
- [x] Add approval accept/decline actions.
- [x] Add browser reconnect behavior that reloads event cache and resubscribes.
- [x] Run backend and frontend verification.
- [x] Commit with `feat: add browser chat experience`.

Active execution notes:

- Task 2.3 adds in-memory browser REST APIs for runners, runner workspaces, sessions, session history, session messages, and approval decisions.
- Browser history and WebSocket payloads now carry event IDs so reconnect can reload cached events and avoid duplicate rendering.
- Fake runner events now include representative command, tool, file, approval, error, user, and streaming assistant events for the browser cards.
- Verification passed with `cargo fmt --all -- --check`, `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `npm run check`, `npm run lint`, `npm run test`, and `npm run build`.
- Follow-up review fixes are committed in `fix: harden browser chat event flow`: approval decisions are now delivered to the runner before resolved events are published, duplicate resolved decisions return the cached event without re-sending runner commands, runner connections are not registered until required capabilities/workspaces are present, and empty `202 Accepted` responses no longer break the browser API client.

## Milestone 3: Real Agent Adapters

Exit criterion: browser can create or resume Codex and Qwen sessions in a configured workspace and route at least one approval request.

### Task 3.1: Codex App-Server Adapter

**Files:**

- Create: `crates/agenter-runner/src/agents/codex.rs`
- Modify runner state and command handling modules
- Update: `docs/runbooks/codex-app-server-spike.md`

- [ ] Promote observed Codex spike payloads into a typed adapter.
- [ ] Implement process supervision, initialize, thread create/resume/read, turn start, event normalization, interrupt, and approval answer.
- [ ] Add adapter tests using recorded JSON fixtures where possible.
- [ ] Run Rust verification and a local Codex smoke test when `codex` is installed.
- [ ] Commit with `feat: add codex app-server adapter`.

### Task 3.2: Qwen ACP Adapter

**Files:**

- Create: `crates/agenter-runner/src/agents/qwen_acp.rs`
- Modify runner state and command handling modules
- Update: `docs/runbooks/qwen-acp-spike.md`

- [ ] Promote observed Qwen ACP payloads into a typed adapter.
- [ ] Implement process supervision, initialize, capability detection, session create/resume when supported, prompt, event normalization, interrupt if supported, and permission answer.
- [ ] Add adapter tests using recorded JSON fixtures where possible.
- [ ] Run Rust verification and a local Qwen smoke test when `qwen` is installed.
- [ ] Commit with `feat: add qwen acp adapter`.

## Milestone 4: OIDC and Messenger Linking

Exit criterion: browser supports password and Authentik OIDC login, and a Telegram or Mattermost external identity can be linked through browser auth.

### Task 4.1: Add OIDC Auth

**Files:**

- Modify: `crates/agenter-control-plane/src/auth.rs`
- Modify: `crates/agenter-db/src/repositories.rs`
- Add OIDC config docs under `docs/runbooks/`

- [ ] Implement OIDC provider config loading with Authentik-compatible defaults.
- [ ] Implement login redirect, callback, identity binding, and session creation.
- [ ] Add tests for callback state validation and identity upsert.
- [ ] Commit with `feat: add oidc authentication`.

### Task 4.2: Add Messenger Link Flow

**Files:**

- Modify control-plane auth/link modules
- Modify DB repositories

- [ ] Implement short-lived link code creation.
- [ ] Implement `/link/{code}` browser landing and completion API.
- [ ] Bind Telegram or Mattermost external identity to an authenticated user.
- [ ] Add tests for expired, reused, and unauthorized link codes.
- [ ] Commit with `feat: add connector account linking`.

## Milestone 5: Telegram Connector

Exit criterion: a linked Telegram user can list sessions, bind one, send a prompt, receive compact events, and answer an approval from Telegram.

### Task 5.1: Telegram Transport

**Files:**

- Create connector crate or module under `crates/`
- Add config and runbook

- [ ] Implement polling mode.
- [ ] Implement webhook handler.
- [ ] Parse `/login`, `/sessions`, `/use`, `/new`, `/status`, `/open`, and text messages.
- [ ] Add update parsing tests from JSON fixtures.
- [ ] Commit with `feat: add telegram transport`.

### Task 5.2: Telegram Rendering and Approval Buttons

**Files:**

- Modify Telegram connector files
- Modify renderer modules

- [ ] Render compact session cards and recent visible turns.
- [ ] Render normalized event cards without full transcript spam.
- [ ] Add inline approval buttons and callback handling.
- [ ] Add delivery idempotency storage.
- [ ] Commit with `feat: add telegram session projection`.

## Milestone 6: Mattermost Connector

Exit criterion: a linked Mattermost user can bind a thread to a session, send prompts in the thread, receive compact event cards, and answer approvals.

### Task 6.1: Mattermost Transport

**Files:**

- Create connector crate or module under `crates/`
- Add config and runbook

- [ ] Implement bot REST client.
- [ ] Implement WebSocket event listener.
- [ ] Parse direct messages, thread replies, and command fallback.
- [ ] Add parsing tests from JSON fixtures.
- [ ] Commit with `feat: add mattermost transport`.

### Task 6.2: Mattermost Thread Projection

**Files:**

- Modify Mattermost connector files
- Modify renderer modules

- [ ] Map root posts and threads to session bindings.
- [ ] Render compact session cards and browser links.
- [ ] Add interactive approval actions with command fallback.
- [ ] Add delivery idempotency storage.
- [ ] Commit with `feat: add mattermost session projection`.

## Milestone 7: Hardening and Deployment

Exit criterion: the system can be run locally through documented commands and has basic reconnect, retry, logging, and pruning behavior.

### Task 7.1: Runner Reconnect and Session Recovery

- [ ] Add heartbeat timeouts and runner status transitions.
- [ ] Reconcile sessions on runner reconnect.
- [ ] Mark unavailable sessions degraded instead of losing registry state.
- [ ] Add tests for reconnect and degraded status.
- [ ] Commit with `feat: harden runner reconnect`.

### Task 7.2: Connector Delivery Retries

- [ ] Add retry state and idempotency keys for connector deliveries.
- [ ] Add backoff for transient Telegram and Mattermost failures.
- [ ] Add tests for duplicate event delivery suppression.
- [ ] Commit with `feat: add connector delivery retries`.

### Task 7.3: Deployment Runbook

- [ ] Add Docker Compose for Postgres, control plane, and optional runner.
- [ ] Add reverse proxy notes for public HTTPS and LAN-only deployment.
- [ ] Add secret configuration examples using environment references.
- [ ] Run end-to-end local smoke verification.
- [ ] Commit with `docs: add deployment runbook`.

## Verification Policy

Run the strongest applicable verification after each task:

- documentation-only tasks: `find . -maxdepth 4 -type f | sort` plus placeholder/consistency review;
- Rust tasks: `cargo fmt --all -- --check`, `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`;
- frontend tasks: the package manager check, lint, test, and build commands recorded in `docs/harness/VERIFICATION.md`;
- connector tasks: fixture tests plus a documented manual smoke test when real credentials are required.

When a verification command cannot run, record the command, failure summary, whether it is a product failure or environment limitation, and the next step in this plan's Current State section.

## Active Execution Notes

- Start with Milestone 0.
- Do not scaffold application code before this plan is approved.
- Prefer local commits at each task boundary.
