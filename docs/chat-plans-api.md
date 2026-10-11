# Chat plans: frontend/backend API contract

The chat remains free unless a plan is explicitly linked. SQLite owns shared
plan, binding, task and execution records. Markdown files are projections.
The frontend uses the routes implemented in `src/chat/plans.rs`.

## Records and versions

- Plan: `{slug, title, checkout, version, content, tasks, executions,
  tasks_markdown, execution_markdown, projection_pending}`.
- Task: `{id, title, checked, cancelled, status, latest_execution}`.
- Execution: `{id, task_id, conversation_id, participant, agent, checkout,
  branch, sha, started_at, finished_at, state, files, validation}`.
- Binding: `{conversation_id, checkout, plan_slug, version, selections, origin}`.
  `plan_slug: null` means free conversation. Selections map participant names
  to task IDs. Origin records source conversation, previous/current plan,
  message count and source checkout.

There are three independent versions: plan `version`, binding `version`, and
chat-state `revision`. Each mutation supplies the applicable expected version;
HTTP 409 indicates a conflict. The frontend retains inputs, blocks stale
snapshot replacement and asks the user to refresh/reload.

## API

Every route accepts `checkout` in its query string (GET) or JSON body (POST).
IDs and slugs are encoded as individual path segments.

| Method | Route | Additional input | Response |
| --- | --- | --- | --- |
| GET | `/api/plans` | — | `{plans: Plan[]}` |
| POST | `/api/plans` | `{slug, title, content}` | Plan |
| GET | `/api/plans/:slug` | — | Plan |
| POST | `/api/plans/:slug/reload` | `{version}` | Plan reloaded from Markdown |
| POST | `/api/plans/:slug/tasks` | `{title, version}` | Updated Plan |
| POST | `/api/plans/:slug/tasks/:id/complete` | `{version, validation, human_confirmed: true, execution_id?}` | Updated Plan |
| GET | `/api/conversations/:id/plan` | — | Binding |
| POST | `/api/conversations/:id/plan` | `{plan_slug, version, state_revision, confirm_fork}` | Mutation result |
| POST | `/api/conversations/:id/fork` | `{state_revision, with_history: true}` | Mutation result |
| POST | `/api/conversations/:id/tasks` | `{version, participant, task_id: string\|null}` | Binding |
| POST | `/api/conversations/:id/checkout` | `{version, state_revision, confirm_fork, plan_action, plan_slug?}` | Mutation result |
| POST | `/api/conversations/:id/plan/undo` | `{version, state_revision}` | `{undone, conversation_id, revision}` |

A mutation result is `{binding, conversation, forked, revision}`. The server
chooses the fork ID and copies the persisted source conversation. With history,
changing a plan or checkout requires `confirm_fork: true`; native sessions and
task selections restart. Ordinary forks preserve the shared plan.

Checkout actions are `unlink`, `select` or `migrate`. Migration creates a copy
of the plan/checklist in the destination; the original plan and original
execution identities stay in the source checkout. The UI displays this effect
before confirmation. Undo removes an unused managed fork; after any turn has
started, it preserves the fork and returns to its source conversation.

## Snapshot and execution integration

`GET /api/chat/state` returns `{state, drafts, revision}` and hydrates each
conversation with authoritative `plan_binding`. `POST /api/chat/state` includes
that revision and returns the next revision. The browser keeps a display cache
but cannot change bindings through snapshot replacement.

`POST /api/chat` includes `{participant, binding_version, intent, task_id}` for
planned conversations. Intent is `discussion` or `execute`. Discussion sends
`write: false`; execution requires a task for every writing recipient. Free
conversations omit plan execution fields and retain their existing permissions.
The server validates the canonical binding, participant and task, and emits
`{type: "execution", id}` through the existing NDJSON stream. Successful agent
completion leaves the task awaiting validation; the UI requires explicit human
confirmation and validation evidence before marking it completed.

## Compatibility and tests

A 404 for the binding route leaves an unlinked chat usable on an older server.
A cached linked chat cannot execute until its binding is verified. Failed
mutations retain the original conversation, task text and composer draft.

`tests/chat-plans.browser.cjs` serves current assets against a fixture implementing
this contract. It validates frontend behavior, not Rust persistence or provider
execution. Run with `AIC_PLAYWRIGHT_MODULE=/path/to/playwright node --test
 tests/chat-plans.browser.cjs`. Rebuild/restart aic to embed changed web assets.
