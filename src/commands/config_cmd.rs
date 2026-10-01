use std::path::Path;

use anyhow::{bail, Context, Result};
use dialoguer::{Input, Select};

use crate::config::{Config, Mode};

/// Prints the *effective* config: global, with the current repo's
/// `.aic/config.toml` override (if any) applied on top — matching what
/// every other command actually resolves and uses.
pub fn show(repo_root: &Path) -> Result<()> {
    let config = Config::load_merged(repo_root)?;
    println!("{}", toml::to_string_pretty(&config)?);
    Ok(())
}

pub fn set(key: &str, value: &str) -> Result<()> {
    let mut config = Config::load_global()?;
    match key {
        "executor" => config.executor = value.to_string(),
        "reviewer" => config.reviewer = value.to_string(),
        "mode" => {
            config.mode = match value {
                "manual" => Mode::Manual,
                "auto" => Mode::Auto,
                other => bail!("unknown mode '{other}', expected 'manual' or 'auto'"),
            };
        }
        "max_rounds" => {
            config.max_rounds = value
                .parse()
                .with_context(|| format!("'{value}' isn't a valid number"))?;
        }
        "permission_mode" => config.permission_mode = value.to_string(),
        "default_base_branch" => config.default_base_branch = value.to_string(),
        "skip_permissions" => {
            config.skip_permissions = match value {
                "true" => true,
                "false" => false,
                other => bail!("'{other}' isn't 'true' or 'false'"),
            };
        }
        other => bail!("unknown config key '{other}' (executor, reviewer, mode, max_rounds, permission_mode, default_base_branch, skip_permissions)"),
    }
    config.save_global()?;
    println!("Set {key} = {value}");
    Ok(())
}

/// A `/config`-style menu: pick a key, type a new value, repeat until you
/// choose to save or discard.
pub fn interactive() -> Result<()> {
    let mut config = Config::load_global()?;

    loop {
        let items = vec![
            format!("executor = {}", config.executor),
            format!("reviewer = {}", config.reviewer),
            format!("mode = {}", mode_label(config.mode)),
            format!("max_rounds = {}", config.max_rounds),
            format!("permission_mode = {}", config.permission_mode),
            format!(
                "default_base_branch = {}",
                if config.default_base_branch.is_empty() {
                    "(auto-detect)"
                } else {
                    &config.default_base_branch
                }
            ),
            format!(
                "skip_permissions = {} (opt-in: auto-approves every tool request in headless runs)",
                config.skip_permissions
            ),
            "Save and exit".to_string(),
            "Exit without saving".to_string(),
        ];

        let selection = Select::new()
            .with_prompt("aic config — pick a key to change")
            .items(&items)
            .default(0)
            .interact()?;

        match selection {
            0 => {
                config.executor = Input::new()
                    .with_prompt("executor")
                    .default(config.executor.clone())
                    .interact_text()?;
            }
            1 => {
                config.reviewer = Input::new()
                    .with_prompt("reviewer")
                    .default(config.reviewer.clone())
                    .interact_text()?;
            }
            2 => {
                let mode_items = ["manual", "auto"];
                let current = if config.mode == Mode::Manual { 0 } else { 1 };
                let picked = Select::new()
                    .with_prompt("mode")
                    .items(&mode_items)
                    .default(current)
                    .interact()?;
                config.mode = if picked == 0 { Mode::Manual } else { Mode::Auto };
            }
            3 => {
                let value: String = Input::new()
                    .with_prompt("max_rounds")
                    .default(config.max_rounds.to_string())
                    .interact_text()?;
                config.max_rounds = value.parse().unwrap_or(config.max_rounds);
            }
            4 => {
                config.permission_mode = Input::new()
                    .with_prompt("permission_mode (acceptEdits|plan)")
                    .default(config.permission_mode.clone())
                    .interact_text()?;
            }
            5 => {
                config.default_base_branch = Input::new()
                    .with_prompt("default_base_branch (empty = auto-detect)")
                    .default(config.default_base_branch.clone())
                    .allow_empty(true)
                    .interact_text()?;
            }
            6 => {
                config.skip_permissions = !config.skip_permissions;
            }
            7 => {
                config.save_global()?;
                println!("Saved.");
                break;
            }
            _ => {
                println!("Discarded changes.");
                break;
            }
        }
    }

    Ok(())
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Manual => "manual",
        Mode::Auto => "auto",
    }
}
