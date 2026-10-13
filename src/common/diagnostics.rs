//! Read-only, credential-free diagnosis shared by CLI and web.
use crate::agent::{CliAgent, RunOpts};
use crate::chat::store::{ChatEvent, parse_line_for};
use crate::config::{Config, SandboxKind};
use crate::sandbox::Sandbox;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static CACHE_LOCK: Mutex<()> = Mutex::new(());
static PROBES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const TTL: i64 = 24 * 60 * 60;
const OUTPUT_LIMIT: u64 = 1 << 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitContext {
    pub repository: String,
    pub checkout: String,
    pub branch: Option<String>,
    pub sha: String,
    pub label: String,
}
pub fn git_context(root: &Path) -> Result<GitContext> {
    let read = |args: &[&str]| -> Result<String> {
        let out = Command::new("git").args(args).current_dir(root).output()?;
        if !out.status.success() {
            bail!("Git context unavailable");
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let checkout = read(&["rev-parse", "--show-toplevel"])?;
    let branch = read(&["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let sha = read(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let label = branch
        .clone()
        .unwrap_or_else(|| format!("detached HEAD · {sha}"));
    Ok(GitContext {
        repository: Path::new(&checkout)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        checkout,
        branch,
        sha,
        label,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub state: String,
    pub reason: String,
    pub action: String,
}
fn check(state: &str, reason: &str, action: &str) -> Check {
    Check {
        state: state.into(),
        reason: reason.into(),
        action: action.into(),
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDiagnostic {
    pub agent: String,
    pub configuration: String,
    pub installation: String,
    pub host_installation: String,
    pub authentication: String,
    pub execution: String,
    pub quota: String,
    pub readiness: String,
    pub reason: String,
    pub action: String,
    pub tested_at: Option<i64>,
    pub origin: String,
    pub context: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub schema_version: u32,
    pub checked_at: i64,
    pub system: String,
    pub architecture: String,
    pub git: Option<GitContext>,
    pub repository: Check,
    pub runtime: Check,
    pub image: Check,
    pub default_agent: String,
    pub agents: Vec<AgentDiagnostic>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestRecord {
    signature: String,
    result: AgentDiagnostic,
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    tests: BTreeMap<String, TestRecord>,
}
fn cache_path(root: &Path) -> PathBuf {
    root.join(".aic/diagnostics/tests.json")
}
fn load_cache(root: &Path) -> Cache {
    std::fs::read(cache_path(root))
        .ok()
        .and_then(|s| serde_json::from_slice(&s).ok())
        .unwrap_or_default()
}
fn save_cache(root: &Path, name: &str, record: TestRecord) -> Result<()> {
    let _lock = CACHE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("diagnostic cache lock unavailable"))?;
    let mut cache = load_cache(root);
    cache.tests.insert(name.into(), record);
    let path = cache_path(root);
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(&serde_json::to_vec(&cache)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
fn resolve_binary(binary: &str) -> Option<PathBuf> {
    let valid = |p: &Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if binary.contains('/') {
        return valid(Path::new(binary)).then(|| PathBuf::from(binary));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join(binary))
        .find(|p| valid(p))
}
fn identity(path: &Path) -> String {
    path.metadata()
        .map(|m| format!("{}:{}:{:?}", path.display(), m.len(), m.modified().ok()))
        .unwrap_or_else(|_| "missing".into())
}

/// Bound both streams and wall time; terminate the entire query process group.
fn bounded(mut command: Command, timeout: Duration) -> Result<(bool, String)> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().context("diagnostic command unavailable")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut stream = stream;
            let _ = (&mut stream).take(OUTPUT_LIMIT).read_to_end(&mut out);
            // Drain after the limit without retaining vendor data.
            let _ = std::io::copy(&mut stream, &mut std::io::sink());
            out
        })
    };
    let a = read(Box::new(stdout));
    let b = read(Box::new(stderr));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    crate::agent::Killer::process_group(child.id()).kill();
    let _ = child.kill();
    let _ = child.wait();
    let out = a.join().unwrap_or_default();
    let err = b.join().unwrap_or_default();
    let Some(status) = status else {
        bail!("timeout");
    };
    Ok((
        status.success(),
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&err)
        ),
    ))
}
pub(crate) fn command_succeeds(command: Command, timeout: Duration) -> Result<bool> {
    bounded(command, timeout).map(|(success, _)| success)
}

fn environment(config: &Config) -> (Check, Check, String) {
    if config.sandbox.runtime == SandboxKind::None {
        return (
            check("ready", "host execution", ""),
            check("not_required", "host execution", ""),
            "host".into(),
        );
    }
    let rt = config.sandbox.runtime.label();
    if resolve_binary(rt).is_none() {
        return (
            check("missing", "Container runtime absent", "aic setup"),
            check("unknown", "Runtime absent", "aic setup"),
            "missing".into(),
        );
    }
    let mut cmd = Command::new(rt);
    cmd.args(["info"]);
    if !bounded(cmd, Duration::from_secs(3)).is_ok_and(|(ok, _)| ok) {
        return (
            check("unavailable", "Container runtime unavailable", "aic setup"),
            check("unknown", "Runtime unavailable", "aic setup"),
            "unavailable".into(),
        );
    }
    let mut cmd = Command::new(rt);
    cmd.args([
        "image",
        "inspect",
        "--format",
        "{{.Id}}",
        &config.sandbox.image,
    ]);
    match bounded(cmd, Duration::from_secs(3)) {
        Ok((true, id)) => (
            check("ready", "Runtime available", ""),
            check("ready", "Sandbox image available", ""),
            id.trim().into(),
        ),
        _ => (
            check("ready", "Runtime available", ""),
            check("missing", "Sandbox image absent", "aic sandbox build"),
            "missing".into(),
        ),
    }
}
fn signature(config: &Config, name: &str, image_id: &str) -> String {
    let mut hash = DefaultHasher::new();
    if let Ok(cfg) = config.agent(name) {
        serde_json::to_string(cfg)
            .unwrap_or_default()
            .hash(&mut hash);
        resolve_binary(&cfg.binary)
            .map(|p| identity(&p))
            .hash(&mut hash);
        if config.sandbox.bind_agents
            && let Some(binary) = resolve_binary(&cfg.binary).and_then(|p| p.canonicalize().ok())
        {
            identity(
                &binary
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .join("codex-code-mode-host"),
            )
            .hash(&mut hash);
            resolve_binary("codex-code-mode-host")
                .map(|p| identity(&p))
                .hash(&mut hash);
        }
    }
    serde_json::to_string(&config.sandbox)
        .unwrap_or_default()
        .hash(&mut hash);
    serde_json::to_string(&config.permissions)
        .unwrap_or_default()
        .hash(&mut hash);
    resolve_binary(config.sandbox.runtime.label())
        .map(|p| identity(&p))
        .hash(&mut hash);
    image_id.hash(&mut hash);
    let cli = config
        .agent(name)
        .ok()
        .map(|cfg| CliAgent::new(name, &cfg.binary));
    let (files, variables): (&[&str], &[&str]) = match cli {
        Some(ref a) if a.is_codex() => (
            &[".codex/auth.json", ".codex/config.toml"],
            &["OPENAI_API_KEY", "CODEX_HOME"],
        ),
        Some(ref a) if a.is_agy() => (&[".local/share/aic/sandbox/agy"], &[]),
        _ => (
            &[".claude/.credentials.json", ".claude/settings.json"],
            &[
                "ANTHROPIC_API_KEY",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CONFIG_DIR",
            ],
        ),
    };
    if let Ok(home) = std::env::var("HOME") {
        for rel in files {
            identity(&Path::new(&home).join(rel)).hash(&mut hash);
        }
    }
    for key in variables
        .iter()
        .copied()
        .chain(config.sandbox.env.iter().map(String::as_str))
    {
        // Only this provider's relevant variables; never serialize or display values.
        std::env::var_os(key).hash(&mut hash);
    }
    format!("{:016x}", hash.finish())
}
fn claude_login(output: &str) -> Option<bool> {
    // Deserialize the first JSON document, including pretty-printed output;
    // a trailing diagnostic on stderr is not part of the auth payload.
    serde_json::Deserializer::from_str(output.trim())
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()?
        .get("loggedIn")?
        .as_bool()
}

fn auth_status(binary: &str) -> String {
    let cli = CliAgent::new("probe", binary);
    let mut cmd = Command::new(binary);
    if cli.is_claude() {
        cmd.args(["auth", "status", "--json"]);
    } else if cli.is_codex() {
        cmd.args(["login", "status"]);
    } else {
        return "unknown".into();
    }
    match bounded(cmd, Duration::from_secs(3)) {
        Ok((ok, out)) if cli.is_claude() => {
            let logged = claude_login(&out);
            match logged {
                Some(true) if ok => "authenticated",
                Some(false) => "login_required",
                _ => "unknown",
            }
            .into()
        }
        Ok((ok, out)) => {
            let lower = out.to_ascii_lowercase();
            if ok && lower.contains("logged in") {
                "authenticated"
            } else if lower.contains("not logged in") {
                "login_required"
            } else {
                "unknown"
            }
            .into()
        }
        _ => "unknown".into(),
    }
}
pub fn snapshot(root: &Path, config: &Config) -> Diagnostic {
    snapshot_selected(root, config, None)
}

/// Validate only the requested agents against their last real test. This path
/// checks runtime/image identity and local metadata, without auth CLI calls or
/// starting probe containers. A missing/expired test still blocks execution.
pub fn for_send(root: &Path, config: &Config, agents: &[String]) -> Diagnostic {
    snapshot_selected(root, config, Some(agents))
}

fn snapshot_selected(root: &Path, config: &Config, selected: Option<&[String]>) -> Diagnostic {
    let now = chrono::Utc::now().timestamp();
    let git = git_context(root).ok();
    let (runtime, image, image_id) = environment(config);
    let context = if config.sandbox.runtime == SandboxKind::None {
        "host"
    } else {
        "sandbox"
    };
    let cache = load_cache(root);
    let mut names: Vec<_> = config.agents.keys()
        .filter(|name| selected.is_none_or(|agents| agents.contains(name)))
        .collect();
    names.sort();
    let agents = names
        .into_iter()
        .map(|name| {
            let cfg = &config.agents[name];
            let host = resolve_binary(&cfg.binary).is_some();
            let mut installation = if host { "installed" } else { "missing" }.to_owned();
            if context == "sandbox" {
                installation = "unknown".into();
                if selected.is_none() && runtime.state == "ready" && image.state == "ready" {
                    let mut cmd = Command::new(config.sandbox.runtime.label());
                    let probe_name = format!(
                        "aic-check-{}-{}",
                        std::process::id(),
                        PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    );
                    cmd.args([
                        "run",
                        "--name",
                        &probe_name,
                        "--rm",
                        "--network",
                        "none",
                        "--read-only",
                        "--entrypoint",
                        "sh",
                        &config.sandbox.image,
                        "-c",
                        "command -v -- \"$1\" >/dev/null",
                        "probe",
                        Path::new(&cfg.binary)
                            .file_name()
                            .and_then(|p| p.to_str())
                            .unwrap_or(&cfg.binary),
                    ]);
                    installation = match bounded(cmd, Duration::from_secs(5)) {
                        Ok((true, _)) => "installed",
                        Ok((false, _)) => "missing",
                        _ => "unknown",
                    }
                    .into();
                    let mut cleanup = Command::new(config.sandbox.runtime.label());
                    cleanup.args(["rm", "-f", &probe_name]);
                    let _ = bounded(cleanup, Duration::from_secs(2));
                    // Host binds are mounted by Sandbox; validate them separately.
                    if config.sandbox.bind_agents && !CliAgent::new(name, &cfg.binary).is_claude() {
                        installation = if host { "installed" } else { "missing" }.into();
                    }
                }
                if selected.is_some() && config.sandbox.bind_agents
                    && !CliAgent::new(name, &cfg.binary).is_claude()
                {
                    installation = if host { "installed" } else { "missing" }.into();
                }
            }
            let action = format!("aic agent test {name}");
            let mut d = AgentDiagnostic {
                agent: name.clone(),
                configuration: "configured".into(),
                host_installation: if host { "installed" } else { "missing" }.into(),
                installation,
                authentication: if selected.is_none() && context == "host" && host {
                    auth_status(&cfg.binary)
                } else {
                    "unknown".into()
                },
                execution: "unknown".into(),
                quota: "unknown".into(),
                readiness: "unknown".into(),
                reason: "Run a real availability test in the configured context".into(),
                action,
                tested_at: None,
                origin: "passive".into(),
                context: context.into(),
            };
            if let Some(record) = cache.tests.get(name).filter(|r| {
                r.signature == signature(config, name, &image_id)
                    && r.result
                        .tested_at
                        .is_some_and(|t| now >= t && now - t < TTL)
            }) {
                let passive_auth = d.authentication.clone();
                let installation = d.installation.clone();
                let host = d.host_installation.clone();
                d = record.result.clone();
                if selected.is_none() || installation != "unknown" {
                    d.installation = installation;
                }
                d.host_installation = host;
                if passive_auth == "login_required" {
                    d.authentication = passive_auth;
                    d.readiness = "blocked".into();
                    d.reason = "Login required".into();
                    d.action = format!("aic agent login {name}");
                }
            }
            if runtime.state != "ready" || !matches!(image.state.as_str(), "ready" | "not_required")
            {
                d.readiness = "blocked".into();
                d.reason = "Prepare the configured sandbox first".into();
                d.action = if image.state == "missing" {
                    "aic sandbox build"
                } else {
                    "aic setup"
                }
                .into();
            } else if d.installation == "missing" {
                d.readiness = "blocked".into();
                d.reason = "Agent CLI absent in the configured context".into();
                d.action = if context == "sandbox" {
                    "aic sandbox build".into()
                } else {
                    installation_url(&cfg.binary).into()
                };
            } else if d.authentication == "login_required" {
                d.readiness = "blocked".into();
                d.reason = "Login required".into();
                d.action = format!("aic agent login {name}");
            }
            d
        })
        .collect();
    Diagnostic {
        schema_version: 1,
        checked_at: now,
        system: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        repository: if git.is_some() {
            check("ready", "Git checkout available", "")
        } else {
            check(
                "missing",
                "Run inside a Git repository",
                "cd /path/to/repository",
            )
        },
        git,
        runtime,
        image,
        default_agent: config.chat_default_agent().into(),
        agents,
    }
}
pub fn installation_url(binary: &str) -> &'static str {
    let a = CliAgent::new("probe", binary);
    if a.is_codex() {
        "https://developers.openai.com/codex/cli/"
    } else if a.is_agy() {
        "https://github.com/google-antigravity/antigravity-cli"
    } else {
        "https://code.claude.com/docs/en/setup"
    }
}
/// Classification produces only fixed public strings; vendor output stays private.
pub fn classify_error(message: &str) -> (&'static str, &'static str) {
    let s = message.to_ascii_lowercase();
    if [
        "quota exceeded",
        "quota exhausted",
        "usage limit",
        "usagelimitexceeded",
        "insufficient_quota",
        "rate limit",
        "ratelimitexceeded",
    ]
    .iter()
    .any(|p| s.contains(p))
    {
        (
            "quota",
            "Quota exhausted or rate limited; retry after the provider reset",
        )
    } else if [
        "not logged in",
        "authentication required",
        "please log in",
        "please login",
        "login required",
        "invalid api key",
        "token expired",
        "unauthorized",
    ]
    .iter()
    .any(|p| s.contains(p))
    {
        ("authentication", "Login required")
    } else if [
        "connection refused",
        "network error",
        "failed to connect",
        "dns",
        "connection timed out",
    ]
    .iter()
    .any(|p| s.contains(p))
    {
        (
            "network",
            "Provider network unavailable; check connection and retry",
        )
    } else if s.contains("timeout") {
        (
            "timeout",
            "Availability test timed out; retry in the terminal",
        )
    } else {
        (
            "execution",
            "Agent execution failed; verify the CLI and retry in the terminal",
        )
    }
}
fn apply_failure(d: &mut AgentDiagnostic, message: &str) {
    let (kind, reason) = classify_error(message);
    d.readiness = "blocked".into();
    d.execution = "failure".into();
    d.reason = reason.into();
    // A successful test clears the recovery action. If a later chat run or
    // availability test fails, restore it so the Environment dialog can offer
    // Copy and Run instead of showing a blocked card with no way to retry.
    d.action = format!("aic agent test {}", d.agent);
    if kind == "quota" {
        d.quota = "exhausted".into();
    }
    if kind == "authentication" {
        d.authentication = "login_required".into();
        d.action = format!("aic agent login {}", d.agent);
    }
}
pub fn record_failure(root: &Path, config: &Config, name: &str, message: &str) -> Result<()> {
    let (_, _, id) = environment(config);
    let sig = signature(config, name, &id);
    let mut d = load_cache(root)
        .tests
        .get(name)
        .filter(|r| r.signature == sig)
        .map(|r| r.result.clone())
        .unwrap_or_else(|| AgentDiagnostic {
            agent: name.into(),
            configuration: "configured".into(),
            installation: "unknown".into(),
            host_installation: "unknown".into(),
            authentication: "unknown".into(),
            execution: "unknown".into(),
            quota: "unknown".into(),
            readiness: "unknown".into(),
            reason: String::new(),
            action: format!("aic agent test {name}"),
            tested_at: None,
            origin: "chat".into(),
            context: if config.sandbox.runtime == SandboxKind::None {
                "host"
            } else {
                "sandbox"
            }
            .into(),
        });
    apply_failure(&mut d, message);
    d.tested_at = Some(chrono::Utc::now().timestamp());
    d.origin = "chat".into();
    save_cache(
        root,
        name,
        TestRecord {
            signature: sig,
            result: d,
        },
    )
}
pub fn test_agent(
    root: &Path,
    config: &Config,
    name: &str,
    timeout: Duration,
) -> Result<AgentDiagnostic> {
    config.agent(name)?;
    let report = snapshot(root, config);
    let mut d = report.agents.into_iter().find(|a| a.agent == name).unwrap();
    if report.repository.state != "ready" {
        bail!("Run agent test inside a Git repository");
    }
    if d.installation == "missing"
        || report.runtime.state != "ready"
        || !matches!(report.image.state.as_str(), "ready" | "not_required")
    {
        bail!("{}; next: {}", d.reason, d.action);
    }
    let cfg = config.agent(name)?;
    let agent = CliAgent::with_defaults(
        name,
        cfg.binary.clone(),
        cfg.model.clone(),
        cfg.effort.clone(),
    )
    .with_sandbox(Sandbox::from_config(config)?);
    let opts = RunOpts {
        prompt:
            "Reply with the single word READY. Do not read files, use tools, or modify anything."
                .into(),
        cwd: Some(root.to_owned()),
        read_only: true,
        permission: Some("plan".into()),
        stream: true,
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        let mut process = agent.spawn_stream(&opts)?;
        let killer = process.killer.clone();
        let stdout = process.child.stdout.take().unwrap();
        let stderr = process.child.stderr.take().unwrap();
        let a = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stdout.take(OUTPUT_LIMIT).read_to_string(&mut s);
            s
        });
        let b = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.take(OUTPUT_LIMIT).read_to_string(&mut s);
            s
        });
        let started = Instant::now();
        let status = loop {
            if let Some(status) = process.child.try_wait()? {
                break Some(status);
            }
            if started.elapsed() >= timeout {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        killer.kill();
        let _ = process.child.wait();
        let output = a.join().unwrap_or_default();
        let err = b.join().unwrap_or_default();
        let Some(status) = status else {
            bail!("timeout");
        };
        let mut finished = false;
        let mut text = false;
        let mut failure = None;
        for line in output.lines() {
            if let Some(event) = parse_line_for(agent.is_agy(), agent.is_codex(), line) {
                match event {
                    ChatEvent::Delta { text: t } => text |= !t.trim().is_empty(),
                    ChatEvent::Done {
                        ok, text: t, error, ..
                    } => {
                        finished = ok;
                        text |= !t.trim().is_empty();
                        failure = error;
                    }
                    _ => {}
                }
            }
        }
        if let Some(error) = failure {
            bail!("{error}");
        }
        if !status.success() {
            bail!("{err}");
        }
        if !finished || !text {
            bail!("Invalid response: no completed nonempty agent answer");
        }
        Ok(())
    })();
    d.tested_at = Some(chrono::Utc::now().timestamp());
    d.origin = "agent_test".into();
    match result {
        Ok(()) => {
            d.authentication = "authenticated".into();
            d.execution = if d.context == "host" {
                "ready_host"
            } else {
                "ready_sandbox"
            }
            .into();
            d.readiness = "ready".into();
            d.reason = "Real response confirmed in the configured context".into();
            d.action = String::new();
        }
        Err(e) => apply_failure(&mut d, &e.to_string()),
    }
    let (_, _, id) = environment(config);
    save_cache(
        root,
        name,
        TestRecord {
            signature: signature(config, name, &id),
            result: d.clone(),
        },
    )?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_auth_accepts_pretty_json_and_ignores_trailing_diagnostics() {
        assert_eq!(
            claude_login("{\n  \"loggedIn\": true,\n  \"email\": \"private\"\n}\nwarning"),
            Some(true)
        );
        assert_eq!(claude_login("{\n  \"loggedIn\": false\n}"), Some(false));
        assert_eq!(claude_login("unexpected output"), None);
    }
    #[test]
    fn quota_never_means_logout() {
        assert_eq!(classify_error("quota exhausted unauthorized").0, "quota");
        assert_eq!(classify_error("HTTP 429").0, "execution");
        assert_eq!(classify_error("token expired").0, "authentication");
        assert_eq!(classify_error("network error").0, "network");
    }
    #[test]
    fn execution_failure_restores_test_action_after_success() {
        let mut diagnostic = AgentDiagnostic {
            agent: "codex".into(),
            configuration: "configured".into(),
            installation: "installed".into(),
            host_installation: "installed".into(),
            authentication: "authenticated".into(),
            execution: "ready_host".into(),
            quota: "available".into(),
            readiness: "ready".into(),
            reason: "Real response confirmed in the configured context".into(),
            action: String::new(),
            tested_at: Some(1),
            origin: "agent_test".into(),
            context: "host".into(),
        };

        apply_failure(&mut diagnostic, "process exited unsuccessfully");

        assert_eq!(diagnostic.execution, "failure");
        assert_eq!(diagnostic.readiness, "blocked");
        assert_eq!(diagnostic.action, "aic agent test codex");
    }
    #[test]
    fn context_handles_unborn_and_detached() {
        let tmp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let c = git_context(tmp.path()).unwrap();
        assert!(c.branch.is_some());
        assert!(c.sha.is_empty());
    }
    #[test]
    fn bounded_command_times_out() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 10"]);
        let start = Instant::now();
        assert!(bounded(cmd, Duration::from_millis(50)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn cache_is_private_and_contains_only_safe_records() {
        let root = tempfile::tempdir().unwrap();
        let config = Config::default();
        record_failure(
            root.path(),
            &config,
            "codex",
            "token expired secret=DO_NOT_STORE",
        )
        .unwrap();
        let text = std::fs::read_to_string(cache_path(root.path())).unwrap();
        assert!(!text.contains("DO_NOT_STORE"));
        assert!(text.contains("login_required"));
        assert_eq!(
            cache_path(root.path())
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
