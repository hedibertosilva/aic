# Chat plans frontend validation

Validated on 2026-10-07 using current `src/chat/web` assets, Chromium, isolated
Git/configuration directories and fixture CLIs. No real provider requests,
OAuth or quota consumption were performed.

## Results

- 14 plan browser tests passed without skips or failures. Thirteen use an
  isolated API fixture; one uses the real Rust HTTP API and fake provider CLI.
- 33 existing tests passed without skips or failures: 24 policy/import/quota
  tests and nine Chromium onboarding/quota tests.
- `node --check src/chat/web/chat.js` and `git diff --check` passed.
- Desktop and short-mobile screenshots were inspected. Tasks use a scrollable
  dialog to avoid overlapping the fixed message composer. Dialog bounds,
  internal spacing and safe rendering of untrusted plan text are tested.

Coverage includes free writes without plans, Cancel/Esc, plan creation, quick
tasks, reload persistence, per-participant task selection, discussion permissions,
duplicate concurrent task rejection, validation before completion, source draft
preservation, plan changes with automatic forks, native session reset, shared
plan forks, detaching, checkout selection/copy, external Markdown reload,
revision conflicts and local draft recovery after reload. A remote binding
change blocks sending rather than silently replacing the plan. Older servers
keep free chats usable while unverifiable linked chats remain blocked.

The live test creates a real plan, reserves a writing execution, verifies that
agent completion leaves the checklist unchecked, submits human validation,
checks TASKS.md and EXECUTION.md on disk, forks the shared binding and switches
the new conversation to free mode without changing its source plan.

## Reproduction

Provide Playwright/Chromium and the operating system libraries it needs. For
the live test, provide a newly built debug binary:

```sh
AIC_PLAYWRIGHT_MODULE=/path/to/playwright \
AIC_TEST_BINARY="$PWD/target/debug/aic" \
node --test tests/chat-plans.browser.cjs
```

When AIC_TEST_BINARY is absent, the live case is explicitly skipped. Existing
browser suites also require that variable. Tests of this frontend used current
source assets over the local HTTP server, including when the binary contained
an earlier JavaScript bundle. The final embedded-asset comparison found HTML
and CSS current and JavaScript requiring another build; rebuild/restart before
checking the packaged application. No Rust compilation or Rust-unit-test
result is claimed by this frontend validation.

See [feature behavior](chat-plans.md) and the [API contract](chat-plans-api.md).
