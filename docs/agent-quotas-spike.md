# Spike: agent quotas and limits in chat

Date: 2026-10-03. Technical discovery for quota visibility in the web chat.

## Confirmed scope

Only the web chat is in scope. Show a persistent warning at **≥80%** and
exhausted at **≥100%**. When quota is unavailable, show observed conversation
usage per agent and “quota unavailable”. Missing tokens or cost remain missing,
with “not reported”; never estimate them as zero. CLI commands, billing,
history, custom limits and threshold configuration are out of scope for v1.

## Environment and method

The investigation ran in the Debian 13/trixie sandbox with Node available:

| CLI | Version |
| --- | --- |
| Claude Code | 2.1.287, matching the pinned version in src/templates.rs |
| Codex | 0.160.0 |
| agy / agent configured as gemini | 1.2.16 |

The work used provider help and documentation, repository parser inspection,
the Codex-generated schema and non-generating queries. No account was exhausted
to produce real quota errors. Claude and agy were not authenticated; Codex was
authenticated and its quota query worked. Docker/Podman was unavailable for a
host-to-container launch test.

## Agent capability matrix

| Agent | Quota source | Sandbox validation | Cost and latency | Limit error and v1 decision |
| --- | --- | --- | --- | --- |
| Codex | codex app-server over stdio, account/rateLimits/read; percentage, duration and reset in primary/secondary | Authenticated query confirmed; returned 300- and 10080-minute windows | One read took 2304 ms including startup; no generation turn was started | Preserve structured turn.failed/error; R1 when windows are returned, otherwise R2 |
| Claude | claude -p /usage; stream-json assistant event includes usage_report.rate_limits.limits[] | Host validation confirmed on 2026-10-03; not validated in sandbox | 1.5 s JSON and 3 s stream-json; no model call | Contract observed in pinned version, not officially documented; R1 |
| gemini / agy | agy --print /usage --output-format json; groups contain windows, remaining fraction and reset time | Host validation confirmed on 2026-10-03; not validated in sandbox | No generation tokens; unauthenticated query falls into OAuth and times out | Quota is per model group, not agent; R1, with model-group fallback |

These are single samples, not benchmarks. A second Codex query took 3495 ms
and returned both windows. The probe passed syntax validation and returned
ENOENT for a missing binary. Running a command on trixie proves binary
compatibility, but not credentials or host keyring behavior.

## Evidence and mechanisms

### Codex

The [official App Server](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt)
documents account/rateLimits/read and account/rateLimits/updated. The sequence
is initialize → initialized → account/rateLimits/read, without a generation
turn. The [reproducible probe](agent-quotas-probe.cjs) implements the sequence,
timeout and output limited to windows, source and timestamps:

~~~sh
node docs/agent-quotas-probe.cjs
~~~

Prefer rateLimitsByLimitId with rateLimits as fallback. Authentication, API and
provider responses without windows use fallback R2; not every install has
subscription quota. The generated schema includes usageLimitExceeded and
rateLimitExceeded, but these app-server codes must not be assumed to arrive
unchanged in the chat's exec --json path.

### Claude

The [Anthropic SDK parser](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/message_parser.py)
recognizes rate_limit_event with status, resetsAt, rateLimitType and utilization.
Convert utilization to a percentage while preserving missing values. A provider
warning without a percentage does not authorize inventing “80%”.

The [official status line](https://code.claude.com/docs/en/statusline#rate-limit-usage)
documents 5-hour/7-day percentages and resets, but this is a TUI payload and
does not by itself provide a preflight chat query. Do not depend on private
endpoints for v1.

### agy

The provider documents /usage and /quota as quota panels. Convert
remaining_fraction to used percentage with 100 × (1 − remaining_fraction).
Do not confuse context-window usage with account quota. Quota is grouped by
model; if no group recognizes a participant model, keep all windows visible.

The tested commands failed before querying because login was unavailable. That
is an authentication error, not a quota error. An authenticated account is
needed to confirm the exact JSON and stream-json output in --print mode.

## Implications for code and data model

The current Usage type is reusable but does not cover the full initiative:

- Codex parsing must preserve turn.completed.usage and structured terminal
  errors, including error.message.
- Normalize Claude, agy and Codex cache fields without double-counting cached
  input.
- Treat resumed-session totals as cumulative and calculate a per-session delta.
- Record agent and model on each turn so changing or removing a participant
  cannot reassign historical usage.
- Define conversation usage as executions performed in that conversation.
  Copying history does not create usage; deleting a reply does not return tokens.
- Include failures when usage is reported; missing usage produces partial totals.

Proposed contract:

~~~text
AgentQuota {
  agent, state: available | unavailable | read_error,
  windows: [QuotaWindow],
  source?, fetched_at?, attempted_at?, error?
}
QuotaWindow {
  id, label?, model?, used_pct?, window_minutes?, resets_at?, provider_status?
}
QuotaFailure { agent, kind: quota | rate_limit, reason, resets_at? }
~~~

Provider fields are optional, but identity, state and windows have explicit
meaning. windows=[] does not mean zero usage. A failed read must not update the
timestamp of older data; store attempted_at separately and mark stale data.
A window needs a fresh read, not a local reset to zero.

Preserve all windows with stable IDs instead of one window field. Quota belongs
to the provider account/bucket; observed usage belongs to the aic conversation.
Several participants using the same agent may share one quota.

## v1 and UX direction

R1 can begin with Codex and complete Claude events; R2 guarantees that every
agent is represented, including agy and Claude before a first valid reading.
The UI should show a compact per-agent summary, expandable window details with
source/time/reset and conversation usage even when quota exists.

Failure messages should distinguish temporary rate limiting, exhausted quota,
authentication and network errors. Manual and post-turn refreshes must run
outside generation, with timeout, cache and per-agent deduplication. Never ask
the model for its own quota, and never block a turn because quota lookup failed.

## Remaining validation

1. Capture authenticated Claude rate_limit_event data in --print.
2. Capture authenticated agy /usage in JSON and stream-json.
3. Capture natural limit failures and validate reset/classification metadata.
4. Verify resumed-session deltas and cache/token semantics for each CLI.
5. Run probes through Sandbox::run_args with real credential mounts.

The spike delivered the matrix, the Codex probe and technical decisions. Phase 1
was later implemented with the Codex adapter, server cache, SQLite counters and
web interface. The consolidated UX scope is in agent-quotas-ux.md.
