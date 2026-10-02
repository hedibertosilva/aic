mod agent;
mod cli;
mod commands;
mod config;
mod diff_filter;
mod git;
mod review;
mod sandbox;
mod state;
mod templates;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

use cli::{Cli, Commands, ConfigCommands, DevCommands, PlanCommands, SandboxCommands};

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            let repo_root = resolve_repo_root()?;
            commands::init::run(&repo_root)
        }

        Commands::Plan { command } => match command {
            PlanCommands::New {
                slug,
                title,
                no_open,
            } => {
                let repo_root = resolve_repo_root()?;
                commands::plan::new(&repo_root, &slug, title, no_open)
            }
            PlanCommands::Review {
                slug,
                reviewer,
                executor,
                mode,
                max_rounds,
            } => {
                let repo_root = resolve_repo_root()?;
                commands::plan::review(
                    &repo_root,
                    &slug,
                    commands::plan::ReviewOverrides {
                        reviewer,
                        executor,
                        mode: mode.map(Into::into),
                        max_rounds,
                    },
                )
            }
        },

        Commands::Dev { command } => match command {
            DevCommands::Execute {
                slug,
                mode,
                executor,
                permission_mode,
            } => {
                let repo_root = resolve_repo_root()?;
                let slug = resolve_slug(&repo_root, slug)?;
                commands::dev::execute(
                    &repo_root,
                    &slug,
                    commands::dev::ExecuteOverrides {
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
                        anyhow::bail!("no feature '{slug}' found; run `aic plan new {slug}` first")
                    }
                    _ => None,
                };
                commands::dev::review(
                    &repo_root,
                    slug.as_deref(),
                    commands::dev::ReviewOverrides {
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
        },

        Commands::Status { slug } => {
            let repo_root = resolve_repo_root()?;
            commands::status::run(&repo_root, slug)
        }

        Commands::Sandbox { command } => match command {
            SandboxCommands::Build { tag, claude_version } => {
                let repo_root = resolve_repo_root()
                    .or_else(|_| std::env::current_dir().context("reading current directory"))?;
                let config = config::Config::load_merged(&repo_root)?;
                commands::sandbox::build(&config, tag, claude_version)
            }
        },

        Commands::Config { command } => match command {
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
