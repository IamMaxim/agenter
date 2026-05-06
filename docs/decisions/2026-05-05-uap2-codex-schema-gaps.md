# UAP/2 Codex Schema Gaps

Status: accepted

## Context

The Codex app-server protocol exposes first-class concepts that were not yet
represented in Agenter's `uap/2` schema: thread forks, permission approvals,
blocking native server requests, schema-shaped input forms, richer tool item
categories, and raw native frames. Without schema support, the adapter and
browser would need hidden Codex-specific branches or would drop useful native
debug data during early adapter work.

The current Codex implementation plan also defines an early raw payload browser
contract: every decoded, partially decoded, or undecoded Codex app-server frame
observed by the adapter should be reachable from the browser while the mapping
is under active research.

## Decision

Add the following additive `uap/2` schema fields and variants:

- `AgentProviderId::CODEX = "codex"` as a stable provider constant while
  preserving the open-string provider ID model.
- `UniversalCommand::ForkSession` for native thread fork semantics.
- `ApprovalKind::Permission` for permission-profile approvals that are neither
  shell commands nor file changes.
- `QuestionState.native_request_id` and `QuestionState.native_blocking` for
  durable native server-request correlation.
- `AgentQuestionField.schema` for field-level native or schema-shaped form
  metadata, including MCP elicitation fields.
- `ToolProjection.subkind` for richer tool rows such as `file_change`,
  `web_search`, `image_view`, `image_generation`, `review_mode`, `hook`,
  `context_compaction`, and `one_off_command`.
- `NativeRef.raw_payload` for the original native JSON frame or payload.

Use existing `provider_details` capability metadata for Codex-only or
experimental surfaces such as `thread_fork`, `turn_steer`, `thread_rollback`,
`thread_compaction`, `thread_goal`, `memory_mode`, `review`, `realtime`,
`client_fs`, `one_off_command`, `skills_plugins`, `config_account`, and
`dynamic_tools`.

## Consequences

All changes are wire-compatible for existing providers: new fields are optional
or defaulted for deserialization and skipped when empty during serialization.
Existing provider IDs remain open strings.

The browser can parse and preserve raw native payloads without checking
`provider_id == "codex"`. Normal rendering can continue to use universal fields,
while debug rendering can inspect `native.raw_payload`.

Rust struct literals in adjacent code must explicitly set the new optional
fields to `None` or `false`, but this does not change runtime behavior.

Future productization may redact, size-limit, hash, or replace raw payloads with
local pointers. That is intentionally deferred until the Codex mapping is stable.

## Alternatives Considered

Treat Codex-only features as opaque provider commands only. This was rejected
for thread fork and permission approvals because they affect universal session
lifecycle and security presentation.

Add many Codex-specific tool enum variants. This was rejected for Stage 1 in
favor of `ToolProjection.subkind`, which preserves truthful row categories
without overfitting the shared enum.

Store raw native payloads only in runner-local logs. This was rejected for the
early adapter phase because browser-visible raw payloads make reducer and replay
bugs much faster to diagnose.
