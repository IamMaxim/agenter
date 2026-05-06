# Codex TUI-Style Approvals Implementation Plan

Status: implemented with deferred connector/live-preapproval follow-ups
Date: 2026-05-06

## Goal

Implement an Agenter approval system for Codex that behaves like Codex TUI and
app-server approvals, but deliberately removes Agenter-owned sandboxing as a
product feature. The system should support interactive approvals, session and
workspace remembers, durable reconnect/replay, and explicit dangerous modes
that allow all operations when the runner is isolated in a VM.

## Source Research Summary

Reference inputs:

- `docs/codex-approvals/README.md`
- `docs/codex-approvals/policy-and-requirements.md`
- `docs/codex-approvals/orchestrator-and-session.md`
- `docs/codex-approvals/guardian-and-hooks.md`
- `docs/codex-approvals/tui-ux.md`
- `docs/codex-approvals/other-surfaces.md`
- `tmp/codex/codex-rs/protocol/src/protocol.rs`
- `tmp/codex/codex-rs/protocol/src/approvals.rs`
- `tmp/codex/codex-rs/core/src/tools/sandboxing.rs`
- `tmp/codex/codex-rs/core/src/session/mod.rs`
- `tmp/codex/codex-rs/tui/src/bottom_pane/approval_overlay.rs`
- `tmp/codex/codex-rs/tui/src/bottom_pane/mod.rs`
- `tmp/codex/codex-rs/tui/src/chatwidget.rs`
- `tmp/codex/codex-rs/utils/approval-presets/src/lib.rs`
- current Agenter approval code under `crates/agenter-core`,
  `crates/agenter-control-plane`, `crates/agenter-runner/src/agents/codex`,
  and `web/src`.

Codex TUI has four relevant properties to mimic:

1. **Policy and permission preset are separate but paired.** Codex has
   `AskForApproval::{untrusted,on-failure,on-request,granular,never}` and
   permission profiles such as read-only, workspace-write, and full access.
   For Agenter, keep approval policy first-class but do not invent an Agenter
   sandbox. Codex may still receive its native `sandboxMode` or permission
   profile when launching/resuming a thread, because that is part of Codex's
   own contract.
2. **Native approvals are blocking obligations.** Codex registers pending
   approval callbacks before emitting events; dropped callbacks resolve as
   abort. Agenter must keep the runner-side native request registered until the
   app-server response has definitely been written or the turn is cancelled.
3. **The UI renders only available decisions.** Codex sends or derives
   `available_decisions`; the TUI converts those to overlay options and maps
   user selection back to native review decisions. Agenter should use native
   available decisions as the source of truth, then add Agenter remember options
   only when they have a real matcher and are safe to persist.
4. **Approvals are queued presentation, not lost events.** Codex defers
   interrupt-class prompts while streaming and delays the approval modal by one
   second after composer typing. In Agenter, browser rows can appear
   immediately, but modal/toast focus should be non-stealing and FIFO.

## Non-Goals

- Do not add an Agenter sandbox layer.
- Do not emulate Codex Guardian/auto-review in the first implementation.
  Preserve the schema space for automated decisions, but route all first-pass
  decisions through Agenter policy, persistent rules, explicit allow-all modes,
  or the user.
- Do not hide Codex native payloads from the browser during adapter research.
- Do not make provider-specific browser branches for normal approval rendering.
- Do not make broad product UI changes beyond the approval settings, approval
  cards, rule list, and status affordances needed for this plan.

## Target Architecture

```text
Browser / connectors
  -> control-plane command: resolve approval
  -> control-plane approval policy engine and idempotency
  -> runner AnswerApproval command
  -> Codex app-server server-request response

Codex app-server server request
  -> runner Codex obligation mapper
  -> optional runner preapproval from control-plane supplied policy snapshot
  -> uap/2 approval.requested or immediate native response + approval.resolved
  -> control-plane approval registry, event log, replay, connectors
  -> browser approval card / non-stealing prompt
```

The control plane owns user authorization, durable rules, browser replay, and
idempotent resolution state. The runner owns Codex process state, native request
IDs, native request payloads, and exactly-once response delivery to Codex.

Implementation note: this pass implements control-plane auto-resolution for
approval modes and persistent rules. Runner-side native preapproval snapshots
remain a deferred optimization; the runner still emits `approval.requested`
first, keeps the Codex native request pending, and receives an immediate
control-plane `AnswerApproval` when policy auto-allows.

## Approval Modes

Add an explicit Agenter approval mode for each session/runner workspace. These
are not sandbox modes.

| Mode | Codex launch mapping | Agenter behavior |
| --- | --- | --- |
| `ask` | `approvalPolicy = on-request`, Codex permission profile `workspace-write` by default | Ask for command, file change, and permission approvals unless a persistent rule matches. |
| `read_only_ask` | `approvalPolicy = on-request`, Codex permission profile `read-only` | Read-only default; ask before writes/network/extra permissions. |
| `trusted_workspace` | `approvalPolicy = on-request` or granular with approvals enabled, Codex permission profile `workspace-write` | Add workspace-provider remember options prominently; still ask for unmatched high-risk operations. |
| `allow_all_session` | `approvalPolicy = never`, Codex permission profile `full-access`/`danger-full-access` | Auto-approve every approval for the current Agenter session only. Show persistent dangerous status in browser. |
| `allow_all_workspace` | `approvalPolicy = never`, Codex permission profile `full-access`/`danger-full-access` | Persist a workspace-provider allow-all rule; auto-approve all future Codex approvals for that workspace until revoked. |

The UI labels must say **Allow all operations** and **danger-full-access** where
applicable. This should be intentionally scary but supported, because the runner
may be isolated in a VM.

## Native Decision Mapping

Agenter canonical decisions:

- `ApprovalDecision::Accept` maps to Codex app-server `accept` and legacy
  `approved`.
- `ApprovalDecision::AcceptForSession` maps to Codex app-server
  `acceptForSession` and legacy `approved_for_session`.
- `ApprovalDecision::Decline` maps to `decline` and legacy `denied`.
- `ApprovalDecision::Cancel` maps to `cancel` and legacy `abort`.
- `ApprovalDecision::ProviderSpecific` is allowed only when the native
  available decision cannot be faithfully represented by the canonical enum.

Command approvals must use Codex's effective approval ID rule:
`approval_id.unwrap_or(call_id)` for legacy TUI protocol, and
`approvalId.unwrap_or(itemId).unwrap_or(request_id)` for app-server obligation
mapping. Store both the universal approval ID and native request ID.

## Persistent Rules

Persisted rules live in the control-plane database and are scoped by owner user,
workspace, provider, and approval kind.

Required matchers:

- `allow_all`: matches all approvals for the workspace/provider/kind or all
  kinds when `kind = provider_specific` plus matcher says all kinds.
- `command_prefix`: tokenized command prefix, rejecting shell metacharacter
  previews.
- `command_exact`: canonical command vector/string plus cwd.
- `file_change_root`: workspace-root or specific grant root.
- `native_method`: Codex server-request method such as
  `item/permissions/requestApproval`.
- `permission_profile`: requested Codex permission JSON normalized enough to
  compare stable profiles.

Rules should store the decision payload, source approval ID when created from a
prompt, label, disabled timestamp, and updated timestamp. Existing
`approval_policy_rules` can be extended rather than replaced.

## Phase 0: Audit And Fixture Baseline

**Files:**

- Modify: `docs/plans/2026-05-05-codex-app-server-uap2.md`
- Modify: `docs/codex-approvals/*.md` only if source research gaps are found
- Create: `crates/agenter-runner/tests/fixtures/codex_approvals_trace.json`
- Create: `crates/agenter-runner/tests/codex_approval_parity.rs`

**Steps:**

- [x] Confirm the current Codex app-server schema still contains
  `AskForApproval`, `ReviewDecision`, `available_decisions`,
  `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`,
  and `item/permissions/requestApproval`.
- [x] Add a sanitized fixture covering command approval with
  `availableDecisions`, command approval without `approvalId`, file-change
  approval, permission approval, legacy `ExecCommandApproval`, legacy
  `ApplyPatchApproval`, and an unknown server request.
- [x] Add fixture tests that fail on missing mapping, wrong native request ID,
  missing raw payload, or wrong native response payload.
- [x] Update the active Codex app-server plan with a note that this plan owns
  approval policy/rule/UI behavior.

**Verification:**

```sh
cargo test -p agenter-runner codex_approval
git diff --check docs/plans/2026-05-06-codex-tui-style-approvals.md docs/plans/2026-05-05-codex-app-server-uap2.md
```

**Exit Criteria:**

- Fixture tests describe every approval path that later phases depend on.
- The existing Codex adapter plan points to this plan for approval behavior.

## Phase 1: Core Approval Mode Model

**Files:**

- Modify: `crates/agenter-core/src/approval.rs`
- Modify: `crates/agenter-core/src/events.rs`
- Modify: `crates/agenter-core/src/session.rs` or the current session settings
  module if renamed
- Modify: `crates/agenter-protocol/src/runner.rs`
- Test: `crates/agenter-core/src/approval.rs`
- Test: `crates/agenter-protocol/tests/browser_json_frame_conformance.rs`

**Steps:**

- [x] Add `ApprovalMode` with `ask`, `read_only_ask`, `trusted_workspace`,
  `allow_all_session`, and `allow_all_workspace`.
- [x] Add `ApprovalMode::is_dangerous_allow_all()` and
  `ApprovalMode::default_for_provider(provider_id)` helpers.
- [x] Add provider-details equivalent in the Codex turn/start request mapper:
  `approvalPolicy`, `sandboxPolicy`, and `permissions` are derived from
  `ApprovalMode` unless raw provider `extra` already supplies them.
- [x] Add tests proving mode JSON names are stable and allow-all modes are
  flagged dangerous.

**Verification:**

```sh
cargo test -p agenter-core approval_mode
cargo test -p agenter-protocol --test browser_json_frame_conformance
```

**Exit Criteria:**

- Approval mode is a universal/session setting, while Codex launch details stay
  provider metadata.
- No code claims Agenter is sandboxing the runner.

## Phase 2: Policy Engine And Rule Matching

**Files:**

- Modify: `crates/agenter-control-plane/src/policy.rs`
- Modify: `crates/agenter-db/src/models.rs`
- Modify: `crates/agenter-db/src/repositories.rs`
- Test: `crates/agenter-control-plane/src/policy.rs`
- Test: `crates/agenter-db/src/repositories.rs`

**Steps:**

- [x] Extend policy input with `provider_id`, `workspace_id`, `approval_mode`,
  native method, command vector/string, cwd, grant root, permission JSON, and
  raw request payload hash.
- [x] Implement `allow_all` matcher and decision.
- [x] Implement `command_exact`, improved `command_prefix`,
  `file_change_root`, `native_method`, and `permission_profile` matchers.
- [x] Make `allow_all_session` return `Allow(Accept)` without a DB rule.
- [x] Make `allow_all_workspace` create or reuse a disabled-at-null allow-all
  DB rule and then return `Allow(AcceptForSession)`.
- [x] Keep `ask` and `read_only_ask` defaulting to `Ask`, with high-risk labels
  for network-like commands and broad permission profiles.
- [x] Keep policy decisions pure and testable: no runner calls inside policy
  evaluation.

**Verification:**

```sh
cargo test -p agenter-control-plane policy
cargo test -p agenter-db approval_policy_rule
```

**Exit Criteria:**

- Matching rules can auto-approve without browser involvement.
- Dangerous allow-all modes are explicit in stored data and labels.

## Phase 3: Runner Preapproval And Native Delivery

**Files:**

- Modify: `crates/agenter-protocol/src/runner.rs`
- Modify: `crates/agenter-runner/src/agents/adapter.rs`
- Modify: `crates/agenter-runner/src/agents/codex/obligations.rs`
- Modify: `crates/agenter-runner/src/agents/codex/runtime.rs`
- Test: `crates/agenter-runner/src/agents/codex/obligations.rs`
- Test: `crates/agenter-runner/src/agents/codex/runtime.rs`

**Steps:**

- [ ] Add runner session policy snapshot data: approval mode, active rules, and
  a generation number supplied by the control plane.
- [ ] In the Codex server-request path, evaluate preapproval before emitting an
  unresolved `approval.requested`.
- [ ] If preapproved, emit `approval.requested` with policy metadata and
  `status = resolving`, immediately write the native app-server response, then
  emit `approval.resolved`.
- [x] If not preapproved, emit `approval.requested` and keep the native request
  registered until response delivery succeeds, conflicts, or the turn is
  cancelled.
- [x] Preserve idempotent duplicate answer behavior: same decision joins or
  replays; conflicting decision returns conflict.
- [ ] Ensure `Cancel` for a native approval also interrupts/cancels the native
  turn when Codex semantics require that, matching Codex `Abort`.
- [x] Do not add or rely on an Agenter sandbox. Codex `sandboxMode` is only a
  pass-through Codex runtime option.

Deferred: policy preapproval remains in the control plane for this pass. This
preserves exactly-once native response delivery while avoiding a stale runner
policy snapshot protocol. A future runner snapshot can reduce browser-visible
pending churn, but it must be generationed and invalidated on rule changes.

**Verification:**

```sh
cargo test -p agenter-runner codex_approval
cargo test -p agenter-runner codex_runtime
```

**Exit Criteria:**

- Auto-approval and user approval both end with exactly one native response.
- Runner reconnect and duplicate answer behavior still satisfy existing
  approval idempotency ADRs.

## Phase 4: Control-Plane Approval Lifecycle

**Files:**

- Modify: `crates/agenter-control-plane/src/state.rs`
- Modify: `crates/agenter-control-plane/src/api/approvals.rs`
- Modify: `crates/agenter-control-plane/src/api/approval_rules.rs`
- Modify: `crates/agenter-control-plane/src/api/sessions.rs`
- Test: `crates/agenter-control-plane/src/state.rs`
- Test: `crates/agenter-control-plane/src/api/approvals.rs`
- Test: `crates/agenter-control-plane/src/api/approval_rules.rs`

**Steps:**

- [x] Apply policy/rule matching when an `approval.requested` event arrives.
- [x] When control-plane policy auto-allows, transition to resolving and send
  `AnswerApproval` without waiting for browser action.
- [x] Return pending, resolving, and auto-resolved approval envelopes from
  `GET /api/approvals` so reloads reconstruct truthful state.
- [x] Add create/update endpoints for approval mode and allow-all workspace
  rules; keep revoke/disable endpoint.
- [x] Make `persist_rule:*` options persist only the preview attached to the
  approval request, never ad hoc browser-provided matcher JSON.
- [x] Add conflict responses for attempts to change a resolving or resolved
  decision.
- [x] Emit visible provider/control-plane notifications when auto-approval
  happens because of `allow_all_session` or `allow_all_workspace`.

**Verification:**

```sh
cargo test -p agenter-control-plane approval
cargo test -p agenter-control-plane universal
cargo test -p agenter-control-plane subscribe_snapshot
```

**Exit Criteria:**

- Browser reload and runner reconnect preserve every approval state.
- Workspace allow-all can be listed and revoked.

## Phase 5: Browser Approval UX

**Files:**

- Modify: `web/src/api/types.ts`
- Modify: `web/src/api/approvalRules.ts`
- Modify: `web/src/api/sessions.ts`
- Modify: `web/src/lib/chatEvents.ts`
- Modify: `web/src/routes/ChatRoute.svelte`
- Modify: `web/src/styles.css`
- Test: `web/src/lib/chatEvents.test.ts`
- Test: `web/src/api/approvalRules.test.ts`
- Test: `web/src/api/sessions.test.ts`

**Steps:**

- [x] Render only approval options present on the request.
- [x] Render Codex native option labels from `native_option_id` when they are
  more specific than the generic label.
- [x] Add an approval-mode control in session settings or the chat header:
  `Ask`, `Read-only ask`, `Trusted workspace`, `Allow all this session`,
  `Allow all for workspace`.
- [x] For allow-all choices, require a deliberate confirmation dialog that says
  the runner should be isolated in a VM and that Agenter will not sandbox it.
- [x] Show a persistent danger status pill when allow-all is active.
- [x] Keep approval cards in the transcript immediately, but make modal/toast
  presentation FIFO and non-stealing when the composer has changed in the last
  second.
- [x] Display resolving decisions as disabled in-flight buttons.
- [x] Keep the approval rules list flat and revocable.

**Verification:**

```sh
cd web
npm run test -- chatEvents approvalRules sessions
npm run check
npm run lint
```

Manual browser smoke:

- Trigger a command approval, type in the composer while it arrives, and confirm
  the row appears without stealing focus.
- Approve once and confirm the button enters resolving state.
- Approve with a persist option and confirm the rule appears in the rule list.
- Enable allow-all session and confirm subsequent approvals resolve
  automatically with a visible danger indicator.
- Revoke a workspace rule and confirm the next matching operation asks again.

**Exit Criteria:**

- Browser behavior feels like Codex TUI's approval overlay semantics while
  fitting a persistent web transcript.
- Dangerous modes are supported without being visually subtle.

## Phase 6: Connector Projections

**Files:**

- Modify: Telegram connector module when present
- Modify: Mattermost connector module when present
- Modify: shared connector approval rendering helpers if present
- Test: connector approval tests or add focused tests next to connector modules

**Steps:**

- [ ] Project the same approval options into Telegram inline buttons when
  possible.
- [ ] Project the same approval options into Mattermost interactive actions
  when possible.
- [ ] For allow-all workspace/session mode changes, require browser
  confirmation rather than accepting a one-tap messenger action.
- [ ] Ensure connector retries use the same idempotent approval decision API as
  browser decisions.

**Verification:**

```sh
cargo test --workspace approval
```

Manual smoke:

- Telegram approval ask/approve/deny.
- Mattermost approval ask/approve/deny.
- Browser reload after messenger decision.

**Exit Criteria:**

- Connectors remain projections; they do not own approval business logic.

## Phase 7: Documentation And Decisions

**Files:**

- Create: `docs/decisions/2026-05-06-codex-approval-modes-without-agenter-sandbox.md`
- Modify: `docs/harness/PROJECT_CONTEXT.md`
- Modify: `docs/harness/VERIFICATION.md`
- Modify: `docs/runbooks/universal-protocol-smoke.md`
- Modify: this plan with progress notes

**Steps:**

- [x] Record the ADR: Agenter does not sandbox agents; Codex native
  sandbox/permission fields are provider pass-through; allow-all is supported
  for VM-isolated runners.
- [x] Update project context to mention explicit approval modes and dangerous
  allow-all support.
- [x] Add approval-mode smoke checks to the universal protocol runbook.
- [x] Add unresolved questions or implementation deviations to this plan.

**Verification:**

```sh
find . -maxdepth 4 -type f | sort
git diff --check docs/decisions docs/harness docs/runbooks docs/plans
```

**Exit Criteria:**

- Future agents can understand why there is no Agenter sandbox and why
  allow-all exists.

## Full Verification Gate

Run when all implementation phases are complete:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
cd web
npm run check
npm run lint
npm run test
npm run build
git diff --check
```

Also run the universal protocol smoke from
`docs/runbooks/universal-protocol-smoke.md` with a live Codex app-server session
when local Codex authentication is available.

## Open Questions

- Should `allow_all_workspace` apply to every approval kind by default, or
  should the UI default to command/file/permission all enabled with per-kind
  toggles? Decision: apply to every approval kind by default.
- Should Agenter expose Codex `on-failure` at all, given the project goal of no
  Agenter sandbox? Decision: no; hide it behind raw provider config only.
- Should persisted command rules use exact parsed command vectors from Codex
  `parsed_cmd` when available instead of tokenizing display strings?
  Decision: yes.
- Should allow-all be runner-wide instead of workspace-provider scoped?
  Decision: no for first pass; make VM-level trust visible through
  workspace/provider settings first.

## Implementation Deviations And Follow-Ups

- Runner policy snapshots and native preapproval were not implemented in this
  pass. Control-plane auto-resolution now handles `allow_all_session`,
  `allow_all_workspace`, and persisted rules immediately after
  `approval.requested`. This keeps user-visible replay truthful and avoids a
  stale runner rule cache. Future work can add a generationed runner snapshot.
- Connector projections remain deferred. Browser is the canonical full-fidelity
  approval surface for dangerous mode changes.
- No SQL migration was required because existing `approval_policy_rules`
  already stores matcher JSON, decisions, disabled timestamps, labels, source
  approval IDs, and user scoping.
