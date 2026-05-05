# Universal-Only UAP/2 Architecture Reset

Status: implemented
Date: 2026-05-05

## Goal

Make `uap/2` the primary contract from provider runtime to browser before
reintroducing any Codex adapter work.

## Phases

1. Runner reset
   - Convert ACP reducers to produce `AdapterEvent` / `UniversalEventKind`
     directly.
   - Convert fake runner deterministic smoke events to direct universal events.
   - Delete runner-local normalized projection helpers and Codex-shaped adapter
     tests.

2. Control-plane reset
   - Keep `publish_universal_event` and runner `AgentUniversalEvent` ingestion
     as canonical event paths.
   - Store approval/question registries from universal envelopes.
   - Import discovered native history as universal discovery events with
     session IDs filled after registration.

3. Browser reset
   - Keep WebSocket frames universal-only.
   - Remove Codex-specific presentation branches and fixtures.
   - Drive transcript rows from universal item/tool/approval/plan fields.

4. Documentation and verification
   - Record the no-compatibility-window ADR.
   - Update smoke runbooks and harness memory.
   - Run focused protocol, runner, control-plane, and frontend tests, then the
     full gate when feasible.

## Current Implementation Notes

- ACP and fake-runner runtime event emission are universal-first.
- The runner adapter layer has no normalized projection helper.
- Control-plane runtime ingestion, cache, browser replay, history reads,
  approval/question state, and discovered-history import are universal-only.
- Slash-command user echoes are `item.created` user items, slash execution
  results are `provider.notification` events with category `slash_command`, and
  provider commands come from runner/provider manifests.
- Runtime crates and browser source have no `NormalizedEvent`,
  `CachedEventEnvelope`, `legacy_normalized_projection_universal_event`, or
  `AgentProviderId::CODEX` references.

## Verification

Focused:

```sh
cargo test -p agenter-protocol --test browser_json_frame_conformance
cargo test -p agenter-protocol browser
cargo test -p agenter-protocol runner
cargo test -p agenter-runner acp
cargo test -p agenter-runner fake
cargo test -p agenter-control-plane universal
cargo test -p agenter-control-plane slash
cargo test -p agenter-control-plane approval
cargo test -p agenter-control-plane subscribe_snapshot
cargo test -p agenter-control-plane runner_event
cd web && npm run test -- normalizers sessionSnapshot universalEvents events sessions slashCommands
```

Full gate:

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
