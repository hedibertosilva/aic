use clap::error::ErrorKind;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};

use crate::config::Mode;

#[derive(Parser)]
#[command(
    name = "aic",
    about = "Local web chat for Claude Code, Gemini (Antigravity CLI) and Codex, plus a plan/dev review workflow. Without a subcommand, starts the chat."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

impl Cli {
    /// Like `Cli::parse()`, except that a missing positional argument or
    /// subcommand prints the help of the command being run instead of just
    /// an error, so the user sees what to pass.
    pub fn parse_args() -> Self {
        let mut root = Cli::command();
        root.build();
        let err = match root.try_get_matches_from_mut(std::env::args_os()) {
            Ok(matches) => return Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit()),
            Err(err) => err,
        };
        if !matches!(
            err.kind(),
            ErrorKind::MissingRequiredArgument
                | ErrorKind::MissingSubcommand
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        ) {
            err.exit();
        }
        // The deepest subcommand named on the command line (skipping flags).
        let mut current = &mut root;
        for arg in std::env::args().skip(1).filter(|a| !a.starts_with('-')) {
            if current.find_subcommand(&arg).is_some() {
                current = current.find_subcommand_mut(&arg).expect("checked above");
            }
        }
        let _ = current.write_help(&mut std::io::stderr());
        std::process::exit(2);
    }
}

/// Defaults of `aic chat`, also used when no subcommand is given.
pub const DEFAULT_CHAT_HOST: &str = "127.0.0.1";
pub const DEFAULT_CHAT_PORT: u16 = 7878;

#[derive(Subcommand)]
pub enum Commands {
    /// Set up `.aic/` in this repo and the global config, if missing.
    Init,
    /// Run a local web chat that talks to the configured agents (the default).
    ///
    /// Runs `aic init` first, so `.aic/` and the config exist.
    Chat {
        /// Address to listen on. Anything but loopback exposes your agents
        /// (and their mounted credentials) to the network.
        #[arg(long, default_value = DEFAULT_CHAT_HOST)]
        host: String,
        #[arg(long, default_value_t = DEFAULT_CHAT_PORT)]
        port: u16,
    },
    /// Plan, execute and review a feature with two agents.
    #[command(visible_alias = "wf")]
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommands,
    },
    /// List the configured agents, check their quota and log them in.
    #[command(alias = "agents")]
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Manage the container sandbox agents run in.
    Sandbox {
        #[command(subcommand)]
        command: SandboxCommands,
    },
    /// View or edit aic's configuration (defaults for every other command).
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
    /// Alias of `aic workflow plan`.
    #[command(hide = true)]
    Plan {
        #[command(subcommand)]
        command: PlanCommands,
    },
    /// Alias of `aic workflow dev`.
    #[command(hide = true)]
    Dev {
        #[command(subcommand)]
        command: DevCommands,
    },
    /// Alias of `aic workflow status`.
    #[command(hide = true)]
    Status { slug: Option<String> },
}

#[derive(Subcommand)]
pub enum WorkflowCommands {
    /// Manage planning proposals (the `.md` discussed before building anything).
    Plan {
        #[command(subcommand)]
        command: PlanCommands,
    },
    /// Execute an approved plan and review the resulting code changes.
    Dev {
        #[command(subcommand)]
        command: DevCommands,
    },
    /// Show plan/dev status for one feature, or all of them.
    Status {
        /// Feature slug. If omitted, inferred from the current branch name;
        /// if that doesn't match a tracked feature either, lists all of
        /// them instead of erroring.
        slug: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum PlanCommands {
    /// Scaffold a new PLAN.md for a feature.
    New {
        /// Feature slug — also the directory name under .aic/plans/.
        slug: String,
        /// Title used as the PLAN.md heading. Defaults to the slug.
        #[arg(long)]
        title: Option<String>,
        /// Don't open the executor agent interactively after creating the plan.
        #[arg(long = "no-open")]
        no_open: bool,
    },
    /// Ask the reviewer agent to review PLAN.md.
    Review {
        /// Feature slug (must already exist — run `aic workflow plan new` first).
        slug: String,
        /// Agent that reviews the plan. Must be a key under [agents] in the
        /// config. Defaults to the configured `reviewer` (normally gemini).
        #[arg(long)]
        reviewer: Option<String>,
        /// Agent that revises the plan when `--mode auto` requests changes.
        /// Defaults to the configured `executor` (normally claude).
        #[arg(long)]
        executor: Option<String>,
        /// manual: run one review round and stop, no matter the verdict.
        /// auto: if not approved, have the executor fix the plan and
        /// re-review, up to --max-rounds times. Defaults to the configured
        /// `mode`.
        #[arg(long)]
        mode: Option<ModeArg>,
        /// Cap on review rounds in --mode auto (ignored in manual mode,
        /// which is always exactly one round). Defaults to the configured
        /// `max_rounds`.
        #[arg(long = "max-rounds")]
        max_rounds: Option<u32>,
    },
}

#[derive(Subcommand)]
pub enum DevCommands {
    /// Run the executor agent against the plan.
    Execute {
        /// Feature slug (must have an approved plan). If omitted, inferred
        /// from the current branch name.
        slug: Option<String>,
        /// interactive (default): hand the terminal to the executor
        /// (`exec`), like a normal chat session — aic doesn't run anything
        /// after this. headless: run non-interactively via `-p` and log the
        /// result to .aic/dev/<slug>/EXECUTION-LOG.md.
        #[arg(long, value_enum)]
        mode: Option<ExecuteModeArg>,
        /// Agent that executes the plan. Must be a key under [agents] in
        /// the config. Defaults to the configured `executor`.
        #[arg(long)]
        executor: Option<String>,
        /// acceptEdits (default): the executor can edit files/run commands
        /// without asking. plan: the executor only proposes, without
        /// touching anything. Defaults to the configured `permission_mode`.
        #[arg(long = "permission-mode")]
        permission_mode: Option<String>,
    },
    /// Ask the reviewer agent to review a diff (staged, recent commits, or against the base branch).
    ///
    /// With a <SLUG> that names a tracked feature (from `aic workflow plan new`),
    /// rounds are recorded in that feature's history and saved under
    /// .aic/dev/<slug>/reviews/CODE-RN.md, same as `aic workflow plan review`. If
    /// <SLUG> is omitted (or doesn't match a tracked feature — e.g. you're
    /// on `main`), this is an ad-hoc review instead: not tied to any
    /// feature, not recorded in state.json or `aic workflow status`, saved under
    /// .aic/adhoc/reviews/<timestamp>-RN.md. Use a tracked slug when
    /// following the plan -> execute -> review flow for a feature; leave it
    /// off for a one-off "review whatever's staged right now".
    Review {
        /// Feature slug for a tracked review; omit for an ad-hoc one (see
        /// above). If given but not a tracked feature, this errors instead
        /// of silently going ad-hoc.
        slug: Option<String>,
        /// Which diff to review. staged: `git diff --staged`. commits:
        /// everything since `dev execute`'s recorded start commit (or
        /// --range). branch: diff against the auto-detected default
        /// branch. If omitted: staged if there are staged changes,
        /// otherwise commits if a start commit is on record, otherwise
        /// branch.
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
        /// Explicit `<from>..<to>` commit range for `--target commits`,
        /// instead of the feature's recorded start commit.
        #[arg(long)]
        range: Option<String>,
        /// Agent that reviews the diff. Must be a key under [agents] in the
        /// config. Defaults to the configured `reviewer`.
        #[arg(long)]
        reviewer: Option<String>,
        /// Agent that fixes the code when `--mode auto` requests changes.
        /// Defaults to the configured `executor`.
        #[arg(long)]
        executor: Option<String>,
        /// manual: run one review round and stop, no matter the verdict.
        /// auto: if not approved, have the executor fix the code and
        /// re-review, up to --max-rounds times. Defaults to the configured
        /// `mode`.
        #[arg(long)]
        mode: Option<ModeArg>,
        /// Cap on review rounds in --mode auto (ignored in manual mode,
        /// which is always exactly one round). Defaults to the configured
        /// `max_rounds`.
        #[arg(long = "max-rounds")]
        max_rounds: Option<u32>,
        /// Review the full, unfiltered diff: skip the built-in lockfile
        /// list, `.gitignore`/`.gitattributes` (linguist-generated) and the
        /// git_diff_*_patterns config, for this run only.
        #[arg(long = "no-filter")]
        no_filter: bool,
    },
}

#[derive(Subcommand)]
pub enum SandboxCommands {
    /// Build the sandbox image from the Dockerfile embedded in aic.
    ///
    /// Also the upgrade path: the image's agent versions win over the
    /// host's inside the sandbox, so rebuild to pick up newer ones. With
    /// [sandbox] bind_agents enabled, agy/Codex use host binaries instead.
    Build {
        /// Image tag to build. Defaults to the configured `[sandbox] image`.
        tag: Option<String>,
        /// Version of @anthropic-ai/claude-code to install in the image.
        #[arg(long = "claude-version")]
        claude_version: Option<String>,
    },
    /// Alias of `aic agent login`.
    #[command(hide = true)]
    Login { agent: String },
}

#[derive(Subcommand)]
pub enum AgentCommands {
    /// List the configured agents (binary, model, effort, default roles).
    List,
    /// Read the account quota of each agent (or one) through its CLI.
    ///
    /// Takes a few seconds per agent. A failure usually means the agent isn't
    /// logged in: see `aic agent login`.
    Status {
        /// An agent from the config ([agents] key). Defaults to all of them.
        agent: Option<String>,
    },
    /// Open an agent's own CLI so you can log in (once; the login persists).
    ///
    /// Always the vendor's CLI, so it's clear you're signing in to it and
    /// not to aic. `gemini` (the `agy` binary) opens inside the sandbox: it
    /// keeps the host login in the keyring, which the container can't reach,
    /// so the sandbox has its own under ~/.local/share/aic/sandbox/agy (host
    /// network, so the OAuth callback works). `claude` and `codex` open on
    /// the host, because the sandbox reuses the host's login files.
    Login {
        /// An agent from the config ([agents] key) or binary: gemini (agy),
        /// claude or codex.
        agent: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Print the effective configuration (global + repo override).
    Show,
    /// Set a single key in the global config, as `section.key`.
    ///
    /// Keys: workflow.executor, workflow.reviewer, workflow.mode
    /// (manual|auto), workflow.max_rounds, workflow.default_base_branch,
    /// permissions.mode (acceptEdits|plan), permissions.skip (true|false),
    /// sandbox.runtime (docker|podman|none), sandbox.image,
    /// sandbox.bind_agents (true|false), chat.default_agent. The old flat
    /// names (mode, sandbox_image, ...) still work.
    Set {
        /// Config key to set (see above for the full list).
        key: String,
        /// New value for that key.
        value: String,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum ModeArg {
    /// Run one review round and stop, no matter the verdict.
    Manual,
    /// If not approved, let the executor fix it and re-review automatically
    /// up to --max-rounds times.
    Auto,
}

impl From<ModeArg> for Mode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Manual => Mode::Manual,
            ModeArg::Auto => Mode::Auto,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum ExecuteModeArg {
    /// Hand the terminal to the executor directly (exec) — like a normal
    /// chat session.
    Interactive,
    /// Run non-interactively (-p) and log the result to a file.
    Headless,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum TargetArg {
    /// `git diff --staged`.
    Staged,
    /// Everything since `dev execute`'s recorded start commit (or --range).
    Commits,
    /// Diff against the auto-detected default branch.
    Branch,
}
