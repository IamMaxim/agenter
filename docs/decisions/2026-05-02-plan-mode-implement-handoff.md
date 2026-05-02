# Plan Mode "Implement plan" Handoff

Status: accepted

## Context

Codex's Plan mode is a developer-prompt-driven collaboration mode. The model
emits a `<proposed_plan>` block, and Codex's TUI offers an "Implement this
plan?" popup with three options:

1. Switch to Default and continue in the same thread.
2. Clear context and continue in a fresh thread seeded with the plan.
3. Stay in Plan mode (dismiss).

Agenter's browser previously had no equivalent: switching collaboration mode
was a separate `PATCH /settings` call that raced with `POST /messages`. The
composer's mode dropdown also had a hardcoded `<option value="">default</option>`
that, when picked, persisted `collaboration_mode: null`. The control plane
forwarded that to the runner, the runner omitted `collaborationMode` from the
Codex `turn/start` payload, and Codex retained the thread's prior Plan mode.
The model dutifully re-planned instead of implementing.

## Decision

1. **Atomic per-turn settings override.** `POST /sessions/:id/messages`
   accepts an optional `settings_override: AgentTurnSettings`. When present,
   the control plane persists the override as the session's sticky settings
   *before* forwarding the runner command, so the new collaboration mode is
   applied to the very turn the message starts. This mirrors Codex TUI's
   `SubmitUserMessageWithMode` event.

2. **Initial-message session creation.** `POST /sessions` accepts an optional
   `initial_message` plus `settings_override`. When both are present the
   control plane registers the session, applies the override, and dispatches
   the seed message in a single HTTP round-trip. This is what powers the
   "Implement in fresh thread" handoff: the new session starts with the prior
   plan content already in the model's context and Default mode active.

3. **Composer dropdown sourced from runner options.** The hardcoded
   `<option value="">default</option>` is gone. The dropdown renders only the
   `collaboration_modes` advertised by the runner. Selecting the empty value
   (or any blank value) resolves to the runner-provided "default" id rather
   than persisting `null`.

4. **PlanCard handoff row.** The most recent completed plan in Plan mode
   shows three buttons that mirror Codex TUI's selection items:
   - "Implement plan" → `sendSessionMessage(..., { settings_override: { collaboration_mode: defaultModeId, ... } })`.
   - "Implement in fresh thread" → `createSession({ initial_message: PLAN_IMPLEMENTATION_CLEAR_CONTEXT_PREFIX + "\n\n" + plan, settings_override: { collaboration_mode: defaultModeId } })`, then route to the new session.
   - "Stay in Plan mode" → local dismissal.

   The clear-context preamble is copied verbatim from
   `tmp/codex/codex-rs/tui/src/chatwidget/plan_implementation.rs` so the model
   interprets the fresh-thread implementation request the same way.

## Consequences

- Switching modes via the composer is now a single round-trip whether the
  user toggles the dropdown or clicks "Implement plan". The control plane is
  the single source of truth for sticky session settings.
- The runner contract is preserved: it only emits `collaborationMode` when
  the control plane sends one, and Codex's app-server normalizer fills in the
  preset `developer_instructions`. We do not duplicate Plan mode's prompt
  text.
- A non-Codex provider that does not advertise a `default` collaboration mode
  will see the handoff buttons disabled with "Default mode unavailable",
  matching Codex TUI's `PLAN_IMPLEMENTATION_DEFAULT_UNAVAILABLE` guard.

## Alternatives Considered

- Strip `<proposed_plan>` from the conversation history before the implement
  turn. Rejected: Codex's Default mode interprets a prior plan plus the
  explicit "Implement the plan." message as a request to implement, and the
  "Implement in fresh thread" path already covers the case where context
  pressure is the problem.
- Keep the dropdown's empty sentinel and "fix" it by translating `null` to
  the default mode id in the runner. Rejected: the runner's contract that
  "no mode means do not change Codex's mode" is correct and matches Codex's
  own `turn/start` semantics; the bug was the browser silently storing
  `null` for a user-selected mode.
