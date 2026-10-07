# Onboarding release notes

- Linux x86_64 and arm64 static musl release artifacts with SHA-256 checksums.
- `aic setup`, consented `aic agent test` and read-only `aic doctor --json`.
- Contextual readiness and recovery before chat sends, without automatic paid tests.
- Explicit chat default: empty no longer inherits the workflow executor; no fallback
  to Claude or the first configured agent. Existing conversations retain sessions
  and participants. New chats ask for selection when a default is missing/unavailable.
- `aic` opens a browser; use `aic chat --no-open` for terminal/SSH environments.
- Persistent Git context and branch-change warnings with history/session preservation.
- Consultative Environment panel with recovery commands and bounded, scrollable
  dialogs. Chat creation uses only `+`; multiple participants use Add participant.
- Targeted cached send validation without full catalog/auth/container probes.
- Agent visibility in Configuration with default protection; checkout switching
  under Advanced, requiring explicit confirmation and session restart.

Settings and local `.aic/` data remain compatible. Run setup after upgrading and
confirm a ready agent. A repository-level default still overrides the global one.
Release publication and real participant usability measurements are separate
validation steps recorded in the plan checklist.
