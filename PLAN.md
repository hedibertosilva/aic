# `aic` — CLI harness to orchestrate Claude Code + Gemini (Antigravity CLI)

## Context

Today the user's workflow is manual: (1) in the planning phase, they discuss a
feature, write a proposal in `.md`, and ask Gemini to review it — copying/pasting
between tools or files; (2) in the development phase, they ask Claude or Gemini to
execute the plan and then ask for a code review of the commits/staged changes,
again with manual interaction between the two CLIs.

The goal is to build a CLI (`aic`, from "ai-integrate") in **Rust** — an explicit
learning project for the user, who will review the generated code to learn the
language — that formalizes this workflow: it creates and tracks the plan, invokes
Gemini (via `agy`, the Antigravity CLI) or Claude (via `claude`) to review, and
invokes either of them to execute/review code, keeping state locally outside git
(`.aic/` goes in `.gitignore` — artifacts stay only on the machine, and the user
manually copies whatever they want to version).

After a first design round, two important adjustments:

1. **Config-first, not flag-first.** Instead of every command taking half a dozen
   flags (`--reviewer`, `--executor`, `--mode`, `--max-rounds`, `--permission-mode`,
   `--target`...), the CLI follows the model of Claude Code's `/config`: there is an
   interactive `aic config` command that sets the defaults (who reviews, who
   executes, manual/auto mode, round limit, base branch), and the day-to-day
   commands stay short (`aic plan review <slug>`, `aic dev execute <slug>`).
   Flags still exist, but only as a *one-off override*, not normal usage.
2. **Fixes from the review of the original plan** (see the "Revised decisions"
   section below): binary distribution outside the distrobox, semantics of the
   interactive execution mode, implementation order (validate the real `agy` JSON
   envelope before fixing the parser), auto-detected base branch, and error
   handling in the review loop.

Relevant findings from the survey:
- `claude -p "<prompt>" --output-format json --json-schema '<schema>'` and
  `agy -p "<prompt>" --output-format json --json-schema '<schema>'` have, per the
  documentation, **the same headless interface** (print mode, json/stream-json
  output, json-schema for structured output, `--permission-mode`, `--effort`,
  `--model`). This allows a single `CliAgent` abstraction parameterized by binary —
  but that assumption was only confirmed by actually running `claude --help`; `agy`
  hasn't been tested yet (see "Revised decisions").
- `claude` without `--print` starts an interactive session already with the initial
  prompt — great for the "follow along in the terminal" mode of plan execution.
- The environment is Fedora Silverblue (immutable); Rust is installed neither on the
  host nor in the `apps-env` distrobox (Fedora 41 container already used for general
  dev). We'll install the Rust toolchain (via `rustup`) **inside `apps-env`** instead
  of creating a new container — compilation happens there, but the final binary has
  to be exported to the host (see "Revised decisions").
- `agy` (Antigravity CLI) isn't installed yet — it needs to be installed and
  authenticated (once, via OAuth/keyring) **early**, to validate the real response
  format before implementing the parser.

## Revised decisions (after the first version of the plan)

- **Binary distribution**: after `cargo build --release` inside `apps-env`, run
  `distrobox-export --bin ~/aic/target/release/aic` to expose the binary on the
  Silverblue host's `PATH`. Without that, the CLI would only work from inside the
  container, useless for real day-to-day use in any repo on the host.
- **`dev execute --mode interactive`**: implemented with
  `std::os::unix::process::CommandExt::exec()`, which *replaces* the `aic` process
  with the agent process (`claude`/`agy`) — it inherits the real terminal (signals,
  stdin/stdout/stderr) without having to forward anything manually. Explicit
  consequence: `aic` runs no code after the `exec()` — there is no
  "post-processing" in this command. That's why the `HEAD` SHA is recorded *before*
  the exec, and why `dev review` is always a separate command, called afterwards.
- **Implementation order**: install and authenticate `agy` right after `cargo new`
  (it was step 9, became step 3), and manually capture a real response from
  `claude -p ... --output-format json --json-schema ...` and from
  `agy -p ... --output-format json --json-schema ...` for the same test prompt,
  comparing the two before writing `agent.rs`. If the envelopes diverge (different
  field names, for example), `CliAgent` gets a small per-agent mapping instead of
  assuming an identical shape.
- **Base branch**: instead of a fixed value (`main`) in the config, `git.rs` tries
  `git symbolic-ref refs/remotes/origin/HEAD` to discover the remote's real default
  branch; it only falls back to the config value (`default_base_branch`, optional)
  if that fails (e.g. repo with no remote).
- **Agent errors in the review loop**: `review::run_review_loop` treats
  `status != SUCCESS` (error, timeout, cancelled) as a hard failure — it aborts
  with a clear message and doesn't enter the auto loop trying to "fix" an execution
  error as if it were a content issue.
- **Default target of `dev review`**: if there are staged changes, use them;
  otherwise, if the feature has a start SHA recorded by `dev execute`, use the diff
  from that SHA to `HEAD`; otherwise, fall back to the diff against the
  auto-detected base branch.

## Config (`aic config`) — the heart of "fewer parameters"

`aic config` with no arguments opens an interactive menu (via `dialoguer`), similar
to Claude Code's `/config`: it lists the current keys and lets you pick one to
edit. `aic config show` prints the effective config (global plus, if present, the
repo override) in a readable format. `aic config set <key> <value>` still exists
for non-interactive/scriptable use, but it's the secondary path.

Config keys (`~/.config/aic/config.toml`, with an optional per-repository override
in `.aic/config.toml`):

```toml
executor = "claude"       # agent used by default to execute plans / fix issues
reviewer = "gemini"       # agent used by default to review plans / diffs
mode = "manual"           # "manual" (one round and stop) | "auto" (loop until approved)
max_rounds = 3            # cap for auto mode
permission_mode = "acceptEdits"  # forwarded as --permission-mode to the executor
default_base_branch = ""  # optional; empty = auto-detect via origin/HEAD

[agents.claude]
binary = "claude"
model = ""                # empty = use claude's own default
effort = ""

[agents.gemini]
binary = "agy"
model = ""
effort = ""
```

With that, day-to-day usage becomes:

```
aic plan review auth-flow          # uses reviewer/executor/mode/max_rounds from config
aic dev execute auth-flow          # uses executor/mode/permission_mode from config
aic dev review auth-flow           # uses reviewer/executor/mode/max_rounds from config, auto-detected target
```

And the per-command flags (`--reviewer`, `--executor`, `--mode`, `--max-rounds`,
`--permission-mode`, `--target`, `--range`) still exist on every relevant command,
but only to *override* the config on a one-off call — they aren't needed in normal
use.

## v1 scope (MVP)

Single `aic` binary, commands:

1. **`aic init`** — creates `~/.config/aic/config.toml` (if it doesn't exist, with
   the defaults above) and the `.aic/` skeleton in the current repository
   (`.aic/plans/`, `.aic/dev/`, `.aic/state.json`), adding `.aic/` to the repo's
   `.gitignore` (creates the file if it doesn't exist, only appends if it already
   exists and the line isn't present).

2. **`aic plan new <slug> [--title "..."]`** — creates
   `.aic/plans/<slug>/plan.md` from a fixed template (Problem, Proposal,
   Alternatives considered, Impact, Open questions) and registers the feature in
   `state.json` with status `draft`. It doesn't automate the "discussion" — that
   remains a normal conversation with Claude/the editor; the command only
   formalizes where the file lives.

3. **`aic plan review <slug> [--reviewer <agent>] [--executor <agent>] [--mode
   manual|auto] [--max-rounds N]`** (all flags optional, default comes from the
   config)
   - Reads `plan.md`, builds a fixed review prompt + the file content, calls the
     `reviewer` in headless mode with `--json-schema` asking for
     `{approved: bool, summary: string, issues: [string]}`.
   - Saves the response to `.aic/plans/<slug>/reviews/plan-rN.md` (timestamp,
     agent, summary, issues) and updates `state.json` (round, approved, status
     `approved`/`changes_requested`).
   - `manual` mode (default): runs one round and stops — prints the result for the
     user to decide the next step.
   - `auto` mode: if not approved, calls the `executor` in headless mode asking it
     to revise `plan.md` addressing the `issues` (with the configured
     `--permission-mode`, restricted to the plan file), runs the reviewer again,
     and repeats until `approved=true`, `max_rounds` is reached, or a hard agent
     error (aborts).

4. **`aic dev execute <slug> [--mode interactive|headless] [--executor <agent>]
   [--permission-mode <mode>]`**
   - `interactive` (default): `exec()` into the `executor` process, initial prompt
     "execute the plan in `.aic/plans/<slug>/plan.md`" — the user follows/takes part
     normally; `aic` runs nothing else after that in this invocation.
   - `headless`: runs in `--print`, captures JSON, writes an execution record to
     `.aic/dev/<slug>/execution-log.md` (timestamp, status, response summary).
   - In both cases, records the `HEAD` SHA in `state.json` **before** starting —
     used later by `dev review` to know the feature's commit range.

5. **`aic dev review [<slug>] [--target staged|commits|branch] [--range <rev>]
   [--reviewer <agent>] [--executor <agent>] [--mode manual|auto] [--max-rounds
   N]`**
   - Default target (without an explicit `--target`): staged if any; otherwise
     commits since the SHA recorded by `dev execute`; otherwise the diff against
     the auto-detected base branch.
   - Without `<slug>`: ad-hoc review, not tied to a feature — saved to
     `.aic/adhoc-reviews/<timestamp>.md`.
   - Same `--json-schema` pattern for `{approved, summary, issues: [{file,
     description, severity}]}`, same manual/auto loop as `plan review`.

6. **`aic status [<slug>]`** — reads `state.json` and prints the state (plan:
   draft/in_review/approved + rounds; dev: not_started/executing/review/done +
   rounds) of one feature or all of them. Without `<slug>`, tries to infer the
   current feature from the git branch name (slugified) before listing all.

7. **`aic config`** (interactive, no args), **`aic config show`**, **`aic config
   set <key> <value>`** — per the "Config" section above.

In every command that takes `<slug>` as an optional/first positional argument
(`dev execute`, `dev review`, `status`), if omitted the CLI tries to infer it from
the current git branch name (slugified) and only asks explicitly if it can't find
a matching feature in `state.json`.

Out of scope for v1 (leave for later, don't implement now): multiple
simultaneous repositories/workspaces, GitHub PR integration, support for more
agents beyond `claude`/`agy`, per-project customizable plan templates.

## Architecture / modules

Binary crate `aic` (edition 2021), main dependencies: `clap` (derive),
`serde`/`serde_json`, `toml`, `anyhow`, `chrono`, `dialoguer` (interactive menu for
`config` and confirmations in manual mode), `owo-colors` (colored output).

```
src/
  main.rs            // clap parsing, dispatch to the command handlers
  cli.rs             // clap structs/enums (Cli, Commands, subcommands; all the
                     // override flags are Option<T>, default = None = "use the config")
  config.rs          // Config (toml, global + repo override), load/save/merge, defaults,
                     // interactive menu (dialoguer) for `aic config` with no args
  state.rs           // State/Feature/Round (serde), load/save .aic/state.json
  agent.rs           // AgentRunner trait (to allow a fake in tests) + CliAgent (real impl
                     // via std::process::Command), RunOpts, JSON envelope parsing
  review.rs          // ReviewVerdict { approved, summary, issues }, fixed schema,
                     // generic run_review_loop() (used by plan review and dev review),
                     // treats status != SUCCESS as a hard error (not "not approved")
  git.rs             // diff_staged(), diff_range(a,b), diff_against_branch(name),
                     // default_branch() (via origin/HEAD, falling back to config),
                     // head_sha(), is_repo(), current_branch_slug()
  templates.rs       // fixed plan.md template
  commands/
    init.rs
    plan.rs          // new, review
    dev.rs           // execute, review
    status.rs
    config_cmd.rs
```

`CliAgent::run` is the single point that invokes `claude`/`agy` as a subprocess —
it builds the args according to `RunOpts`, captures stdout/stderr, and returns
`AgentRunResult { status, raw_text, json: Option<Value> }`. Both `plan review` and
`dev review` call `review::run_review_loop`, passing the "content to review" (plan
text or diff), the specific base prompt, and the resolved executor/reviewer pair
(flag > repo config > global config) — the loop logic is shared, not duplicated
between the two commands.

Errors: `anyhow::Result` everywhere, with context (`.context("...")`) at the I/O
and subprocess-call points — including a clear message if the configured binary
isn't found on the PATH.

## Implementation steps

1. Install Rust inside `apps-env` (distrobox): `distrobox enter apps-env --
   bash -lc 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s --
   -y'`.
2. `cargo new aic` in `/var/home/hediberto/Workspace/ai-integrate` (also `git init`,
   to version the CLI's own code).
3. Install and authenticate `agy` (Antigravity CLI) — manual step for the user
   (opens a browser once). Manually run `claude -p "..." --output-format
   json --json-schema '...'` and `agy -p "..." --output-format json --json-schema
   '...'` with the same test prompt/schema, and compare the two real JSON envelopes
   before moving on — this defines the exact shape of `AgentRunResult`.
4. Add dependencies to `Cargo.toml`.
5. Implement `config.rs` + `state.rs` (models + load/save/merge) and unit tests of
   (de)serialization, including the global+repo merge.
6. Implement `agent.rs` (`AgentRunner` trait + `CliAgent`, builds args, runs the
   subprocess, parses the envelope) with unit tests of the parsing using the real
   responses captured in step 3 as fixtures.
7. Implement `git.rs` (including `default_branch()` via `origin/HEAD`) with tests
   against a temporary git repo created in `tempdir`.
8. Implement `review.rs` (generic loop, including hard-error handling) with tests
   using a fake `AgentRunner`.
9. Implement the commands (`init`, `plan new/review`, `dev execute/review`,
   `status`, interactive `config` menu) wiring the modules above together.
10. `distrobox-export --bin` of the release binary to the host.
11. Manual smoke test: `aic init` → `aic plan new demo` → edit `plan.md` →
    `aic plan review demo` (calls the real `agy`, using the config defaults) →
    `git add` something for testing → `aic dev review demo` → `aic status`.

## Verification

- `cargo build` and `cargo test` (inside `apps-env`) after each implemented module
  — covers config/state/agent-envelope/git diff parsing without depending on the
  network, using real fixtures captured in step 3.
- Manual end-to-end test (step 11 above) with the real `agy` and `claude`, in a
  disposable test repo, confirming that: the files in `.aic/` are created
  correctly; `.aic/` doesn't show up in `git status`; the `aic` exported via
  `distrobox-export` runs directly from the host shell, outside the container.
