# Persistent approval policy rules

Status: accepted

Date: 2026-05-04

## Context

Codex TUI can reduce repeated prompts by persisting broader approval rules, such
as command prefix allowances. Agenter previously modeled `approve always` as a
native/session decision only. New browser sessions and new agent sessions still
prompted for equivalent actions.

## Decision

Agenter stores durable approval policy rules in Postgres. Rules are scoped to
the approving user, workspace, and provider. The control plane is the source of
truth for these rules and evaluates them before presenting repeat approvals.

Approval requests may include conservative rule-preview options derived from
provider-native payloads or stable normalized request data. Choosing such an
option stores the rule and then resolves the current native approval through the
existing runner acknowledgement path. Matching future rules auto-resolve native
approvals through the same runner path.

Rules are disabled rather than deleted so revocation leaves an audit trail.

## Consequences

- Approval policy becomes durable control-plane state, not browser state.
- Provider-native session approval behavior remains supported, but it is not the
  persistence source for agenter.
- Rule matching is intentionally conservative. If agenter cannot derive a stable
  matcher, the request keeps the normal one-time/session options only.
- The browser needs a rule-management surface so users can revoke broad grants.
