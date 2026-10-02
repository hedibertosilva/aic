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

The [sandbox](#sandbox) mounts this same binary into its containers, so it has to run there too. A static build runs on any image:

```
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
cp target/x86_64-unknown-linux-musl/release/aic ~/.local/bin/aic
```

A regular (glibc) build also works as long as your build host's glibc isn't newer than the image's (Debian trixie, glibc 2.41).

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
sandbox = "docker"               # "docker" | "podman" | "none" — run agents in a container
sandbox_image = "aic-sandbox"    # image built by `aic sandbox build`
sandbox_mounts = []              # extra "host:container[:ro]" binds (e.g. cargo registry)
sandbox_env = []                 # host env var names to forward (e.g. "ANTHROPIC_API_KEY")

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

## Sandbox

By default every agent invocation (executor and reviewer, headless and interactive) runs in a Docker or Podman container, so an agent running unattended can only modify the mounted repo, not the rest of your `$HOME`. The runtime must be installed on the **host** where `aic` runs (not inside distrobox).

One-time setup, then it is transparent:

```
aic sandbox build [tag] [--claude-version X.Y.Z]
```

Or set `sandbox = "none"` (globally or in `.aic/config.toml`) to keep running agents directly on the host. If the image is missing, `aic` fails before starting any agent and tells you to build it.

- **Mounts:** the repo root at the same absolute path (read-write for the executor, read-only plus a tmpfs `/tmp` for the reviewer); the shared git dir when it lives outside the repo (worktrees); `~/.gitconfig` read-only. Nothing else from `$HOME` is visible. Add more with `sandbox_mounts`, and forward environment variables by name with `sandbox_env`.
- **Credentials:** `claude` runs with a container-owned state dir (`~/.local/share/aic/sandbox/claude`, as `CLAUDE_CONFIG_DIR`) with only `~/.claude/.credentials.json` bound in from the host; `agy` gets your `~/.gemini` mounted read-write.
- **`aic` inside:** the running `aic` binary is mounted read-only at `/usr/local/bin/aic` and your global config dir at `/etc/aic` (as `AIC_CONFIG_DIR`), so agents — and the `aic-*` skills — can call `aic` with the same agents, models and permission mode as on the host, with no image rebuild after upgrading `aic`. Containers get `AIC_IN_SANDBOX=1`, which makes that `aic` run agents directly instead of nesting another sandbox (`aic status` and `aic config show` say so); its reviewer then falls back to `plan` mode instead of a read-only mount. Agents it starts must exist in the image: `claude` always does, `agy` only if it was on your `PATH` at `aic sandbox build`, otherwise `aic` says so. Images built before this change need a rebuild (the base moved to Debian trixie).
- **Image upgrades:** the image's agent versions win over the host's inside the sandbox. `claude` is installed at a pinned version (`--claude-version` bumps it); `agy` is copied from the host `PATH` at build time. Re-run `aic sandbox build` to upgrade either.
- **SELinux:** containers run with `--security-opt label=disable` instead of relabeling (`:z`/`:Z`) your repo and home files, which has lasting side effects. The tradeoff is that SELinux does not confine these containers.
- **Threat model:** this limits the blast radius of *accidental* destructive commands on the filesystem. The network is open and agent credentials are mounted, so a malicious agent or prompt injection could still read and send them.

## Web chat

```
aic server [--host 127.0.0.1] [--port 7878]
```

Serves a local chat page where one conversation can have several participants — each a named instance of any agent under `[agents]` (e.g. two `claude`s with different roles, plus `gemini`) — and every message goes to them in parallel, with replies side by side, one fixed column per participant. The **To:** pills under the message box show who receives it (all by default; typing `@name` at the start picks them too). When a round is done, **Ask them to comment on each other's replies** under the replies lets them answer what the others said since their last turn, for debate or cross-review (so does sending an empty message). Conversations live in the browser's `localStorage`; the server is stateless and continues each participant by passing the agent's own session id back (`claude --resume`, `agy --conversation`).

- Runs in the repo you start it from, through the same sandbox and `skip_permissions` settings as the other commands.
- **Participants** are added and edited in a dialog (click a participant's name): name, agent, model and effort (default: the agent's config), colour, role, and file access. Renaming rewrites the conversation's log; changing the agent starts it without this conversation's history.
- **Roles** are sent as an appended system prompt on every turn for `claude`; `agy` has no such flag, so its role goes in the first message only.
- **Read-only by default:** participants can read the repo but not change it (read-only mount in the sandbox, plan mode without one) and are told so, instead of hunting for workarounds. Set *Can edit* on a participant to let it write, with your `permission_mode`; it then shows a pencil badge.
- While an agent works, the card shows what it is doing (`Read: src/agent.rs`, `Bash: Run tests`) and the elapsed time; afterwards a collapsible list keeps every action. The card footer shows time, duration, tokens and cost when the agent reports them, plus copy, regenerate and delete. Your last message can be edited and resent.
- **Stop** (the ■ button, or Esc) ends the replies and kills the agents, including anything they started. Enter never stops anything. You can switch conversations while replies come in.
- **Skills:** typing `/` lists the skills in the repo's `.claude/skills/` (plus `~/.claude/skills/` without a sandbox). A skill message must go to exactly one participant, which needs edit access if the skill writes files (`allowed-tools` with `Edit`/`Write`, or `aic-requires-write: true` in its frontmatter); `agy` participants can't run skills.
- The sidebar groups conversations by date and searches titles and contents; the ⋯ menu renames, exports (Markdown or JSON) and deletes. Ctrl+Shift+O starts a new conversation.
- Binds to loopback by default and rejects requests with a foreign `Host` or `Origin`. `--host 0.0.0.0` exposes your agents to anyone who can reach the port.
- `agy` currently reports its reply only at the end of a turn, so Gemini answers appear in one piece rather than token by token.

## Commands

| Command | What it does |
|---|---|
| `aic init` | Creates `.aic/` in the current repo (`plans/`, `dev/`, `adhoc/reviews/`, `state.json`) and the global config, if they don't exist. Adds `.aic/` to `.gitignore`. |
| `aic plan new <slug> [--title "..."] [--no-open]` | Creates `.aic/plans/<slug>/PLAN.md` from a template (Problem, Proposal, Alternatives, Impact, Open questions) and registers the feature. Then opens the executor (claude/gemini) interactively to start the discussion; `--no-open` skips that. |
| `aic plan review <slug>` | Sends `PLAN.md` to the reviewer; saves the report to `.aic/plans/<slug>/reviews/PLAN-RN.md`. `auto` mode fixes and re-reviews until approved. |
| `aic dev execute <slug> [--mode interactive\|headless]` | Runs the executor against the approved plan. `interactive` (default) takes over the terminal (`exec`), like a normal session; `headless` runs via `-p` and logs to `.aic/dev/<slug>/EXECUTION-LOG.md`. |
| `aic dev review [<slug>]` | Reviews a diff: staged, commits since `dev execute`, or against the base branch — chosen automatically, or via `--target`. With `<slug>`, saves to `.aic/dev/<slug>/reviews/CODE-RN.md`. Without `<slug>`, it's an ad-hoc review saved under `.aic/adhoc/reviews/`. |
| `aic status [<slug>]` | Shows the state (plan/dev, rounds) of one feature or all of them. With no argument, tries to infer the feature from the current branch name. |
| `aic sandbox build [tag]` | Builds the container image agents run in (see [Sandbox](#sandbox)). |
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
