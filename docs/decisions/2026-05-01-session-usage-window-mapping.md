# Session Usage Window Mapping

Status: accepted

## Context

Codex emits `account/rateLimits/updated` provider events with `primary` and `secondary` windows. The browser needs stable labels for composer usage metrics.

## Decision

Map provider `primary` to the 5h usage window and provider `secondary` to the weekly usage window.

The control plane stores only the latest normalized `SessionUsageSnapshot` on the app session record. Provider-native payloads remain in event cache for debugging.

## Consequences

- The browser can render stable `5h` and `w` metrics without parsing provider payloads.
- A newly loaded session can show the last-known limits before another provider update arrives.
- If a provider changes window semantics, the control-plane parser is the only mapping layer to update.

## Alternatives Considered

- Parse provider payloads in the browser: rejected because it leaks provider-specific protocol shape into UI code.
- Keep usage as event-only state: rejected because first paint would often show empty metrics until a new event arrived.
