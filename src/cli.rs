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
    Status { slug: Option<String> },
    /// View or edit aic's configuration (defaults for every other command).
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
}

#[derive(Subcommand)]
pub enum PlanCommands {
    /// Scaffold a new plan.md for a feature.
    New {
        slug: String,
        #[arg(long)]
        title: Option<String>,
    },
    /// Ask the reviewer agent to review plan.md.
    Review {
        slug: String,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        executor: Option<String>,
        #[arg(long)]
        mode: Option<ModeArg>,
        #[arg(long = "max-rounds")]
        max_rounds: Option<u32>,
    },
}

#[derive(Subcommand)]
pub enum DevCommands {
    /// Run the executor agent against the plan.
    Execute {
        slug: Option<String>,
        #[arg(long, value_enum)]
        mode: Option<ExecuteModeArg>,
        #[arg(long)]
        executor: Option<String>,
        #[arg(long = "permission-mode")]
        permission_mode: Option<String>,
    },
    /// Ask the reviewer agent to review a diff (staged, recent commits, or against the base branch).
    Review {
        slug: Option<String>,
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
        #[arg(long)]
        range: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        executor: Option<String>,
        #[arg(long)]
        mode: Option<ModeArg>,
        #[arg(long = "max-rounds")]
        max_rounds: Option<u32>,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Print the effective configuration (global + repo override).
    Show,
    /// Set a single top-level key in the global config.
    Set { key: String, value: String },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum ModeArg {
    Manual,
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
    Interactive,
    Headless,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum TargetArg {
    Staged,
    Commits,
    Branch,
}
