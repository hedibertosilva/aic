---
name: aic-dev-review
description: "AIC: Run aic dev review on the current code changes, and auto-fix if the reviewer requests changes (up to 2 fix cycles)"
argument-hint: "[slug]"
allowed-tools:
  - Bash
  - Read
  - Edit
---

<objective>
Get the current code changes reviewed via `aic dev review`, and close the loop
yourself when the reviewer requests changes: read the issues, fix the code directly in
this conversation, and re-review — without relying on `aic`'s own `--mode auto`.
</objective>

<context>
Arguments: $ARGUMENTS — an optional feature slug. If omitted, `aic` infers one from the
current branch, or falls back to an ad-hoc review (not tied to any feature).
</context>

<process>
1. Run `aic dev review <slug> --mode manual` if a slug was given, otherwise
   `aic dev review --mode manual`. Always pass `--mode manual` explicitly — this skill
   does its own fix loop.
2. Find this round's report file:
   - If a slug was given and it's a tracked feature: read `.aic/state.json`,
     `features.<slug>.dev.rounds`, take the last round's `report_path`.
   - Otherwise (ad-hoc review, no slug or an untracked one): find the most recently
     modified file under `.aic/adhoc/reviews/`.
   Read that file — it has the full `Summary` and `Issues` list.
3. If the round's `approved` is true: tell the user it's approved. Stop here.
4. If not approved:
   a. Address every issue from the report directly in the code, using Edit (and Bash
      for things like re-running tests if that's relevant to verifying a fix). Don't
      ask permission for clear-cut fixes; for anything that's a real design/scope
      decision, ask the user inline before making that specific change.
   b. Re-run the same `aic dev review ...` command from step 1 and repeat from step 2.
5. Cap this at **2 fix-and-re-review cycles** total (up to 3 review calls overall). If
   still not approved after that, stop, summarize what's still blocking approval, and
   ask the user how they want to proceed.
</process>
