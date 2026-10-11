# aic

The `aic` is a local environment for working with Claude Code, Gemini (through
the Antigravity CLI, `agy`) and Codex in the same repository. The product
centers on a local web chat for bringing agents together, comparing answers,
creating plans and tracking implementation in one conversation.

Everything goes through the providers' CLIs. aic does not create an intermediary
account or send code to its own service; conversations, plans and state stay in
the local checkout.

## Why use aic

- Talk to one or several agents in the same screen and compare perspectives.
- Give each participant a clear role: implementer, reviewer, researcher or
  domain specialist.
- Start in read-only mode and enable editing only where it is needed.
- Keep plans, reviews, tasks and history in the checkout itself.
- Run agents in Docker or Podman to reduce the reach of accidental changes
  outside the repository.
- Reuse sessions, export conversations and resume work without losing visible
  context.

## Requirements

- Linux x86_64 or arm64 for published binaries.
- Git and network access.
- At least one installed and authenticated agent CLI: `claude`, `agy` or
  `codex`.
- Docker or Podman for the default sandbox. You can choose `none`, but agents
  will then run directly on the host.

Rust is only required to build from source. macOS, Windows and other
architectures are not part of the current release pipeline.

## Installation

The installer detects Linux x86_64/arm64, downloads the latest release, verifies
its SHA-256 checksum and installs it in `~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/hedibertosilva/aic/main/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
```

To install a specific version, pass it to the script:

```sh
curl -fsSL https://raw.githubusercontent.com/hedibertosilva/aic/main/install.sh | sh -s -- 0.1.0
```

Set a different destination with `AIC_INSTALL_DIR`:

```sh
curl -fsSL https://raw.githubusercontent.com/hedibertosilva/aic/main/install.sh |
  AIC_INSTALL_DIR="$HOME/.local/bin" sh
```

For a manual build, see [building from source](docs/build-from-source.md).

## First repository

Run setup inside the checkout you want to use:

```sh
cd /caminho/do/seu/repositorio
aic setup
aic
```

Setup checks the environment, prepares the sandbox, finds the CLIs, guides
login and runs a minimal real test before allowing you to choose the default
agent. This test may consume quota. You can interrupt and resume it with
`aic setup`.

To run the steps manually:

```sh
aic init
aic sandbox build
aic agent login claude       # ou: gemini, codex
aic agent test claude
aic
```

Running `aic` without a subcommand starts the chat and runs `aic init`
automatically. The server listens on `127.0.0.1:7878` by default. Use
`aic chat --no-open` on a machine without a browser and open the printed URL
manually. Do not use `--host 0.0.0.0` without understanding that anyone who
can reach the port can operate the agents and mounted credentials.

## Recommended flow for new work

1. Open the correct repository and confirm the branch in **Git context**.
2. Create a conversation and choose a ready agent in **Add participant**.
3. Start with a read-only question: ask for diagnosis, alternatives and risks.
4. Add participants for a second opinion and give each one a role.
5. Keep analysis read-only. Enable **Can edit** only for the implementer and
   describe the expected result.
6. For a larger change, use **Link or create plan**, record the objective and
   acceptance criteria, and discuss them before executing tasks.
7. Review the diff, run tests and validate each task. An agent response does
   not replace human validation.
8. Export the conversation or leave the plan and records in `.aic/` to resume.

## Participants and skills

An agent is a configured integration (`claude`, `gemini` or `codex`). A
participant is an instance of that integration in a conversation, with a name,
model, effort, role and file access. A conversation supports up to five
participants.

- Use clear names and roles, such as `Implementer` and `Reviewer`.
- Select recipients in **To:** or use `@name` at the start of a message.
- Send the same question to several participants to compare answers, then use
  **Ask them to comment on each other's replies**.
- Use `/fresh` to restart selected sessions without replaying history;
  `/clear` clears the conversation; `/fork` creates an alternative branch.
- Skills appear when you type `/`. Send a skill to exactly one participant.
  That participant must support skills and have editing enabled when the skill
  writes files; `agy` cannot run skills.
- Check a skill's arguments, role and resulting diff before accepting changes.

## Essential configuration

`aic config` opens the interactive menu. Global configuration is stored in
`~/.config/aic/config.toml`; a repository `.aic/config.toml` can override
any part of it.

```sh
aic config show
aic config set chat.default_agent claude
aic config set sandbox.runtime docker     # docker, podman ou none
```

The sandbox mounts the checkout for the agent, keeps reviewers read-only and
limits access to the rest of the host. Network access and required credentials
remain available; review external prompts and files before running agents with
write permission.

## Useful commands

| Comando | Uso |
| --- | --- |
| `aic setup` | Prepare the environment, agents and default agent. |
| `aic doctor` | Diagnose without generating a provider response. |
| `aic agent list` | List configured CLIs, models and roles. |
| `aic agent status` | Read quota information reported by providers. |
| `aic init` | Create the local aic structure in the checkout. |
| `aic chat` | Start the local web chat. |
| `aic config ...` | View or change defaults. |
| `aic sandbox build` | Create or update the sandbox image. |

## Documentation

The [documentation index](docs/README.md) separates user guides from contracts
and implementation documents. Start with [setup and recovery](docs/onboarding.md),
[worktrees](docs/worktrees.md) and [plans in chat](docs/chat-plans.md).

To develop aic itself, see [building from source](docs/build-from-source.md).
