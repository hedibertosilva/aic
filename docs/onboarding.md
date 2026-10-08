# Setup, diagnosis and recovery

The intended audience is developers familiar with Git and a terminal. Rust and
manual TOML editing are unnecessary for onboarding from a release binary.

Run `aic setup` inside a Git checkout. Stages cover Environment, Sandbox, Agents,
Default agent and Conclusion. Back returns to a prior stage; Finish later and Esc
leave completed settings and tests available to the next invocation. Third-party
CLIs are detected, never installed automatically. Unused agents can be skipped.
Only one ready agent is needed, and its default selection is always explicit.

- Missing runtime: install/start Docker or Podman, or deliberately choose `none`
  to run on the host. Sandbox preparation does not compile the aic binary.
- Missing image: `aic sandbox build`, then resume setup.
- Missing CLI: use the official installation link displayed for that provider.
- Missing authentication: `aic agent login NAME`, then `aic agent test NAME`.
- Unknown/unavailable execution: `aic agent test NAME`; inspect the vendor CLI
  locally if it keeps failing. A network error is not evidence of logout.
- Quota exhausted: wait for reset or choose another ready agent. Do not repeat login
  solely because a provider's quota is exhausted.

An availability test makes a real call and may use quota. Interactive confirmation
is required; `--yes` explicitly consents in automation. `--timeout` is 1–300 seconds,
45 by default. It uses read-only execution, does not request file changes, and
stores only safe state and time in `.aic/diagnostics/tests.json`. Readiness is cached
for 24 hours, bound to binary/runtime/image/configuration, and downgraded after an
actual failed send. Success on the host is not proof of success in the sandbox.
Creating a chat or refreshing readiness never performs a paid generation test.

```sh
aic agent test codex
aic agent test codex --yes --timeout 45
aic setup --agent codex --yes --no-open
aic doctor
aic doctor --json
aic chat --no-open
```

The Environment panel shows repository, runtime, sandbox and independent agent
states, with last-test time and commands to copy. New chats open it automatically
when no agent is ready; a failed selected recipient can also open it for recovery.
Otherwise it is a
secondary status control. Opening, refreshing or closing it never changes the
conversation, participants, roles or sessions. Quota is displayed only in Usage
limits, while installation, authentication and execution stay in Environment.
Its indicator considers infrastructure, the default and agents used by the current
conversation; optional unused agents stay visible inside the panel without adding
global pending items.
Use the terminal for build, login and paid tests, then Refresh environment.
The web does not execute those commands.
The configuration dialog can change the global chat default only with explicit
confirmation and a ready agent; repository overrides retain precedence.

An agent is a configured integration. A participant is an instance of an agent in
one conversation, with name, model, effort, role and file access. New individual
chats use a ready explicit default. Without one, confirm/select a ready agent;
an unavailable default is explained and never silently replaced. Existing chats
keep participants and sessions after upgrading. Blocked sends preserve drafts.
The sidebar `+` is the only entry for creating a new chat. Without a ready default,
the new chat shows the existing Add participant and Import conversation cards,
without opening a dialog automatically. The first page load and keyboard shortcut
use the same flow. Choose Add participant to select a ready agent explicitly,
even with only one ready agent, or Import conversation to bring in existing history.
If the preferred agent is unavailable, the participant form explains the problem
when opened. Environment remains available for diagnosis and recovery.
Add is disabled until a ready agent is selected. To compare replies or form a
team, add participants with the existing Add participant control and set their
roles there; Environment never creates participants or assigns roles.
Role / instructions accepts multiple lines, starts with five visible rows and
can be resized vertically. Line breaks are retained when saving, editing and
sending the participant's instructions.

Before each send, a targeted preflight reads Git context and validates only the
recipient agents' cached tests. The backend repeats this validation before
starting an agent, protecting against changes after preflight. It checks local
binary/credential/configuration metadata and runtime/image identity, without
auth CLI calls or diagnostic containers. Catalogs are refreshed when the checkout
or configuration changes. Full diagnosis runs in Environment, setup and doctor.

Agent visibility lives in Configuration. Hiding an agent does not invalidate the
default or change existing participants; the effective and newly selected defaults
cannot be hidden. Visibility changes stay in the Configuration draft until Save
succeeds. Cancel and Esc discard them; a failed Save keeps only the editable draft.
Working checkout selection lives under the Git context → Advanced → This
conversation only, outside global Configuration. It cannot discard that form's
unsaved preferences. Confirmation restarts native sessions while preserving history.

By default `aic` opens the browser with `xdg-open`. In SSH/headless environments,
open the printed URL manually; `aic chat --no-open` suppresses launching a browser.
The server defaults to loopback. Existing Host and same-origin checks remain in
place; exposing it beyond loopback grants access to agents and mounted credentials.

Diagnosis/cache do not store credentials, account names, responses or vendor
stderr. No vendor secret should be pasted into a readiness report.
