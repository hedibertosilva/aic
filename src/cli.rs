use clap::{Parser, Subcommand, ValueEnum};

use crate::config::Mode;

#[derive(Parser)]
#[command(
    name = "aic",
    about = "Orchestrates Claude Code and Gemini (Antigravity CLI) for planning and dev review"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Set up `.aic/` in this repo and the global config, if missing.
    Init,
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
    /// Run a local web chat that talks to the configured agents.
    Chat {
        /// Address to listen on. Anything but loopback exposes your agents
        /// to the network.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 7878)]
        port: u16,
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
        /// Feature slug (must already exist — run `aic plan new` first).
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
    /// With a <SLUG> that names a tracked feature (from `aic plan new`),
    /// rounds are recorded in that feature's history and saved under
    /// .aic/dev/<slug>/reviews/CODE-RN.md, same as `aic plan review`. If
    /// <SLUG> is omitted (or doesn't match a tracked feature — e.g. you're
    /// on `main`), this is an ad-hoc review instead: not tied to any
    /// feature, not recorded in state.json or `aic status`, saved under
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
    /// sandbox_bind_agents enabled, agy/Codex use host binaries instead.
    Build {
        /// Image tag to build. Defaults to the configured `sandbox_image`.
        tag: Option<String>,
        /// Version of @anthropic-ai/claude-code to install in the image.
        #[arg(long = "claude-version")]
        claude_version: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Print the effective configuration (global + repo override).
    Show,
    /// Set a single top-level key in the global config.
    ///
    /// Keys: executor, reviewer, mode (manual|auto), max_rounds,
    /// permission_mode (acceptEdits|plan), default_base_branch,
    /// skip_permissions (true|false), sandbox (docker|podman|none),
    /// sandbox_image, sandbox_bind_agents (true|false).
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
