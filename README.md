# aic

CLI that orchestrates Claude Code and Gemini (via the [Antigravity CLI](https://antigravity.google/docs/cli/using/), `agy`) across the two phases of a feature workflow: planning (discussing and reviewing a proposal in `.md`) and development (executing the approved plan and reviewing the resulting code). One agent writes the code, the other reviews it — no login to any third-party app, everything goes through local CLIs.

It follows the "config-first" model of Claude Code's `/config`: you set the defaults (who reviews, who executes, manual/auto mode) once, and the day-to-day commands stay short.

## Installation

Requires Rust (`cargo`/`rustc`) and the `claude` and `agy` (Antigravity CLI) binaries, already installed and authenticated.

```
cargo build --release
cp target/release/aic ~/.local/bin/aic   # or another directory on your PATH
```

> If you build inside a distrobox/container, copy the binary straight to the host instead of using `distrobox-export --bin` — the generated wrapper runs `aic` *inside* the container on every call, which can make its own tools (`git`, etc.) resolve inside the container too.

## Configuration

`aic config` opens an interactive menu (like Claude Code's `/config`) to adjust the defaults, stored in `~/.config/aic/config.toml`:

```toml
executor = "claude"              # who executes plans / fixes issues
reviewer = "gemini"              # who reviews plans / diffs
mode = "manual"                  # "manual" (one round and stop) | "auto" (loop until approved)
max_rounds = 3                   # cap for auto mode
permission_mode = "acceptEdits"  # "acceptEdits" | "plan"
default_base_branch = ""         # empty = auto-detect via origin/HEAD
skip_permissions = false         # opt-in: auto-approve every tool in headless runs

[agents.claude]
binary = "claude"
model = ""
effort = ""

[agents.gemini]
binary = "agy"
model = ""
effort = ""
```

A repository can override any key in `.aic/config.toml` (same schema, partial fields).

Outside the interactive menu:

```
aic config show                  # effective config (global + repo override)
aic config set mode auto
aic config set max_rounds 5
```

Every command below accepts flags (`--reviewer`, `--executor`, `--mode`, `--max-rounds`, `--permission-mode`, `--target`, `--no-filter`) to override the config for a single call — but in normal use they aren't needed.

**`git_diff_exclude_patterns` / `git_diff_include_patterns`**: `aic dev review` filters the diff before sending it to the reviewer, removing common lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, ...), files ignored by `.gitignore`, and files marked `linguist-generated` in `.gitattributes`. `git_diff_exclude_patterns` (list, default `[]`) adds exclusions; `git_diff_include_patterns` re-includes whatever matches, taking precedence over any exclusion. Patterns use a subset of the gitignore syntax: `*` and `?` (within a single path segment) and `**` (crosses `/`). Without a `/`, the pattern matches any path segment (`node_modules`, `*.min.js`); with a `/`, it is anchored at the repo root and also covers everything inside the matched directory. A trailing `/` is ignored. There is no `!` negation (use `git_diff_include_patterns`) and no `[...]` classes. Use `aic dev review --no-filter` to see the full diff in a single call.

**`skip_permissions`**: off by default. Without it, if the reviewer or executor tries to use a tool (read a file, run a command) in headless mode, the call is denied and fails with a clear error instead of silently returning an empty response. Only enable it if you trust the configured agents to run unattended in that repository.

## Commands

| Command | What it does |
|---|---|
| `aic init` | Creates `.aic/` in the current repo (`plans/`, `dev/`, `adhoc/reviews/`, `state.json`) and the global config, if they don't exist. Adds `.aic/` to `.gitignore`. |
| `aic plan new <slug> [--title "..."] [--no-open]` | Creates `.aic/plans/<slug>/PLAN.md` from a template (Problem, Proposal, Alternatives, Impact, Open questions) and registers the feature. Then opens the executor (claude/gemini) interactively to start the discussion; `--no-open` skips that. |
| `aic plan review <slug>` | Sends `PLAN.md` to the reviewer; saves the report to `.aic/plans/<slug>/reviews/PLAN-RN.md`. `auto` mode fixes and re-reviews until approved. |
| `aic dev execute <slug> [--mode interactive\|headless]` | Runs the executor against the approved plan. `interactive` (default) takes over the terminal (`exec`), like a normal session; `headless` runs via `-p` and logs to `.aic/dev/<slug>/EXECUTION-LOG.md`. |
| `aic dev review [<slug>]` | Reviews a diff: staged, commits since `dev execute`, or against the base branch — chosen automatically, or via `--target`. With `<slug>`, saves to `.aic/dev/<slug>/reviews/CODE-RN.md`. Without `<slug>`, it's an ad-hoc review saved under `.aic/adhoc/reviews/`. |
| `aic status [<slug>]` | Shows the state (plan/dev, rounds) of one feature or all of them. With no argument, tries to infer the feature from the current branch name. |
| `aic config` / `config show` / `config set` | View/edit the defaults. |

`.aic/` stays out of git (it's in `.gitignore`): plan, reports and logs live only on your machine — manually copy whatever is useful into the PR.

## Day-to-day flow

End-to-end example: from a feature ticket to reviewed code ready for a PR.

**1. First time in the repo**

```
$ cd ~/code/payments-service && aic init
```

**2. Planning** — create the skeleton, discuss and write the proposal as usual (editor, or asking Claude for help outside `aic` — the design conversation isn't automated), then ask for the review:

```
$ aic plan new health-check --title "Health check endpoint"
$ aic plan review health-check
Round 1: approved=false — Missing a timeout check and the
alternatives/impact sections are empty.
```

Edit `plan.md` and run again until approved (or use `--mode auto --max-rounds 4` to let the loop fix it by itself).

**3. Execute the approved plan** — interactive mode lets you follow Claude working, like a normal session:

```
$ aic dev execute health-check
```

**4. Review what was implemented**:

```
$ git add -A
$ aic dev review health-check
Round 1: approved=false — Liveness and readiness need to be separated.
```

Fix it (manually or with `--mode auto`) and run again until approved. `aic` always re-evaluates the real diff after each fix — even if it wasn't committed — so the loop converges instead of repeating the same diff.

**5. Check the state** at any time:

```
$ aic status health-check
## health-check
  plan: approved (1 round(s))
  dev:  done (2 round(s))
```

Or, inside the feature branch, without passing the slug: `aic status` tries to infer it from the branch name.

**6. Ad-hoc review**, not tied to a plan (e.g. a quick fix):

```
$ git add -A && aic dev review
```

**7. Close the cycle**: with `dev` at `done`, continue with the normal flow — commit, push, PR.

## Architecture

```
src/
  main.rs            // clap parsing, dispatch to the commands
  cli.rs             // subcommand/flag definitions (clap derive)
  config.rs          // Config (toml, global + repo override), merge, defaults
  state.rs           // State/Feature/Round (serde), .aic/state.json
  agent.rs           // CliAgent — invokes claude/agy as a subprocess, normalizes the
                     // JSON response envelope (the two have different formats)
  review.rs          // generic run_review_loop() — used by both plan review
                     // and dev review
  git.rs             // diffs, default branch, current branch slug
  templates.rs       // plan.md template
  commands/          // init, plan, dev, status, config
```

`claude` and `agy` expose nearly identical headless flags (`-p`, `--output-format json`, `--json-schema`), but the response envelope and some flags (edit permission, initial prompt in interactive mode) differ — `agent.rs` and `commands/dev.rs` normalize those differences per binary, not per the logical agent name in the config.
