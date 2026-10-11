# Usage limits — UX specification

Approved scope: web chat, English interface, persistent warning at ≥80%, with
selection, saving and sending never blocked. “Send to…” is out of scope for v1.
Technical discovery: agent-quotas-spike.md.

## Deliverables

1. **Phase 1:** per-conversation/agent usage in SQLite, agent recorded per turn,
   corrected Codex parser, Codex/Claude/agy quota readings, server memory cache,
   panel, badges and selector notes.
2. **Phase 2:** classified failure card (quota vs. rate limit), after event
   ordering and a real limit failure are captured.
3. **Phase 3:** *(absorbed into phase 1 for agy.)*

An agent whose reading fails, or whose account has no windows, uses explicit
states and observed usage. For agy, the panel and alerts use the model group
that governs the participant's model; an unrecognized model keeps all windows.

## Access points

- A meter button beside Configuration opens **Usage limits**. It lists every
  configured agent, including agents absent from the conversation.
- Participant chips inherit the agent state: **Near limit** or a percentage at
  80%+, and **Exhausted** at 100%+. Participants sharing an agent share its
  quota reading.
- **To:** pills receive an alert icon, tooltip and accessible description.
- The participant dialog shows a note below Agent. Options receive a short
  warning/exhausted suffix; Save remains enabled.

## Agent row structure

Each row has three equally important levels:

1. Agent name, most critical window summary and details control.
2. Bar/reset when available; explanatory text otherwise.
3. **This conversation:** observed usage, even when quota is available.

Details expand all windows/buckets, source, last reading and last attempt when
an error occurred. The most critical window has the highest alert state and,
within that state, the highest percentage. Shared quota must not be presented
as independent quotas per participant or model.

## States

| State | Content | Treatment |
| --- | --- | --- |
| Normal | 34% · 5h and reset | Neutral bar |
| ≥80% | Near limit 86% · wk | warn bar/badge |
| ≥100% | Exhausted and reset | err bar/badge |
| Before first reading | No quota reading yet | No bar; disappears after reading |
| Unavailable | Quota not available and reason | No bar or track |
| First reading | Checking… | Per-agent indicator |
| Warning without percentage | Near limit (reported by provider) | No number or bar |
| Rejection without percentage | Exhausted | Do not invent a percentage |
| Error with previous reading | Previous value, error and Retry | 50% opacity bar; preserve fetched_at |
| Error without previous reading | Couldn't read quota and Retry | No bar |
| Expired window | Previous value and Reset at… | Stale data; query automatically; never reset locally |

Reading errors and stale data do not erase previous values. Source and time are
per agent; a global timestamp must not imply that all agents were refreshed.
Readings older than 15 minutes show their age.

## Reset and language

The web contract uses UTC instants; the browser displays local time. For a reset
under 24 hours away: “resets in 3h 10m (17:40)”. For longer intervals, show
date and time. Without a reset: “reset time unknown”. Abbreviations: 300 minutes
→ 5h; 10080 → wk; other durations keep the adapter's label.

Do not promise that the next response will provide quota. In an exhausted-agent
dialog, use “Replies may fail”, because the limit may affect only one bucket.

## Usage

Usage measures executions in this aic conversation, not total account usage.
Per-conversation/agent counters and cumulative counter references live in the
existing SQLite database; there is no turn ledger.

- Missing fields remain n/a; never invent zero.
- Partial totals use a ≥ prefix where coverage is incomplete and indicate how
  many replies did not report usage.
- No executions: “No usage yet in this conversation”.
- Older history without a recorded agent gets one panel note, outside per-agent
  totals.
- Deleting/regenerating a reply does not return tokens. Copying/importing
  history does not charge its old executions to the new conversation.
- If an old session's first cumulative reading cannot isolate current spend,
  establish a baseline and treat that turn as partial.

## Refresh and cache

The panel opens immediately; readings run in parallel with chat use. Manual
Refresh updates enabled adapters with a per-row indicator. Refresh is disabled
while a reading is in progress. At the end of a turn, refresh only that agent,
without extra animation. Concurrent requests for one agent are deduplicated,
including across tabs.

The server keeps the last reading per agent in memory: page reloads recover the
value and timestamp. The cache does not survive a server restart in v1. Relevant
configuration changes invalidate it. Stale readings get limited retries; a
reading failure never blocks sending.

## Turn failures — phase 2

A special card is used only with a structured classification:

| Type | Title | Action | Effect |
| --- | --- | --- | --- |
| quota | Agent usage limit reached | Change agent and retry | Marks only the confirmed bucket exhausted |
| rate_limit | Agent is rate limited | Try again; secondary Change agent | Does not mark quota exhausted |
| Authentication/network/ambiguous | Normal error | Existing retry | Does not change quota |

Show reset only when reported; otherwise say “Reset time not reported by the
agent”. Keep the original error in Details. Change agent reuses the participant
dialog and focuses Agent. In phase 1, failures remain normal cards and preserve
the Codex reason.

## Accessibility and responsiveness

- Use existing tokens, with text/icon in addition to color.
- Show a bar only for a known percentage; include the window/reset in the
  accessible value.
- Announce warning/exhaustion once per state change.
- The panel can close by button, outside click or Escape, returning focus to the
  button when closed by keyboard.
- At ≤700px, the panel uses the available chat width.
- Indicators respect prefers-reduced-motion.

## Phase 1 acceptance

With Claude, Codex and Gemini configured, all appear in the panel. Codex shows
windows when available; Claude/Gemini show usage and an explicit state. Missing
values never appear as 0% or $0.00. Reload preserves the cached reading while
the server remains active. Counts remain correct when changing agents,
regenerating/deleting replies and copying history.

## Delivery validation

Phase 1 passed 91 Rust tests, 17 JavaScript tests and HTTP/Chromium integration
checks. HTTP tests use controlled CLIs to verify Codex normalization, conversation
isolation, missing Usage, previous-reading preservation and persistence after a
server restart. Browser tests cover all three agents, reload cache, 80%/100%,
selector, sending, regeneration/deletion, narrow panels and read failures.

To repeat the checks:

~~~sh
cargo test
cargo build
AIC_TEST_BINARY="$PWD/target/debug/aic" node --test tests/*.cjs
~~~

Browser tests are optional when Playwright/Chromium are unavailable; install
them and set AIC_PLAYWRIGHT_MODULE when the module is outside the repository.
