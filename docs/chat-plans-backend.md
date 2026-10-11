# Shared chat plans: backend implementation

The [API contract](chat-plans-api.md) describes requests used by the frontend.
The Rust implementation is in `src/chat/plans.rs`, integrated into chat requests
and durable snapshots in `src/chat/server.rs`.

## Persistence and coordination

The checkout-local `.aic/chat/chat.sqlite3` stores shared plans, bindings,
execution attempts and running-turn leases. SQLite immediate transactions and
version checks serialize changes across request threads and database connections.
Plan version, binding version and chat snapshot revision are independent.
Unversioned legacy snapshots remain accepted only before server-managed bindings
exist. Snapshot replacement cannot modify a binding or silently move a linked
checkout. Free conversations preserve their existing write behavior.

PLAN.md, TASKS.md and EXECUTION.md remain under `.aic/plans/<slug>/`. Existing
files are imported on first access. Identified checkbox tasks are parsed, including
`[*]` cancellations, and their surrounding Markdown is retained. Creating a task
needs only a title; creating a plan does not require a complete interview.

The database commits projection intent before writing documents. Each document
is replaced atomically with a synced temporary file; a dirty outbox retries
partial projections on startup and API access. `projection_pending` is visible
instead of pretending the files were written. Slugs cannot traverse paths, and
symlinked directories/documents are rejected. External changes to managed
Markdown require an explicit versioned reload before another mutation. Agents
are instructed to leave those server-owned documents untouched.

## Bindings, execution and recovery

A plan change in an empty conversation updates it in place. A change with history
requires `confirm_fork`, preserving the original and its draft. Forks carry
provenance, messages and participants, with fresh sessions and no selected tasks.
Ordinary forks keep the same shared plan. Changing checkout explicitly unlinks,
selects, or copies the plan to an unused destination; a cancelled/stale operation
cannot leave migration documents. Undo removes an unused fork; once any turn was
reserved, it preserves the fork and only returns to the source.

Task assignments are per participant. In a planned conversation, discussion is
read only even if that participant has edit permission. Writing requires an open,
non-cancelled task and a matching binding version. Each task permits one running
attempt; different tasks can be executed concurrently. Plan framing and historical
origin are supplied by the server and resent for resumed CLI turns. Native sessions
cannot silently cross conversation/plan boundaries.

Before launching a provider the backend records task, participant, agent, Git
context and time. The NDJSON execution event links the reply to that attempt.
Changed-file observations compare Git-visible file signatures before and after
the turn, excluding `.aic/`; they are observations, not exclusive attribution when
other work occurs in the same checkout. Success becomes `validation_pending`,
while failure, cancellation or interruption becomes `partial`. Neither checks
the task. Explicit human confirmation, with any supplied validation evidence,
is the MVP completion authority; the frontend requires the evidence as well.
The checklist receives only a short last-execution note; full records live in
EXECUTION.md and SQLite.

Process/container identities are retained without prompts or credentials.
Startup recognizes abandoned leases by PID start time, stops recorded orphan
process groups/containers and recovers attempts as partial. Live server owners
are retained. The automated recovery test uses a simulated host provider; actual
vendor/OAuth behavior and runtime crash recovery on Docker/Podman were not exercised
in this round.

## Verification

Final backend verification: 126 Rust tests and 38 Node policy/HTTP integration
tests passed without failures or skips. The new five HTTP cases also passed
against the final rebuilt debug binary. `git diff --check` passed. The embedded
HTML, CSS and JavaScript were compared with current source files and matched.
The retryable-projection test also verifies that a document-write failure cannot
leave a task's running lease stuck.

Run a fresh build before HTTP tests:

```sh
cargo test --locked
cargo build --locked
AIC_TEST_BINARY="$PWD/target/debug/aic" \
  node --test tests/*.test.cjs tests/*.integration.cjs
```

Ten Rust plan tests cover shared plans, imported/cancelled tasks, guarded writes,
read-only discussion, explicit validation, forks/undo, preserved drafts,
optimistic versions, external reload, safe paths, checkout migration rollback and
retryable projections. Five HTTP tests use temporary Git repositories and fixture
CLIs for real server persistence, concurrent mutations, NDJSON execution linkage,
partial failures, free writing, origin protection and crash recovery. No provider
generation or OAuth calls are used.
