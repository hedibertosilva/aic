---
name: plan
description: "Scaffold a new aic feature plan and interview the user to fill it in"
argument-hint: "<slug> [title]"
allowed-tools:
  - Bash
  - Read
  - Edit
---

<objective>
Start a new feature plan tracked by `aic`: create `.aic/plans/<slug>/PLAN.md` if it
doesn't exist yet, then interview the user conversationally to fill in its sections,
editing the file directly as answers come in.
</objective>

<context>
Arguments: $ARGUMENTS — first token is the slug (required), rest (if any) is an
optional `--title` value to pass through.
</context>

<process>
1. Parse `$ARGUMENTS` into `<slug>` and an optional title.
2. Run `aic plan new <slug> --no-open` (so it doesn't launch an interactive agent; add `--title "<title>"` if one was given). If it fails
   because the plan already exists, that's fine — just continue to step 3 with the
   existing file instead of treating it as an error.
3. Read `.aic/plans/<slug>/PLAN.md`.
4. Interview the user, section by section, to fill in whatever is still just the
   template's HTML comment placeholder (`<!-- ... -->`) — skip sections that already
   have real content (e.g. on a plan you're resuming):
   - **Problem**: what problem this solves, and for whom.
   - **Proposal**: what will be built, at a high level.
   - **Alternatives considered**: what else was considered and why it wasn't chosen.
     If the user has no alternatives in mind, it's fine to leave this section noting
     that explicitly rather than inventing ones.
   - **Impact**: what changes for users, other teams, infrastructure.
   - **Open questions**: anything still undecided.
   Have a normal back-and-forth conversation for this, one question per reply, and wait
   for the user's answer in their next message — these are prose answers, not discrete
   choices. Don't depend on a question tool: it doesn't exist in headless runs such as
   `aic chat`. Use your own judgment on follow-up questions when an answer is vague or
   incomplete.
5. After each section is answered, use Edit to write it into the corresponding heading
   in `PLAN.md`, replacing the placeholder comment. Keep the user's own words and
   framing — don't rewrite them into corporate-speak.
6. Once all sections have real content, tell the user the plan is ready.
</process>
