mod chat;
mod cli;
mod commands;
mod common;
mod workflow;

// Modules keep resolving as `crate::agent`, `crate::state`, ... from anywhere.
use chat::{consumption, quotas};
use common::{agent, config, git, sandbox, templates};
use workflow::{diff_filter, review, state};

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use cli::{
    AgentCommands, Cli, Commands, ConfigCommands, DevCommands, PlanCommands, SandboxCommands,
    WorkflowCommands,
    DEFAULT_CHAT_HOST, DEFAULT_CHAT_PORT,
};

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse_args();

    match cli.command {
        None => start_chat(DEFAULT_CHAT_HOST, DEFAULT_CHAT_PORT),

        Some(Commands::Init) => {
            let repo_root = resolve_repo_root()?;
            commands::init::run(&repo_root)
        }

        Some(Commands::Chat { host, port }) => start_chat(&host, port),

        Some(Commands::Workflow { command }) => match command {
            WorkflowCommands::Plan { command } => plan(command),
            WorkflowCommands::Dev { command } => dev(command),
            WorkflowCommands::Status { slug } => status(slug),
        },

        // Hidden aliases of the `workflow` subcommands.
        Some(Commands::Plan { command }) => plan(command),
        Some(Commands::Dev { command }) => dev(command),
        Some(Commands::Status { slug }) => status(slug),

        Some(Commands::Sandbox { command }) => match command {
            SandboxCommands::Build { tag, claude_version } => {
                let repo_root = resolve_repo_root()
                    .or_else(|_| std::env::current_dir().context("reading current directory"))?;
                let config = config::Config::load_merged(&repo_root)?;
                commands::sandbox::build(&config, tag, claude_version)
            }
            SandboxCommands::Login { agent } => agent_command(AgentCommands::Login { agent }),
        },

        Some(Commands::Agent { command }) => agent_command(command),

        Some(Commands::Config { command }) => match command {
            None => commands::config_cmd::interactive(),
            Some(ConfigCommands::Show) => {
                // Falls back to the current directory outside a git repo —
                // `config show` should still work, just without a repo
                // override to merge in.
                let repo_root = resolve_repo_root()
                    .or_else(|_| std::env::current_dir().context("reading current directory"))?;
                commands::config_cmd::show(&repo_root)
            }
            Some(ConfigCommands::Set { key, value }) => commands::config_cmd::set(&key, &value),
        },
    }
}

fn plan(command: PlanCommands) -> Result<()> {
    match command {
        PlanCommands::New {
            slug,
            title,
            no_open,
        } => {
            let repo_root = resolve_repo_root()?;
            workflow::plan::new(&repo_root, &slug, title, no_open)
        }
        PlanCommands::Review {
            slug,
            reviewer,
            executor,
            mode,
            max_rounds,
        } => {
            let repo_root = resolve_repo_root()?;
            workflow::plan::review(
                &repo_root,
                &slug,
                workflow::plan::ReviewOverrides {
                    reviewer,
                    executor,
                    mode: mode.map(Into::into),
                    max_rounds,
                },
            )
        }
    }
}

fn dev(command: DevCommands) -> Result<()> {
    match command {
        DevCommands::Execute {
            slug,
            mode,
            executor,
            permission_mode,
        } => {
            let repo_root = resolve_repo_root()?;
            let slug = resolve_slug(&repo_root, slug)?;
            workflow::dev::execute(
                &repo_root,
                &slug,
                workflow::dev::ExecuteOverrides {
                    mode,
                    executor,
                    permission_mode,
                },
            )
        }
        DevCommands::Review {
            slug,
            target,
            range,
            reviewer,
            executor,
            mode,
            max_rounds,
            no_filter,
        } => {
            let repo_root = resolve_repo_root()?;
            let explicit = slug.is_some();
            // Only use the branch-inferred slug if it actually names a
            // tracked feature — otherwise this is an ad-hoc review
            // (e.g. on `main`, which is never itself a feature). An
            // *explicit* slug that isn't tracked is a mistake (likely a
            // typo), not an invitation to go ad-hoc, so that errors
            // instead.
            let slug = slug.or_else(|| git::current_branch_slug(&repo_root).ok());
            let slug = match slug {
                Some(slug) if state::State::load(&repo_root)?.feature(&slug).is_some() => Some(slug),
                Some(slug) if explicit => {
                    anyhow::bail!("no feature '{slug}' found; run `aic workflow plan new {slug}` first")
                }
                _ => None,
            };
            workflow::dev::review(
                &repo_root,
                slug.as_deref(),
                workflow::dev::ReviewOverrides {
                    target,
                    range,
                    reviewer,
                    executor,
                    mode: mode.map(Into::into),
                    max_rounds,
                    no_filter,
                },
            )
        }
    }
}

fn agent_command(command: AgentCommands) -> Result<()> {
    // Works outside a git repo too: falls back to the current directory.
    let repo_root = resolve_repo_root()
        .or_else(|_| std::env::current_dir().context("reading current directory"))?;
    let config = config::Config::load_merged(&repo_root)?;
    match command {
        AgentCommands::List => commands::agent::list(&config),
        AgentCommands::Status { agent } => commands::agent::status(&config, &repo_root, agent),
        AgentCommands::Login { agent } => commands::agent::login(&config, &repo_root, &agent),
    }
}

fn status(slug: Option<String>) -> Result<()> {
    let repo_root = resolve_repo_root()?;
    workflow::status::run(&repo_root, slug)
}

/// Starts the chat from the repo it was launched in, setting up `.aic/` and
/// the global config first (idempotent).
fn start_chat(host: &str, port: u16) -> Result<()> {
    let repo_root = resolve_repo_root()?;
    commands::init::run(&repo_root)?;
    chat::server::run(&repo_root, host, port)
}

fn resolve_repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    git::repo_root(&cwd).context("not inside a git repository")
}

fn resolve_slug(repo_root: &Path, slug: Option<String>) -> Result<String> {
    if let Some(slug) = slug {
        return Ok(slug);
    }
    git::current_branch_slug(repo_root)
        .context("couldn't infer a feature slug from the current branch; pass one explicitly")
}
