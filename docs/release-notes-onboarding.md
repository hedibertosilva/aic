# Onboarding release notes

- Linux x86_64 static binary release asset.
- `aic setup`, consented `aic agent test` and read-only `aic doctor --json`.
- Contextual readiness and recovery before chat sends, without automatic paid tests.
- Explicit chat default: empty no longer inherits another configured default; no fallback
  to Claude or the first configured agent. Existing conversations retain sessions
  and participants. New chats without a ready default offer participant creation
  and import through the existing cards, without opening a dialog automatically.
- `aic` opens a browser; use `aic chat --no-open` for terminal/SSH environments.
- Persistent Git context and branch-change warnings with history/session preservation.
- Consultative Environment panel with recovery commands and bounded, scrollable
  dialogs. Chat creation uses only `+`; multiple participants use Add participant.
- Targeted cached send validation without full catalog/auth/container probes.
- Agent visibility saved with Configuration and discarded by Cancel/Esc, with
  default protection. Checkout switching is under this conversation's Git context
  → Advanced, requiring explicit confirmation and session restart.
- Participant selection explains unavailable chat defaults when opened explicitly.
  Role / instructions supports multiple lines and preserves line breaks.
  Environment status excludes optional unused agents; quota remains
  solely in Usage limits.

Settings and local `.aic/` data remain compatible. Run setup after upgrading and
confirm a ready agent. A repository-level default still overrides the global one.
Release publication and real participant usability measurements are separate
validation steps recorded in the plan checklist.
