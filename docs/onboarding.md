# Setup and recovery

This guide explains first use of aic from a published binary. Rust and manual
TOML editing are not required.

## Prepare the environment

Execute dentro de um checkout Git:

```sh
aic setup
```

The assistant has five stages: environment, sandbox, agents, default agent and
conclusion. You can go back, press `Esc` or choose **Finish later**; completed
settings remain available when you run `aic setup` again.

Setup does not install third-party CLIs. For each agent, resolve the reported
diagnosis and repeat the test:

```sh
aic doctor
aic agent login claude       # also: gemini, codex or agy
aic agent test claude
```

The availability test makes a minimal real call and may consume quota. It asks
for confirmation; use `--yes` deliberately in automation:

```sh
aic agent test claude --yes --timeout 45
aic setup --agent claude --yes --no-open
```

Only one ready agent is needed to start. A quota failure does not prove that
login expired: wait for the reset or choose another ready agent.

## Sandbox

The default runtime is Docker and the default image is `aic-sandbox`:

```sh
aic sandbox build
```

Podman is also supported. If you do not want isolation, choose `none` in setup
or run `aic config set sandbox.runtime none`. Agents then run directly on host.

## Start and validate the chat

```sh
aic chat
# sem abrir o navegador:
aic chat --no-open
```

The **Environment** panel shows the checkout, branch, runtime, image and state
of each agent. It does not run logins or paid tests; do that in the terminal,
then use **Refresh environment**.

In a new conversation, use **Add participant**, choose a ready agent and only
then send the first message. An unavailable agent is never silently replaced.

## Quick diagnosis

| Symptom | Action |
| --- | --- |
| Missing runtime | Install/start Docker or Podman, or choose `none`. |
| Missing image | Run `aic sandbox build` and resume `aic setup`. |
| CLI not found | Install the provider CLI and run `aic doctor`. |
| Not authenticated | Run `aic agent login NAME`, then `aic agent test NAME`. |
| Execution failure | Repeat `aic agent test NAME`; check the CLI itself. |
| Quota exhausted | Wait for reset or use another ready agent. |
| Chat does not open | Use `aic chat --no-open` and open the printed URL. |

`aic doctor --json` produces a diagnostic for automation. Tests are stored in
`.aic/diagnostics/tests.json`, expire after 24 hours and do not store
credentials, responses or provider stderr.

## Security and local data

The chat listens on `127.0.0.1:7878` by default and has no authentication.
Exposing it with `--host 0.0.0.0` lets anyone who can reach the port operate
the agents. The sandbox reduces the reach of accidental commands, but network
access and required credentials remain available to the agent.

- `~/.config/aic/`: global configuration.
- `.aic/`: plans, chat history, state and checkout diagnostics.
- `~/.local/share/aic/sandbox/`: persistent state for sandboxed agents.

`.aic/` is added to `.gitignore` by `aic init`. To update aic, replace the
binary and run setup again. To uninstall, remove only the binary; delete the
configuration and `.aic/` only if you also want to remove local history and
plans.
