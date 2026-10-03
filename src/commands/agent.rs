use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Result};

use crate::agent::CliAgent;
use crate::chat::quotas;
use crate::config::Config;
use crate::sandbox::Sandbox;

/// `aic agent list`: the configured agents, whether their binary is found,
/// and which ones are the default executor/reviewer.
pub fn list(config: &Config) -> Result<()> {
    let mut names: Vec<&String> = config.agents.keys().collect();
    names.sort();
    let width = names.iter().map(|n| n.len()).max().unwrap_or(0).max("NAME".len());
    println!("{:<width$}  {:<9}  {:<8}  {:<10}  BINARY", "NAME", "ROLE", "MODEL", "EFFORT");
    for name in names {
        let agent = &config.agents[name];
        let role = match (*name == config.workflow.executor, *name == config.workflow.reviewer) {
            (true, true) => "exec,rev",
            (true, false) => "executor",
            (false, true) => "reviewer",
            _ => "-",
        };
        let found = match resolve_binary(&agent.binary) {
            Some(path) => path.display().to_string(),
            None => format!("{} (not found)", agent.binary),
        };
        println!(
            "{name:<width$}  {role:<9}  {:<8}  {:<10}  {found}",
            or_dash(&agent.model),
            or_dash(&agent.effort),
        );
    }
    Ok(())
}

/// `aic agent status [agent]`: reads the account quota windows through each
/// CLI (the same reading the chat shows). It can take a few seconds per
/// agent, and a failure usually means the agent isn't logged in.
pub fn status(config: &Config, cwd: &Path, agent: Option<String>) -> Result<()> {
    let mut names: Vec<String> = match agent {
        Some(name) => {
            config.agent(&name)?;
            vec![name]
        }
        None => config.agents.keys().cloned().collect(),
    };
    names.sort();
    for name in names {
        let cfg = config.agent(&name)?;
        match resolve_binary(&cfg.binary) {
            Some(path) => println!("{name} ({})", path.display()),
            None => {
                println!("{name}: binary '{}' not found", cfg.binary);
                continue;
            }
        }
        let cli = CliAgent::with_defaults(name.clone(), cfg.binary.clone(), cfg.model.clone(), cfg.effort.clone())
            .with_sandbox(Sandbox::from_config(config)?);
        match quotas::read(&cli, cwd) {
            Ok(status) if status.windows.is_empty() => {
                let why = if status.reason.is_empty() { "no quota windows reported" } else { &status.reason };
                println!("  {why}");
            }
            Ok(status) => {
                for w in status.windows {
                    let used = w.used_pct.map_or("unknown".to_string(), |p| format!("{p:.0}% used"));
                    let reset = w.resets_at.map_or(String::new(), |r| format!(", resets {r}"));
                    println!("  {}: {used}{reset}", w.label);
                }
            }
            Err(err) => println!("  couldn't read quota (not logged in? try `aic agent login {name}`): {err:#}"),
        }
    }
    Ok(())
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

/// The binary as a path: used as is when it has a directory part, otherwise
/// looked up on `PATH`.
pub(crate) fn resolve_binary(binary: &str) -> Option<PathBuf> {
    let path = Path::new(binary);
    if path.components().count() > 1 {
        path.is_file().then(|| path.to_path_buf())
    } else {
        crate::sandbox::find_on_path(binary)
    }
}


#[cfg(test)]
mod login_tests {
    use super::*;

    #[test]
    fn gemini_is_an_alias_of_agy() {
        let config = Config::default();
        assert_eq!(login_target(&config, "gemini").1, "agy");
        assert_eq!(login_target(&config, "agy").1, "agy");
        assert_eq!(login_target(&config, "claude").1, "claude");
        assert_eq!(login_target(&config, "codex").1, "codex");
    }
}

/// Binary and its file stem for a login argument: an `[agents]` key such as
/// `gemini` resolves to its configured binary (`agy`); anything else is
/// taken as a binary name.
fn login_target<'a>(config: &'a Config, agent: &'a str) -> (&'a str, &'a str) {
    let binary = config.agents.get(agent).map_or(agent, |a| a.binary.as_str());
    let stem = std::path::Path::new(binary)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(binary);
    (binary, stem)
}

/// `aic agent login <agent>` (also reachable as `aic sandbox login`). `agent` is a key under `[agents]` (`gemini`,
/// `claude`, `codex`) or a binary name (`agy`).
///
/// - `agy`: runs interactively in the sandbox, since its host login lives in
///   the keyring, which containers can't reach; the sandbox keeps its own.
/// - `claude`/`codex`: the sandbox reuses the host's login files, so their
///   own CLI opens on the host and shows its login screen.
pub fn login(config: &Config, repo_root: &std::path::Path, agent: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let (binary, stem) = login_target(config, agent);
    match stem {
        "agy" => {
            let Some(sandbox) = Sandbox::from_config(config)? else {
                bail!("sandbox is set to \"none\"; agy uses your host login, nothing to do");
            };
            sandbox.preflight()?;
            let err = sandbox.login_command(repo_root, "agy")?.exec();
            bail!("failed to exec '{}': {err}", sandbox.runtime_name());
        }
        "claude" | "codex" => {
            // The bare CLI: its own first-run login screen shows the user
            // they are signing in to the vendor, not to aic.
            let err = Command::new(binary).exec();
            bail!("failed to exec '{binary}': {err}");
        }
        _ => bail!("don't know how to log '{agent}' in; use claude, codex or gemini (agy)"),
    }
}
