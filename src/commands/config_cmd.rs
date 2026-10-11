use std::path::Path;

use anyhow::{bail, Context, Result};
use console::{Key, Term};
use dialoguer::{Confirm, Select};

use crate::config::{Config, Mode, SandboxKind};

/// Config keys `aic config set` accepts, as `section.key`.
const KEYS: &[&str] = &[
    "permissions.mode",
    "permissions.skip",
    "sandbox.runtime",
    "sandbox.image",
    "sandbox.bind_agents",
    "chat.default_agent",
];

/// The key a legacy flat name (`mode`, `sandbox_image`, ...) now lives at.
fn canonical_key(key: &str) -> &str {
    match key {
        "executor" => "workflow.executor",
        "reviewer" => "workflow.reviewer",
        "mode" => "workflow.mode",
        "max_rounds" => "workflow.max_rounds",
        "default_base_branch" => "workflow.default_base_branch",
        "permission_mode" => "permissions.mode",
        "skip_permissions" => "permissions.skip",
        "sandbox" => "sandbox.runtime",
        "sandbox_image" => "sandbox.image",
        "sandbox_bind_agents" => "sandbox.bind_agents",
        other => other,
    }
}

/// Prints the public part of the *effective* config: global, with the current
/// repo's `.aic/config.toml` override (if any) applied on top.
pub fn show(repo_root: &Path) -> Result<()> {
    let config = Config::load_merged(repo_root)?;
    if crate::config::in_sandbox() {
        println!("# running inside the aic sandbox: agents run directly ([sandbox] runtime = \"none\")\n");
    }
    let mut public = toml::Value::try_from(&config)?;
    if let Some(table) = public.as_table_mut() {
        table.remove("workflow");
    }
    println!("{}", toml::to_string_pretty(&public)?);
    Ok(())
}

/// Sets one public key of the global config. Takes `section.key` (see `KEYS`);
/// legacy flat names for public keys such as `sandbox_image` still work.
pub fn set(key: &str, value: &str) -> Result<()> {
    let key = canonical_key(key);
    if !KEYS.contains(&key) {
        bail!("unknown config key '{key}' ({})", KEYS.join(", "));
    }
    let mut config = Config::load_global()?;
    match key {
        "workflow.executor" => config.workflow.executor = value.to_string(),
        "workflow.reviewer" => config.workflow.reviewer = value.to_string(),
        "workflow.mode" => {
            config.workflow.mode = match value {
                "manual" => Mode::Manual,
                "auto" => Mode::Auto,
                other => bail!("unknown mode '{other}', expected 'manual' or 'auto'"),
            };
        }
        "workflow.max_rounds" => {
            config.workflow.max_rounds = value
                .parse()
                .with_context(|| format!("'{value}' isn't a valid number"))?;
        }
        "workflow.default_base_branch" => config.workflow.default_base_branch = value.to_string(),
        "permissions.mode" => {
            if !matches!(value, "acceptEdits" | "plan") {
                bail!("unknown permission mode '{value}', expected 'acceptEdits' or 'plan'");
            }
            config.permissions.mode = value.to_string();
        }
        "permissions.skip" => config.permissions.skip = parse_bool(value)?,
        "sandbox.runtime" => {
            config.sandbox.runtime = SandboxKind::parse(value)
                .with_context(|| format!("unknown sandbox '{value}', expected 'docker', 'podman' or 'none'"))?;
        }
        "sandbox.image" => config.sandbox.image = value.to_string(),
        "sandbox.bind_agents" => config.sandbox.bind_agents = parse_bool(value)?,
        "chat.default_agent" => config.chat.default_agent = value.to_string(),
        other => bail!("unknown config key '{other}' ({})", KEYS.join(", ")),
    }
    config.save_global()?;
    println!("Set {key} = {value}");
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => bail!("'{other}' isn't 'true' or 'false'"),
    }
}

/// Every key the menu edits, as `section.key`; also what changes are
/// tracked (and summarized on save) by.
const TRACKED: &[&str] = &[
    "workflow.executor",
    "workflow.reviewer",
    "workflow.mode",
    "workflow.max_rounds",
    "workflow.default_base_branch",
    "permissions.mode",
    "permissions.skip",
    "sandbox.runtime",
    "sandbox.image",
    "sandbox.bind_agents",
    "sandbox.mounts",
    "sandbox.env",
    "chat.default_agent",
];

const WORKFLOW_KEYS: &[&str] = &[
    "workflow.executor",
    "workflow.reviewer",
    "workflow.mode",
    "workflow.max_rounds",
    "workflow.default_base_branch",
];
const PERMISSIONS_KEYS: &[&str] = &["permissions.mode", "permissions.skip"];
const SANDBOX_KEYS: &[&str] = &[
    "sandbox.runtime",
    "sandbox.image",
    "sandbox.bind_agents",
    "sandbox.mounts",
    "sandbox.env",
];
const CHAT_KEYS: &[&str] = &["chat.default_agent"];

/// The value of a tracked key as plain text.
fn value_of(config: &Config, key: &str) -> String {
    match key {
        "workflow.executor" => config.workflow.executor.clone(),
        "workflow.reviewer" => config.workflow.reviewer.clone(),
        "workflow.mode" => mode_label(config.workflow.mode).to_string(),
        "workflow.max_rounds" => config.workflow.max_rounds.to_string(),
        "workflow.default_base_branch" => config.workflow.default_base_branch.clone(),
        "permissions.mode" => config.permissions.mode.clone(),
        "permissions.skip" => config.permissions.skip.to_string(),
        "sandbox.runtime" => config.sandbox.runtime.label().to_string(),
        "sandbox.image" => config.sandbox.image.clone(),
        "sandbox.bind_agents" => config.sandbox.bind_agents.to_string(),
        "sandbox.mounts" => config.sandbox.mounts.join(", "),
        "sandbox.env" => config.sandbox.env.join(", "),
        "chat.default_agent" => config.chat.default_agent.clone(),
        _ => String::new(),
    }
}

/// How a value reads on screen: switches as `[x]`/`[ ]`, empty values as a
/// hint of what empty means.
fn shown(key: &str, raw: &str) -> String {
    match key {
        "permissions.skip" | "sandbox.bind_agents" => (if raw == "true" { "[x]" } else { "[ ]" }).to_string(),
        _ if raw.is_empty() => match key {
            "workflow.default_base_branch" => "(auto-detect)",
            "chat.default_agent" => "(choose in chat)",
            _ => "(none)",
        }
        .to_string(),
        _ => raw.to_string(),
    }
}

/// `(key, before, after)` for every tracked key that differs.
fn changes(before: &Config, after: &Config) -> Vec<(&'static str, String, String)> {
    TRACKED
        .iter()
        .filter_map(|key| {
            let (old, new) = (value_of(before, key), value_of(after, key));
            (old != new).then(|| (*key, old, new))
        })
        .collect()
}

/// One menu line per key: a `•` marks a value that differs from the
/// default, and the names and values line up in columns.
fn section_lines(config: &Config, defaults: &Config, keys: &[&str]) -> Vec<String> {
    let label = |key: &str| key.split_once('.').map_or(key.to_string(), |(_, k)| k.to_string());
    let width = keys.iter().copied().map(|k| label(k).len()).max().unwrap_or(0);
    keys.iter()
        .copied()
        .map(|key| {
            let value = value_of(config, key);
            let marker = if value != value_of(defaults, key) { "•" } else { " " };
            format!("{marker} {:<width$}  {}", label(key), shown(key, &value))
        })
        .collect()
}

/// A `/config`-style menu: pick a section, change its keys, then save or
/// discard. Changes stay in memory until you save.
pub fn interactive() -> Result<()> {
    let mut config = Config::load_global()?;
    let original = config.clone();
    let defaults = Config::default();

    let mut note = None;

    loop {
        draw(&mut note)?;
        let pending = changes(&original, &config).len();
        let items = vec![
            format!(
                "{:<11}  {}{}",
                "Permissions",
                config.permissions.mode,
                if config.permissions.skip { " · auto-approve tools" } else { "" }
            ),
            format!(
                "{:<11}  {} · {}",
                "Sandbox",
                config.sandbox.runtime.label(),
                config.sandbox.image
            ),
            format!("{:<11}  default agent {}", "Chat", config.chat_default_agent()),
            format!(
                "Save and exit{}",
                match pending {
                    0 => String::new(),
                    1 => "  * 1 change".to_string(),
                    n => format!("  * {n} changes"),
                }
            ),
            "Exit without saving".to_string(),
        ];

        let selection = Select::new()
        .report(false)
            .with_prompt("Section")
            .items(&items)
            .default(0)
            .interact_opt()?;

        match selection {
            Some(0) => permissions_menu(&mut config, &defaults)?,
            Some(1) => sandbox_menu(&mut config, &defaults)?,
            Some(2) => chat_menu(&mut config, &defaults)?,
            Some(3) => {
                save(&config, &original)?;
                break;
            }
            _ => {
                let discard = pending == 0
                    || Confirm::new()
                        .report(false)
                        .with_prompt(format!("Discard {pending} unsaved change(s)?"))
                        .default(false)
                        .interact()?;
                if discard {
                    Term::stderr().clear_screen()?;
                    println!("Discarded changes.");
                    break;
                }
            }
        }
    }

    Ok(())
}

/// `path` with the home directory shown as `~`.
fn tilde(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// Clears the screen and prints the header and any pending note, so each
/// menu replaces the previous one instead of piling up in the scrollback.
fn draw(note: &mut Option<String>) -> Result<()> {
    let term = Term::stderr();
    term.clear_screen()?;
    // The same header on every screen; only the prompt below it changes.
    let path = Config::global_path().map(|p| tilde(&p)).unwrap_or_default();
    term.write_line(&format!("aic config · {path}"))?;
    term.write_line("Enter edits · Esc goes back · • = differs from the default")?;
    term.write_line("")?;
    if let Some(note) = note.take() {
        term.write_line(&note)?;
        term.write_line("")?;
    }
    Ok(())
}

/// Writes the config and says what changed.
fn save(config: &Config, original: &Config) -> Result<()> {
    Term::stderr().clear_screen()?;
    let changed = changes(original, config);
    if changed.is_empty() {
        println!("No changes.");
        return Ok(());
    }
    for (key, old, new) in &changed {
        println!("  {key}  {} → {}", shown(key, old), shown(key, new));
    }
    if changed.iter().any(|(key, _, _)| *key == "sandbox.bind_agents") {
        println!("  bind_agents changed: rebuild the image with `aic sandbox build`.");
    }
    config.save_global()?;
    println!("Saved to {}", Config::global_path()?.display());
    Ok(())
}

/// Shows `items` plus a trailing "Back"; `None` on Back or Esc.
fn pick(prompt: &str, mut items: Vec<String>) -> Result<Option<usize>> {
    let back = items.len();
    items.push("Back".to_string());
    let picked = Select::new()
        .report(false)
        .with_prompt(prompt)
        .items(&items)
        .default(0)
        .interact_opt()?;
    Ok(picked.filter(|i| *i != back))
}

/// Pick one of `options`; `None` on Esc.
fn choose_from(prompt: &str, options: &[&str], current: &str) -> Result<Option<String>> {
    let default = options.iter().position(|o| *o == current).unwrap_or(0);
    let picked = Select::new()
        .report(false)
        .with_prompt(prompt)
        .items(options)
        .default(default)
        .interact_opt()?;
    Ok(picked.map(|i| options[i].to_string()))
}

/// Pick a configured agent, with its binary shown (or flagged as missing).
/// With `allow_empty`, the first entry clears the value. `None` on Esc.
fn choose_agent(prompt: &str, config: &Config, current: &str, allow_empty: bool) -> Result<Option<String>> {
    let mut names: Vec<&String> = config.agents.keys().collect();
    names.sort();
    let width = names.iter().map(|n| n.len()).max().unwrap_or(0);
    let mut items: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    if allow_empty {
        items.push("(choose in chat)".to_string());
        values.push(String::new());
    }
    for name in names {
        let binary = &config.agents[name].binary;
        let found = match super::agent::resolve_binary(binary) {
            Some(path) => path.display().to_string(),
            None => format!("{binary} (binary not found)"),
        };
        items.push(format!("{name:<width$}  {found}"));
        values.push(name.clone());
    }
    let default = values.iter().position(|v| v == current).unwrap_or(0);
    let picked = Select::new()
        .report(false)
        .with_prompt(prompt)
        .items(&items)
        .default(default)
        .interact_opt()?;
    Ok(picked.map(|i| values[i].clone()))
}

#[allow(dead_code)] // Retained while Workflow is hidden from the interactive menu.
fn workflow_menu(config: &mut Config, defaults: &Config) -> Result<()> {
    let mut note = None;
    loop {
        draw(&mut note)?;
        let items = section_lines(config, defaults, WORKFLOW_KEYS);
        let Some(choice) = pick("Workflow", items)? else { return Ok(()) };
        match WORKFLOW_KEYS[choice] {
            "workflow.executor" => {
                if let Some(v) = choose_agent("executor", config, &config.workflow.executor, false)? {
                    config.workflow.executor = v;
                }
            }
            "workflow.reviewer" => {
                if let Some(v) = choose_agent("reviewer", config, &config.workflow.reviewer, false)? {
                    config.workflow.reviewer = v;
                }
            }
            "workflow.mode" => {
                if let Some(v) = choose_from("mode", &["manual", "auto"], mode_label(config.workflow.mode))? {
                    config.workflow.mode = if v == "auto" { Mode::Auto } else { Mode::Manual };
                }
            }
            "workflow.max_rounds" => {
                let current = config.workflow.max_rounds;
                if let Some(text) = prompt_text("max_rounds", &current.to_string(), false)? {
                    match text.trim().parse::<u32>() {
                        Ok(n) if n > 0 => config.workflow.max_rounds = n,
                        _ => note = Some(format!("Error: max_rounds must be a number greater than 0; kept {current}.")),
                    }
                }
            }
            _ => {
                let current = config.workflow.default_base_branch.clone();
                if let Some(v) = prompt_text("default_base_branch (empty = auto-detect)", &current, true)? {
                    config.workflow.default_base_branch = v;
                }
            }
        }
    }
}

fn permissions_menu(config: &mut Config, defaults: &Config) -> Result<()> {
    let mut note = None;
    loop {
        draw(&mut note)?;
        let items = section_lines(config, defaults, PERMISSIONS_KEYS);
        let Some(choice) = pick("Permissions", items)? else { return Ok(()) };
        match PERMISSIONS_KEYS[choice] {
            "permissions.mode" => {
                if let Some(v) = choose_from("mode", &["acceptEdits", "plan"], &config.permissions.mode)? {
                    config.permissions.mode = v;
                }
            }
            _ => {
                config.permissions.skip = !config.permissions.skip;
                if config.permissions.skip {
                    note = Some("skip: every tool request is auto-approved in headless runs; use only with agents you trust.".to_string());
                }
            }
        }
    }
}

fn sandbox_menu(config: &mut Config, defaults: &Config) -> Result<()> {
    let mut note = None;
    loop {
        draw(&mut note)?;
        let items = section_lines(config, defaults, SANDBOX_KEYS);
        let Some(choice) = pick("Sandbox", items)? else { return Ok(()) };
        match SANDBOX_KEYS[choice] {
            "sandbox.runtime" => {
                if let Some(v) = choose_from("runtime", &["docker", "podman", "none"], config.sandbox.runtime.label())? {
                    config.sandbox.runtime = SandboxKind::parse(&v).unwrap_or(config.sandbox.runtime);
                }
            }
            "sandbox.image" => {
                let current = config.sandbox.image.clone();
                if let Some(v) = prompt_text("image", &current, false)? {
                    config.sandbox.image = v;
                }
            }
            "sandbox.bind_agents" => {
                config.sandbox.bind_agents = !config.sandbox.bind_agents;
                note = Some("bind_agents: host agy/Codex binaries; rebuild the image after changing (aic sandbox build).".to_string());
            }
            "sandbox.mounts" => {
                let current = config.sandbox.mounts.join(", ");
                if let Some(v) = prompt_text("mounts (comma-separated host:container[:ro])", &current, true)? {
                    config.sandbox.mounts = split_list(&v);
                }
            }
            _ => {
                let current = config.sandbox.env.join(", ");
                if let Some(v) = prompt_text("env (comma-separated variable names)", &current, true)? {
                    config.sandbox.env = split_list(&v);
                }
            }
        }
    }
}

fn chat_menu(config: &mut Config, defaults: &Config) -> Result<()> {
    let mut note = None;
    loop {
        draw(&mut note)?;
        let items = section_lines(config, defaults, CHAT_KEYS);
        let Some(_) = pick("Chat", items)? else { return Ok(()) };
        if let Some(v) = choose_agent("default_agent", config, &config.chat.default_agent, true)? {
            config.chat.default_agent = v;
        }
    }
}

fn split_list(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_lists_only_differing_keys() {
        let before = Config::default();
        let mut after = before.clone();
        assert!(changes(&before, &after).is_empty());
        after.workflow.max_rounds = 5;
        after.sandbox.bind_agents = true;
        let changed = changes(&before, &after);
        assert_eq!(
            changed,
            vec![
                ("workflow.max_rounds", "3".to_string(), "5".to_string()),
                ("sandbox.bind_agents", "false".to_string(), "true".to_string()),
            ]
        );
    }

    #[test]
    fn switches_and_empty_values_read_naturally() {
        assert_eq!(shown("permissions.skip", "true"), "[x]");
        assert_eq!(shown("sandbox.bind_agents", "false"), "[ ]");
        assert_eq!(shown("workflow.default_base_branch", ""), "(auto-detect)");
        assert_eq!(shown("chat.default_agent", ""), "(choose in chat)");
        assert_eq!(shown("sandbox.mounts", ""), "(none)");
        assert_eq!(shown("workflow.executor", "claude"), "claude");
    }

    #[test]
    fn menu_lines_align_and_mark_non_default_values() {
        let defaults = Config::default();
        let mut config = defaults.clone();
        config.workflow.max_rounds = 5;
        let lines = section_lines(&config, &defaults, WORKFLOW_KEYS);
        assert_eq!(lines[0], "  executor             claude");
        assert_eq!(lines[3], "• max_rounds           5");
        assert_eq!(lines[4], "  default_base_branch  (auto-detect)");
    }

    #[test]
    fn legacy_flat_keys_map_to_sections() {
        assert_eq!(canonical_key("mode"), "workflow.mode");
        assert_eq!(canonical_key("sandbox"), "sandbox.runtime");
        assert_eq!(canonical_key("permissions.skip"), "permissions.skip");
    }
}
