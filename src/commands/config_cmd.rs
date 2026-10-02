use std::path::Path;

use anyhow::{bail, Context, Result};
use console::{Key, Term};
use dialoguer::Select;

use crate::config::{Config, Mode, SandboxKind};

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
        "sandbox" => {
            config.sandbox = SandboxKind::parse(value)
                .with_context(|| format!("unknown sandbox '{value}', expected 'docker', 'podman' or 'none'"))?;
        }
        "sandbox_image" => config.sandbox_image = value.to_string(),
        other => bail!("unknown config key '{other}' (executor, reviewer, mode, max_rounds, permission_mode, default_base_branch, skip_permissions, sandbox, sandbox_image)"),
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
            format!("sandbox = {} (docker | podman | none)", config.sandbox.label()),
            format!("sandbox_image = {}", config.sandbox_image),
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
                if let Some(v) = prompt_text("executor", &config.executor, false)? {
                    config.executor = v;
                }
            }
            1 => {
                if let Some(v) = prompt_text("reviewer", &config.reviewer, false)? {
                    config.reviewer = v;
                }
            }
            2 => {
                let mode_items = ["manual", "auto"];
                let current = if config.mode == Mode::Manual { 0 } else { 1 };
                let picked = Select::new()
                    .with_prompt("mode (Esc to go back)")
                    .items(&mode_items)
                    .default(current)
                    .interact_opt()?;
                if let Some(picked) = picked {
                    config.mode = if picked == 0 { Mode::Manual } else { Mode::Auto };
                }
            }
            3 => {
                if let Some(value) = prompt_text("max_rounds", &config.max_rounds.to_string(), false)? {
                    config.max_rounds = value.parse().unwrap_or(config.max_rounds);
                }
            }
            4 => {
                if let Some(v) = prompt_text("permission_mode (acceptEdits|plan)", &config.permission_mode, false)? {
                    config.permission_mode = v;
                }
            }
            5 => {
                if let Some(v) = prompt_text("default_base_branch (empty = auto-detect)", &config.default_base_branch, true)? {
                    config.default_base_branch = v;
                }
            }
            6 => {
                config.skip_permissions = !config.skip_permissions;
            }
            7 => {
                let kinds = ["docker", "podman", "none"];
                let current = kinds.iter().position(|k| *k == config.sandbox.label()).unwrap_or(0);
                let picked = Select::new()
                    .with_prompt("sandbox (Esc to go back)")
                    .items(&kinds)
                    .default(current)
                    .interact_opt()?;
                if let Some(picked) = picked {
                    config.sandbox = SandboxKind::parse(kinds[picked]).unwrap_or(config.sandbox);
                }
            }
            8 => {
                if let Some(v) = prompt_text("sandbox_image", &config.sandbox_image, false)? {
                    config.sandbox_image = v;
                }
            }
            9 => {
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

/// Single-line text prompt that returns `None` on Esc (dialoguer's `Input`
/// can't be cancelled). Enter on an empty line keeps `current` unless
/// `allow_empty`, in which case it clears the value.
fn prompt_text(prompt: &str, current: &str, allow_empty: bool) -> Result<Option<String>> {
    let term = Term::stderr();
    let mut buf = String::new();
    let mut touched = false;
    loop {
        let shown = if touched { buf.as_str() } else { current };
        term.write_str(&format!("{prompt} [Esc to go back]: {shown}"))?;
        let key = term.read_key()?;
        term.clear_line()?;
        match key {
            Key::Escape => return Ok(None),
            Key::Enter => {
                let keep = !touched || (buf.is_empty() && !allow_empty);
                return Ok(Some(if keep { current.to_string() } else { buf }));
            }
            Key::Backspace => {
                if !touched {
                    touched = true;
                    buf = current.to_string();
                }
                buf.pop();
            }
            Key::Char(c) if !c.is_control() => {
                if !touched {
                    touched = true;
                    buf.clear();
                }
                buf.push(c);
            }
            _ => {}
        }
    }
}
