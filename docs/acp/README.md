# ACP Runner Support Notebook

Status: active research and implementation notebook
Date: 2026-05-02

This directory tracks the working evidence for Agenter's ACP runner support. It is intentionally a notebook, not the durable architecture home. Stable outcomes should be promoted to `docs/specs/`, `docs/plans/`, `docs/decisions/`, and `docs/runbooks/`.

## Targets

- Qwen Code through `qwen --acp`.
- Gemini CLI through `gemini --acp`.
- OpenCode through `opencode acp`.

## Current Direction

- Use one generic ACP runtime in the runner.
- Keep provider-specific command lines and quirks in small profiles.
- Let one runner process advertise all locally available configured ACP providers for a workspace.
- Treat provider auth as local setup; Agenter reports clear degraded errors instead of owning provider OAuth.
- Serve ACP file-system and terminal client methods from the runner within the configured workspace boundary.

## Files

- `progress.md`: running notes, evidence, and open questions.
- `provider-matrix.md`: local provider versions and capability status.
- `spikes/qwen.md`: Qwen ACP spike record.
- `spikes/gemini.md`: Gemini ACP spike record.
- `spikes/opencode.md`: OpenCode ACP spike record.

