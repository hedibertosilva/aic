use crate::{
    config::{Config, SandboxKind},
    diagnostics,
};
use anyhow::{Result, bail};
use dialoguer::{Confirm, Select};
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub fn doctor(root: &Path, json: bool) -> Result<()> {
    let config = Config::load_merged(root)?;
    let report = diagnostics::snapshot(root, &config);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "Repository: {} · {}",
        report.repository.state,
        root.display()
    );
    if let Some(git) = &report.git {
        println!("Branch: {}\nCheckout: {}", git.label, git.checkout);
    }
    println!(
        "System: {} / {}\nRuntime: {} · {}\nSandbox: {} · {}\nChat default: {}",
        report.system,
        report.architecture,
        report.runtime.state,
        report.runtime.reason,
        report.image.state,
        report.image.reason,
        if report.default_agent.is_empty() {
            "not selected"
        } else {
            &report.default_agent
        }
    );
    println!("AGENT    INSTALLATION  AUTHENTICATION   EXECUTION       QUOTA       READY");
    for a in report.agents {
        println!(
            "{:<8} {:<13} {:<16} {:<15} {:<11} {}",
            a.agent, a.installation, a.authentication, a.execution, a.quota, a.readiness
        );
        println!(
            "  {} · context: {} · last test: {}",
            a.reason,
            a.context,
            a.tested_at
                .map(|t| t.to_string())
                .unwrap_or_else(|| "never".into())
        );
        if !a.action.is_empty() {
            println!("  Next: {}", a.action);
        }
    }
    Ok(())
}
pub fn agent_test(root: &Path, name: Option<String>, yes: bool, timeout: u64) -> Result<()> {
    if !(1..=300).contains(&timeout) {
        bail!("timeout must be between 1 and 300 seconds");
    }
    let config = Config::load_merged(root)?;
    let names: Vec<String> = if let Some(name) = name {
        config.agent(&name)?;
        vec![name]
    } else {
        let mut names: Vec<_> = config.agents.keys().cloned().collect();
        names.sort();
        names
    };
    println!("This test makes a real provider call for each selected agent and may consume quota.");
    if !yes {
        if !io::stdin().is_terminal() {
            bail!("Use --yes to consent to the real provider call, or run interactively");
        }
        if !Confirm::new()
            .with_prompt("Run the availability test?")
            .default(false)
            .interact()?
        {
            println!("Cancelled; no test performed.");
            return Ok(());
        }
    }
    let mut failed = false;
    for name in names {
        let result = diagnostics::test_agent(root, &config, &name, Duration::from_secs(timeout));
        match result {
            Ok(d) => {
                println!("{}: {} · {}", name, d.readiness, d.reason);
                if !d.action.is_empty() {
                    println!("Next: {}", d.action);
                }
                failed |= d.readiness != "ready";
            }
            Err(e) => {
                failed = true;
                println!("{name}: {e}");
            }
        }
    }
    if failed {
        bail!(
            "One or more agents are unavailable; correct the reported issue and repeat aic agent test"
        );
    }
    Ok(())
}
/// Save in the effective scope so a repository override cannot mask the choice.
pub fn set_default(root: &Path, name: &str) -> Result<()> {
    let cfg = Config::load_merged(root)?;
    if !diagnostics::snapshot(root, &cfg)
        .agents
        .iter()
        .any(|a| a.agent == name && a.readiness == "ready")
    {
        bail!("Only ready agents can be selected; run aic agent test {name}");
    }
    let path = Config::repo_override_path(root);
    if path.exists() {
        let mut table: toml::Table = toml::from_str(&std::fs::read_to_string(&path)?)?;
        if table
            .get("chat")
            .and_then(|t| t.get("default_agent"))
            .is_some()
        {
            let chat = table
                .get_mut("chat")
                .and_then(|v| v.as_table_mut())
                .unwrap();
            chat.insert("default_agent".into(), toml::Value::String(name.into()));
            std::fs::write(&path, toml::to_string_pretty(&table)?)?;
            println!("Saved chat.default_agent in {}", path.display());
            return Ok(());
        }
    }
    crate::commands::config_cmd::set("chat.default_agent", name)
}
pub fn run(root: &Path, requested: Option<String>, yes: bool, no_open: bool) -> Result<()> {
    if !io::stdin().is_terminal() && (requested.is_none() || !yes) {
        bail!(
            "Interactive setup needs a terminal. Use aic setup --agent NAME --yes --no-open for explicit unattended setup"
        );
    }
    crate::commands::init::run(root)?;
    if let Some(name) = requested {
        let config = Config::load_merged(root)?;
        config.agent(&name)?;
        if !diagnostics::snapshot(root, &config)
            .agents
            .iter()
            .any(|a| a.agent == name && a.readiness == "ready")
        {
            agent_test(root, Some(name.clone()), yes, 45)?;
        }
        set_default(root, &name)?;
        println!("Setup complete. Next: aic chat");
        if !no_open
            && io::stdin().is_terminal()
            && Confirm::new()
                .with_prompt("Create your first chat now?")
                .default(true)
                .interact()?
        {
            crate::chat::server::run(root, "127.0.0.1", 7878, true)?;
        }
        return Ok(());
    }
    let mut stage = 0;
    loop {
        let config = Config::load_merged(root)?;
        let report = diagnostics::snapshot(root, &config);
        println!(
            "\n{} of 5 · {}",
            stage + 1,
            [
                "Environment",
                "Sandbox",
                "Agents",
                "Default agent",
                "Conclusion"
            ][stage]
        );
        let choices: Vec<String> = match stage {
            0 => vec![
                format!(
                    "Continue · {} / {} · {}",
                    report.system,
                    report.architecture,
                    root.display()
                ),
                "Choose runtime".into(),
                "Finish later".into(),
            ],
            1 => vec![
                format!("Continue · image {}", report.image.state),
                "Prepare sandbox".into(),
                "Back".into(),
                "Finish later".into(),
            ],
            2 => {
                let mut c = vec!["Continue (unused agents can be skipped)".into()];
                for a in &report.agents {
                    c.push(format!("{} · {} · {}", a.agent, a.readiness, a.reason));
                }
                c.extend(["Back".into(), "Finish later".into()]);
                c
            }
            3 => {
                let mut c: Vec<String> = report
                    .agents
                    .iter()
                    .filter(|a| a.readiness == "ready")
                    .map(|a| format!("Use {} for new chats", a.agent))
                    .collect();
                c.extend(["Back".into(), "Finish later".into()]);
                c
            }
            _ => vec!["Create first chat".into(), "Conclude".into(), "Back".into()],
        };
        let Some(choice) = Select::new()
            .with_prompt("Choose next step (Esc finishes later)")
            .items(&choices)
            .default(0)
            .interact_opt()?
        else {
            println!("Pending steps saved. Resume with aic setup.");
            return Ok(());
        };
        if choices[choice] == "Finish later" {
            println!(
                "Setup pending. Run aic setup to resume; sends require a ready selected agent."
            );
            return Ok(());
        }
        if choices[choice] == "Back" {
            stage = stage.saturating_sub(1);
            continue;
        }
        match stage {
            0 => {
                if choice == 1 {
                    let rt = Select::new()
                        .with_prompt("Runtime (none runs directly on your host)")
                        .items(["docker", "podman", "none"])
                        .interact()?;
                    crate::commands::config_cmd::set(
                        "sandbox.runtime",
                        ["docker", "podman", "none"][rt],
                    )?;
                } else {
                    stage += 1;
                }
            }
            1 => {
                if choice == 1 {
                    if config.sandbox.runtime == SandboxKind::None {
                        println!("Host execution does not require an image");
                    } else if report.image.state == "ready" {
                        println!("Image already prepared; no build repeated");
                    } else if let Err(e) = crate::commands::sandbox::build(&config, None, None) {
                        println!(
                            "Sandbox preparation failed: {e}. Next: aic sandbox build, then aic setup"
                        );
                    }
                } else if report.runtime.state == "ready"
                    && matches!(report.image.state.as_str(), "ready" | "not_required")
                {
                    stage += 1;
                } else {
                    println!(
                        "{}; next: {}. Choose Back to change runtime or Finish later.",
                        report.image.reason, report.image.action
                    );
                }
            }
            2 => {
                if choice == 0 {
                    stage += 1;
                } else if let Some(a) = report.agents.get(choice - 1) {
                    if a.installation == "missing" {
                        println!(
                            "Install {} using {}. aic does not install third-party CLIs.",
                            a.agent,
                            diagnostics::installation_url(&config.agents[&a.agent].binary)
                        );
                    } else if a.authentication == "login_required" {
                        if Confirm::new()
                            .with_prompt(format!("Open {} login in the terminal?", a.agent))
                            .default(false)
                            .interact()?
                        {
                            let status = Command::new(std::env::current_exe()?)
                                .args(["agent", "login", &a.agent])
                                .current_dir(root)
                                .status()?;
                            if !status.success() {
                                println!(
                                    "Login did not complete; repeat aic agent login {}",
                                    a.agent
                                );
                            }
                        }
                    } else if a.readiness == "ready" {
                        println!("{} already tested; no call repeated", a.agent);
                    } else if let Err(e) = agent_test(root, Some(a.agent.clone()), false, 45) {
                        println!("{e}. Correct the issue and select the agent to retry.");
                    }
                }
            }
            3 => {
                let ready: Vec<_> = report
                    .agents
                    .iter()
                    .filter(|a| a.readiness == "ready")
                    .collect();
                if let Some(a) = ready.get(choice) {
                    set_default(root, &a.agent)?;
                    stage += 1;
                }
            }
            _ => {
                if choice == 0 && !no_open {
                    crate::chat::server::run(root, "127.0.0.1", 7878, true)?;
                } else {
                    println!(
                        "Setup complete. Chat default: {}. Next: aic",
                        report.default_agent
                    );
                }
                return Ok(());
            }
        }
    }
}
