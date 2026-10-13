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
            println!("  Action: {}", a.action);
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
                    println!("Action: {}", d.action);
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
        println!("\nSetup complete");
        println!("{}", "─".repeat(56));
        println!("Status: Ready");
        println!("\nYour first aic chat is ready.");
        println!("\nDefault agent: {name}");
        println!("Repository: {}", root.display());
        if !no_open && io::stdin().is_terminal() {
            let action = Select::new()
                .with_prompt("Select an action")
                .items(["[Open first chat]", "[Close setup]"])
                .default(0)
                .interact()?;
            if action == 0 {
                crate::chat::server::run(root, "127.0.0.1", 7878, true)?;
            }
        } else {
            println!("\nRun aic to start chatting.");
        }
        return Ok(());
    }
    let mut stage = 0;
    loop {
        let config = Config::load_merged(root)?;
        let report = diagnostics::snapshot(root, &config);
        let title = [
            "Environment",
            "Sandbox",
            "Connect an agent",
            "Choose your default agent",
        ][stage];
        println!("\nStep {} of 4 · {title}", stage + 1);
        println!("{}", "─".repeat(56));
        let choices: Vec<String> = match stage {
            0 => vec![
                "[Continue]".into(),
                "[Choose runtime]".into(),
                "[Save and exit]".into(),
            ],
            1 => vec![
                "[Continue]".into(),
                "[Build sandbox]".into(),
                "[Go back]".into(),
                "[Save and exit]".into(),
            ],
            2 => {
                let mut c = vec!["[Continue with ready agents]".into()];
                for a in &report.agents {
                    c.push(format!("{} · {} · {}", a.agent, a.readiness, a.reason));
                }
                c.extend(["[Go back]".into(), "[Save and exit]".into()]);
                c
            }
            3 => {
                let mut c: Vec<String> = report
                    .agents
                    .iter()
                    .filter(|a| a.readiness == "ready")
                    .map(|a| format!("[Use {} for new chats]", a.agent))
                    .collect();
                c.extend(["[Go back]".into(), "[Save and exit]".into()]);
                c
            }
            _ => unreachable!(),
        };
        match stage {
            0 => {
                println!("Status: Ready");
                println!("Repository: {}", root.display());
                println!("System: {} {}", report.system, report.architecture);
                if let Some(git) = &report.git {
                    println!("Git checkout: {}", git.label);
                }
                println!("\nYour repository and system are ready.");
            }
            1 => {
                let status = if config.sandbox.runtime == SandboxKind::None
                    || report.image.state == "ready"
                {
                    "Ready"
                } else {
                    "Not ready"
                };
                println!("Status: {status}");
                if config.sandbox.runtime == SandboxKind::None {
                    println!("\nAgents will run directly on the host.");
                } else if report.image.state == "ready" {
                    println!("\nAgents will run inside the configured sandbox.");
                    println!("Runtime: {}", config.sandbox.runtime.label());
                    println!("Image: {}", config.sandbox.image);
                } else {
                    println!("\nThe sandbox image has not been built yet.");
                    println!("\nWhy this matters:");
                    println!("The sandbox limits the files and permissions available to agents.");
                }
            }
            2 => {
                let ready = report.agents.iter().filter(|a| a.readiness == "ready").count();
                let status = if ready > 0 { "Ready" } else { "Action required" };
                println!("Status: {status}");
                println!("\naic needs at least one ready agent before you can start a chat.");
                println!("\nConfigured agents:");
                for a in &report.agents {
                    println!("  {} · {} · {}", a.agent, a.readiness, a.reason);
                }
            }
            3 => {
                let ready = report.agents.iter().filter(|a| a.readiness == "ready").count();
                println!("Status: {}", if ready > 0 { "Ready" } else { "Action required" });
                if ready > 0 {
                    println!("\nChoose the agent selected automatically when you create a new chat.");
                    println!("You can add other agents as participants later.");
                } else {
                    println!("\nChoose a ready agent before continuing.");
                }
            }
            _ => unreachable!(),
        }
        let Some(choice) = Select::new()
            .with_prompt("Select an action (Esc saves and exits)")
            .items(&choices)
            .default(0)
            .interact_opt()?
        else {
            println!("Setup saved. Resume later with aic setup.");
            return Ok(());
        };
        if choices[choice] == "[Save and exit]" {
            println!("Setup saved. Resume later with aic setup.");
            return Ok(());
        }
        if choices[choice] == "[Go back]" {
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
                        println!("\nStatus: Action required");
                        println!("The sandbox could not be prepared.");
                        println!("\nReason: {e}");
                        println!("\nYou can retry, choose host execution or save setup for later.");
                    }
                } else if report.runtime.state == "ready"
                    && matches!(report.image.state.as_str(), "ready" | "not_required")
                {
                    stage += 1;
                } else {
                    println!("\nStatus: Action required");
                    println!("{}", report.image.reason);
                    if !report.image.action.is_empty() {
                        println!("\nRecovery command: {}", report.image.action);
                    }
                    println!("\nChoose Go back to change the runtime or Save and exit.");
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
                    println!("\nSetup complete");
                    println!("{}", "─".repeat(56));
                    println!("Status: Ready");
                    println!("\nYour first aic chat is ready.");
                    println!("\nDefault agent: {}", a.agent);
                    println!("Sandbox: {}", config.sandbox.runtime.label());
                    println!("Repository: {}", root.display());
                    if !no_open {
                        let action = Select::new()
                            .with_prompt("Select an action")
                            .items(["[Open first chat]", "[Close setup]"])
                            .default(0)
                            .interact()?;
                        if action == 0 {
                            crate::chat::server::run(root, "127.0.0.1", 7878, true)?;
                        }
                    } else {
                        println!("\nSetup complete. Run aic to start chatting.");
                    }
                } else {
                    println!("Choose a ready agent before continuing.");
                }
                return Ok(());
            }
            _ => unreachable!(),
        }
    }
}
