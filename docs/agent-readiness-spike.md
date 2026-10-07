# Agent readiness spike

Inspected installed vendor help and primary documentation on 2026-10-07.
The existing quota spike supplies observed /usage envelopes; those observations
are version-specific, not a promise of a stable protocol.

| Agent | Authentication observation | Real execution | Error handling |
|---|---|---|---|
| Claude | `claude auth status --json`: `loggedIn` boolean, no raw payload retained | Existing `-p` stream-json parser; successful nonempty final result | Structured result error followed by conservative classification |
| Codex | `codex login status`: documented status command; accept only recognized logged-in/not-logged-in output | Existing `exec --json`, agent_message plus turn.completed | Structured turn.failed/error; do not infer quota from every HTTP 429 |
| Gemini (agy) | No verified free standalone auth status; remain unknown until successful test | Existing `--print --output-format stream-json`, result envelope | Structured result ERROR; unknown errors stay execution failure |

Binary lookup checks executable access. For a sandbox, installation is checked
inside the configured image with a read-only `command -v`; host detection is a
separate observation. Missing runtime/image prevents tests. Credential success
on the host never establishes sandbox readiness. CLI probes use process-group
cancellation and bounded output/time. No provider generation was performed in
this spike; vendor failures will be verified with deterministic fixture captures.

Authentication, network and quota messages are classified only when they match
specific known phrases/codes. Ambiguous failures stay unknown or execution errors,
with generic safe guidance. No output, secrets, account names or raw stderr enters
the web/cache. Quota windows reuse the current quota implementation and are not
used as proof of authenticated execution.

Sources:
- [Claude CLI reference](https://code.claude.com/docs/en/cli-reference)
- [Codex CLI reference](https://developers.openai.com/codex/cli/reference)
- [Antigravity headless mode](https://www.agy.dev/docs/cli/headless/)
- [Antigravity upstream](https://github.com/google-antigravity/antigravity-cli)
- [Existing observed quota spike](agent-quotas-spike.md)
