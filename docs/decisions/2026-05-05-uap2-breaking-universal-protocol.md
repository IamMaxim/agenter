# UAP/2 Breaking Universal Protocol

Status: accepted
Date: 2026-05-05

## Context

The `uap/1` rollout left universal semantics split across provider adapters,
generic `NormalizedEvent` projection, and control-plane compatibility/import
projection. That made native runtime behavior fragile: a live event, WAL
replay, browser snapshot, or imported history item could follow different
projection rules.

The browser snapshot wire shape also overloaded `latest_seq` and `has_more`.
Clients could not tell whether a value described the materialized snapshot, the
replay page, or a live-stream boundary.

## Decision

Agenter will use `uap/2` as a breaking universal protocol version. `uap/2`
replaces `uap/1`; there is no dual-stack compatibility window in this
development checkout.

Browser `session_snapshot` frames expose explicit replay cursors:

- `snapshot_seq`: the cursor used by the materialized snapshot.
- `replay_from_seq`: the first replay event returned in the frame, if any.
- `replay_through_seq`: the last replay event returned in the frame, if any.
- `replay_complete`: whether the replay page reached the requested/snapshot
  boundary and live events may be consumed normally.

Provider runtimes emit `AgentUniversalEvent` directly. There is no
`NormalizedEvent` compatibility window in this development checkout, including
for fake-runner smoke events. Discovered native history must be imported as
universal discovery events whose `session_id` is filled after registration.

## Consequences

Frontend clients must reject or convert to `uap/2`; old `uap/1` frames are
not part of the active public contract.

Control-plane replay no longer uses `latest_seq` / `has_more` on
`BrowserSessionSnapshot`. Incomplete replay is represented by
`replay_complete: false` and explicit cursor fields.

Runner-side provider-specific native parsing moves out of the control plane.
The control plane stores and materializes universal events, but it does not own
provider-native parsing for runtime events.
