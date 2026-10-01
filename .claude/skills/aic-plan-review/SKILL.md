---
name: aic-plan-review
description: "AIC: Run aic plan review, and auto-fix PLAN.md if the reviewer requests changes (up to 2 fix cycles)"
argument-hint: "<slug>"
allowed-tools:
  - Bash
  - Read
  - Edit
---

<objective>
Get a feature's `PLAN.md` reviewed via `aic plan review`, and close the loop yourself
when the reviewer (by default, Gemini via `agy`) requests changes: read the issues, fix
the plan directly, and re-review — without relying on `aic`'s own `--mode auto` (which
would spawn a separate executor subprocess instead of using this conversation).
</objective>

<context>
Arguments: $ARGUMENTS — the feature slug (required).
</context>

<process>
1. Run `aic plan review <slug> --mode manual`. Always pass `--mode manual` explicitly
   here, regardless of the user's configured default — this skill does its own fix
   loop, so `aic`'s own auto-mode must stay off to avoid doubling up.
2. Read `.aic/state.json`, find `features.<slug>.plan.rounds`, and take the **last**
   round's `report_path`. Read that file — it has the full `Summary` and `Issues` list
   (the command's own stdout only shows a short summary line).
3. If the round's `approved` is true: tell the user it's approved and suggest moving on
   to execution (in the normal chat, not via `aic` — `dev execute` is intentionally not
   wrapped by a skill). Stop here.
4. If not approved:
   a. Read the current `.aic/plans/<slug>/PLAN.md`.
   b. Address every issue from the report directly in the plan, using Edit. Don't ask
      the user for permission to fix obvious issues (typos, missing sections, vague
      wording) — just fix them. If an issue requires a judgment call only the user can
      make (e.g. a genuine product/scope decision), ask them inline before editing that
      part, but keep fixing the rest.
   c. Re-run `aic plan review <slug> --mode manual` and repeat from step 2.
5. Cap this at **2 fix-and-re-review cycles** total (i.e. up to 3 review calls overall:
   the first one plus 2 retries). If still not approved after that, stop, summarize
   what's still blocking approval, and ask the user how they want to proceed — don't
   keep looping indefinitely on something that isn't converging.
</process>
