use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::git;
use crate::sandbox::{Access, RunSpec, Sandbox};

/// What a `CliAgent::run` call should do.
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// JSON Schema string requesting structured output.
    pub json_schema: Option<String>,
    /// Directory the subprocess should run in (defaults to the current one).
    pub cwd: Option<std::path::PathBuf>,
    /// Desired permission/edit mode: `"acceptEdits"` lets the agent edit
    /// files without asking, `"plan"` asks it to propose without touching
    /// anything. `None` means "don't pass a permission flag at all" — used
    /// for reviewer calls, which never edit files.
    pub permission: Option<String>,
    /// Auto-approve every tool permission request (reading files, running
    /// commands, editing). Headless runs can't answer an interactive
    /// permission prompt, so without this, any tool call outside what
    /// `permission` already allows is silently denied — the agent then
    /// reports `status: SUCCESS` with an empty response instead of failing
    /// loudly. Only set when the user opted in via `skip_permissions`;
    /// with a sandbox configured, the damage a skipped prompt can do is
    /// limited to the mounted repo, otherwise the agent has the user's full
    /// privileges on the host.
    pub skip_permissions: bool,
    /// Mount the repo read-only in the sandbox (reviewer calls). Ignored
    /// when no sandbox is configured.
    pub read_only: bool,
    /// Continue an earlier conversation by the id the agent reported
    /// (`session_id` for claude, `conversation_id` for agy).
    pub resume: Option<String>,
    /// Extra system prompt (claude's `--append-system-prompt`). `agy` has no
    /// equivalent, so callers fold it into the prompt themselves.
    pub system_prompt: Option<String>,
    /// Emit `stream-json` events instead of one JSON envelope. Only
    /// meaningful with `CliAgent::spawn_stream`.
    pub stream: bool,
}

/// A streaming run from `CliAgent::spawn_stream`.
pub struct AgentProcess {
    pub child: Child,
    pub killer: Killer,
}

/// Stops a streaming run and everything it started (nested `aic` runs and
/// their agents included). Cheap to clone, so a watcher thread can hold one.
#[derive(Clone)]
pub struct Killer {
    pgid: u32,
    /// Sandbox and container name: killing the `run` client alone would
    /// leave the container running.
    container: Option<(Sandbox, String)>,
}

impl Killer {
    pub fn kill(&self) {
        if let Some((sandbox, name)) = &self.container {
            let _ = sandbox.kill_command(name).status();
        }
        // The process group, plus every descendant found before killing
        // anything (tools may start commands in groups of their own, and
        // killed parents get their children reparented).
        let mut targets = vec![format!("-{}", self.pgid)];
        targets.extend(descendants(self.pgid).iter().map(u32::to_string));
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg("--")
            .args(&targets)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Pids of every process below `root`, from `/proc`.
fn descendants(root: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    let links: Vec<(u32, u32)> = entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            Some((pid, parent_pid(&stat)?))
        })
        .collect();
    let mut found = Vec::new();
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        for &(pid, ppid) in &links {
            if ppid == parent && !found.contains(&pid) {
                found.push(pid);
                queue.push(pid);
            }
        }
    }
    found
}

/// The ppid field of a `/proc/<pid>/stat` line: `pid (comm) state ppid …`.
/// `comm` may hold spaces and parens, so split after the last `)`.
fn parent_pid(stat: &str) -> Option<u32> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// Normalized result of a headless agent run, regardless of which CLI
/// produced it. `claude` and `agy` use different envelope shapes (see
/// `parse_envelope` below) but both expose a `structured_output` field with
/// the same shape when `--json-schema` is used, which is what callers in
/// `review.rs` rely on.
#[derive(Debug, Clone)]
pub struct AgentRunResult {
    pub success: bool,
    /// Free-text response, for logging/display.
    pub text: String,
    /// Parsed `--json-schema` result, if the agent produced valid structured output.
    pub structured_output: Option<Value>,
}

/// A CLI-backed agent (`claude` or `agy`/Antigravity). Flag names and
/// envelope shape are decided by which *binary* is being run, not by the
/// logical config key (`reviewer`/`executor` can point at any key name).
pub struct CliAgent {
    pub name: String,
    pub binary: String,
    /// Defaults from this agent's config entry, used whenever a `RunOpts`
    /// doesn't already specify its own `model`/`effort`.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// When set, every run happens inside this container sandbox.
    pub sandbox: Option<Sandbox>,
}

impl CliAgent {
    pub fn new(name: impl Into<String>, binary: impl Into<String>) -> Self {
        CliAgent {
            name: name.into(),
            binary: binary.into(),
            model: None,
            effort: None,
            sandbox: None,
        }
    }

    pub fn with_sandbox(mut self, sandbox: Option<Sandbox>) -> Self {
        self.sandbox = sandbox;
        self
    }

    pub fn with_defaults(
        name: impl Into<String>,
        binary: impl Into<String>,
        model: impl Into<Option<String>>,
        effort: impl Into<Option<String>>,
    ) -> Self {
        CliAgent {
            name: name.into(),
            binary: binary.into(),
            model: model.into().filter(|s| !s.is_empty()),
            effort: effort.into().filter(|s| !s.is_empty()),
            sandbox: None,
        }
    }

    /// The file name of `binary`, stripped of any directory and extension —
    /// e.g. "/usr/local/bin/agy" and "agy" both yield "agy".
    fn binary_stem(&self) -> &str {
        std::path::Path::new(&self.binary)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.binary)
    }

    /// Builds the (possibly sandboxed) command for one invocation.
    /// `container` names the sandbox container, when there is one.
    fn build_command(&self, opts: &RunOpts, container: Option<String>) -> Result<Command> {
        Ok(match &self.sandbox {
            Some(sandbox) => self.sandboxed_command(sandbox, opts, container)?,
            None => {
                // Inside an aic sandbox there's no nested container to mount
                // the repo read-only, so a read-only run falls back to plan
                // mode.
                let plan;
                let opts = if opts.read_only && opts.permission.is_none() && crate::config::in_sandbox() {
                    plan = RunOpts { permission: Some("plan".to_string()), ..opts.clone() };
                    &plan
                } else {
                    opts
                };
                let mut cmd = Command::new(&self.binary);
                self.push_args(&mut cmd, opts);
                if let Some(cwd) = &opts.cwd {
                    cmd.current_dir(cwd);
                }
                cmd
            }
        })
    }

    /// Starts the agent with piped stdout/stderr and returns immediately, so
    /// the caller can read `stream-json` events as they arrive. Set
    /// `opts.stream`. The agent gets its own process group, so the returned
    /// `Killer` stops it together with everything it started.
    pub fn spawn_stream(&self, opts: &RunOpts) -> Result<AgentProcess> {
        static RUNS: AtomicU64 = AtomicU64::new(0);
        let container = self.sandbox.as_ref().map(|_| {
            format!("aic-{}-{}", std::process::id(), RUNS.fetch_add(1, Ordering::Relaxed))
        });
        let mut cmd = self.build_command(opts, container.clone())?;
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.process_group(0);
        let child = cmd.spawn().with_context(|| self.spawn_error())?;
        let killer = Killer {
            pgid: child.id(),
            container: self.sandbox.clone().zip(container),
        };
        Ok(AgentProcess { child, killer })
    }

    fn spawn_error(&self) -> String {
        if crate::config::in_sandbox() {
            format!(
                "'{}' (configured as agent '{}') isn't available inside the sandbox; \
rebuild the image with it on the host PATH (`aic sandbox build`) or configure a different agent",
                self.binary, self.name
            )
        } else {
            format!(
                "running '{}' (configured as agent '{}'); is it installed and on PATH?",
                self.binary, self.name
            )
        }
    }

    /// Whether this agent speaks `agy`'s envelope rather than `claude`'s.
    pub fn is_agy(&self) -> bool {
        self.binary_stem() == "agy"
    }

    pub fn is_codex(&self) -> bool {
        self.binary_stem() == "codex"
    }

    pub fn run(&self, opts: &RunOpts) -> Result<AgentRunResult> {
        let mut cmd = self.build_command(opts, None)?;

        let output = cmd.output().with_context(|| self.spawn_error())?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "agent '{}' ({}) exited with {}: {}",
                self.name,
                self.binary,
                output.status,
                if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                }
            );
        }

        self.parse_envelope(&stdout)
    }

    /// Wraps this agent's invocation as `<runtime> run … <image> <binary>
    /// <args>`. Fails before starting anything if the runtime or image is
    /// missing.
    fn sandboxed_command(&self, sandbox: &Sandbox, opts: &RunOpts, name: Option<String>) -> Result<Command> {
        sandbox.preflight()?;
        let repo_root = opts
            .cwd
            .clone()
            .context("a sandboxed run needs a working directory inside the repo")?;
        let mut argv = Command::new(&self.binary);
        self.push_args(&mut argv, opts);
        let spec = RunSpec {
            git_dir: git::git_common_dir(&repo_root).ok(),
            workdir: repo_root.clone(),
            repo_root,
            access: if opts.read_only { Access::ReadOnly } else { Access::ReadWrite },
            interactive: false,
            agent: self.binary_stem().to_string(),
            args: argv.get_args().map(|a| a.to_string_lossy().into_owned()).collect(),
            name,
        };
        sandbox.command(&spec)
    }

    /// Builds the subprocess args. `claude` and `agy` share `-p`,
    /// `--output-format json` and `--json-schema`, but diverge on how edits
    /// are allowed: `claude` uses `--permission-mode acceptEdits`, `agy` uses
    /// `--mode accept-edits`.
    fn push_args(&self, cmd: &mut Command, opts: &RunOpts) {
        if self.is_codex() {
            cmd.arg("exec");
            if let Some(id) = &opts.resume {
                cmd.arg("resume").arg(id);
            }
            cmd.arg("--json");
            if let Some(model) = opts.model.as_ref().or(self.model.as_ref()) {
                cmd.arg("--model").arg(model);
            }
            if let Some(effort) = opts.effort.as_ref().or(self.effort.as_ref()) {
                cmd.arg("-c").arg(format!("model_reasoning_effort={effort}"));
            }
            if opts.skip_permissions {
                cmd.arg("--dangerously-bypass-approvals-and-sandbox");
            } else if opts.permission.as_deref().is_some_and(|p| p.eq_ignore_ascii_case("plan")) || opts.read_only {
                cmd.args(["--sandbox", "read-only"]);
            } else {
                cmd.args(["--sandbox", "workspace-write", "--approve-for-me"]);
            }
            let prompt = if let Some(schema) = &opts.json_schema {
                format!("{}\n\nReturn only a JSON value that conforms exactly to this JSON Schema:\n{}", opts.prompt, schema)
            } else {
                opts.prompt.clone()
            };
            cmd.arg(prompt);
            return;
        }
        cmd.arg("-p").arg(&opts.prompt);
        cmd.arg("--output-format")
            .arg(if opts.stream { "stream-json" } else { "json" });
        if opts.stream && self.binary_stem() != "agy" {
            // claude refuses stream-json under -p without --verbose.
            cmd.arg("--verbose").arg("--include-partial-messages");
        }
        if let Some(system) = &opts.system_prompt {
            if self.binary_stem() != "agy" {
                cmd.arg("--append-system-prompt").arg(system);
            }
        }
        if let Some(id) = &opts.resume {
            cmd.arg(if self.binary_stem() == "agy" { "--conversation" } else { "--resume" })
                .arg(id);
        }

        if let Some(schema) = &opts.json_schema {
            cmd.arg("--json-schema").arg(schema);
        }
        if let Some(model) = opts.model.as_ref().or(self.model.as_ref()) {
            cmd.arg("--model").arg(model);
        }
        if let Some(effort) = opts.effort.as_ref().or(self.effort.as_ref()) {
            cmd.arg("--effort").arg(effort);
        }
        if opts.skip_permissions {
            cmd.arg("--dangerously-skip-permissions");
        }
        if let Some(permission) = &opts.permission {
            let is_plan = permission.eq_ignore_ascii_case("plan");
            match self.binary_stem() {
                "agy" => {
                    cmd.arg("--mode").arg(if is_plan { "plan" } else { "accept-edits" });
                }
                _ => {
                    cmd.arg("--permission-mode")
                        .arg(if is_plan { "plan" } else { "acceptEdits" });
                }
            }
        }
    }

    fn parse_envelope(&self, stdout: &str) -> Result<AgentRunResult> {
        if self.is_codex() {
            return self.parse_codex_envelope(stdout);
        }
        let value: Value = serde_json::from_str(stdout.trim()).with_context(|| {
            format!(
                "agent '{}' did not return valid JSON on stdout: {stdout}",
                self.name
            )
        })?;

        let (success, text) = match self.binary_stem() {
            "agy" => {
                let success = value.get("status").and_then(Value::as_str) == Some("SUCCESS");
                let text = value
                    .get("response")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                (success, text)
            }
            _ => {
                // claude: success is `subtype == "success"` and `is_error == false`,
                // there is no top-level `status` field.
                let is_error = value.get("is_error").and_then(Value::as_bool).unwrap_or(true);
                let subtype_ok = value.get("subtype").and_then(Value::as_str) == Some("success");
                let text = value
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                (subtype_ok && !is_error, text)
            }
        };

        let structured_output = value.get("structured_output").cloned();

        Ok(AgentRunResult {
            success,
            text,
            structured_output,
        })
    }

    fn parse_codex_envelope(&self, stdout: &str) -> Result<AgentRunResult> {
        let values: Vec<Value> = stdout.lines().filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).with_context(|| format!("agent '{}' did not return valid JSONL: {line}", self.name)))
            .collect::<Result<_>>()?;
        let success = values.iter().any(|v| v.get("type").and_then(Value::as_str) == Some("turn.completed"));
        let text = values.iter().rev().find_map(|v| {
            let item = v.get("item")?;
            (item.get("type").and_then(Value::as_str) == Some("agent_message"))
                .then(|| item.get("text").and_then(Value::as_str).unwrap_or_default().to_string())
        }).unwrap_or_default();
        let structured_output = serde_json::from_str(&text).ok();
        Ok(AgentRunResult { success, text, structured_output })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_gemini_success_envelope() {
        let agent = CliAgent::new("gemini", "agy");
        let stdout = r#"{"conversation_id":"abc","status":"SUCCESS","response":"hello","structured_output":{"approved":true,"summary":"looks good"}}"#;
        let result = agent.parse_envelope(stdout).unwrap();
        assert!(result.success);
        assert_eq!(result.text, "hello");
        assert_eq!(
            result.structured_output.unwrap()["approved"],
            Value::Bool(true)
        );
    }

    #[test]
    fn parses_real_claude_success_envelope() {
        let agent = CliAgent::new("claude", "claude");
        let stdout = r#"{"subtype":"success","is_error":false,"result":"hello","structured_output":{"approved":true,"summary":"looks good"}}"#;
        let result = agent.parse_envelope(stdout).unwrap();
        assert!(result.success);
        assert_eq!(result.text, "hello");
        assert_eq!(
            result.structured_output.unwrap()["approved"],
            Value::Bool(true)
        );
    }

    #[test]
    fn parses_codex_jsonl_result_and_structured_output() {
        let agent = CliAgent::new("codex", "codex");
        let stdout = "{\"type\":\"thread.started\",\"thread_id\":\"t1\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{\\\"approved\\\":true}\"}}\n{\"type\":\"turn.completed\"}";
        let result = agent.parse_envelope(stdout).unwrap();
        assert!(result.success);
        assert_eq!(result.structured_output.unwrap()["approved"], Value::Bool(true));
    }

    #[test]
    fn gemini_error_status_is_not_success() {
        let agent = CliAgent::new("gemini", "agy");
        let stdout = r#"{"status":"ERROR","response":"something broke"}"#;
        let result = agent.parse_envelope(stdout).unwrap();
        assert!(!result.success);
    }

    #[test]
    fn claude_is_error_true_is_not_success() {
        let agent = CliAgent::new("claude", "claude");
        let stdout = r#"{"subtype":"success","is_error":true,"result":"partial"}"#;
        let result = agent.parse_envelope(stdout).unwrap();
        assert!(!result.success);
    }

    #[test]
    fn missing_structured_output_is_none() {
        let agent = CliAgent::new("gemini", "agy");
        let stdout = r#"{"status":"SUCCESS","response":"plain text, no schema requested"}"#;
        let result = agent.parse_envelope(stdout).unwrap();
        assert!(result.structured_output.is_none());
    }

    #[test]
    fn non_json_stdout_is_an_error() {
        let agent = CliAgent::new("gemini", "agy");
        assert!(agent.parse_envelope("not json").is_err());
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn parent_pid_survives_odd_process_names() {
        assert_eq!(parent_pid("42 (claude) S 7 42 42 0"), Some(7));
        assert_eq!(parent_pid("42 (a (b) c) R 9 1"), Some(9));
        assert_eq!(parent_pid("garbage"), None);
    }

    #[test]
    fn descendants_finds_children_of_this_process() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let found = descendants(std::process::id());
        let _ = child.kill();
        let _ = child.wait();
        assert!(found.contains(&child.id()));
    }

    #[test]
    fn accept_edits_uses_different_flag_per_agent() {
        let opts = RunOpts {
            prompt: "do something".to_string(),
            permission: Some("acceptEdits".to_string()),
            ..Default::default()
        };

        let mut gemini_cmd = Command::new("agy");
        CliAgent::new("gemini", "agy").push_args(&mut gemini_cmd, &opts);
        assert!(args_of(&gemini_cmd).windows(2).any(|w| w == ["--mode", "accept-edits"]));

        let mut claude_cmd = Command::new("claude");
        CliAgent::new("claude", "claude").push_args(&mut claude_cmd, &opts);
        assert!(args_of(&claude_cmd)
            .windows(2)
            .any(|w| w == ["--permission-mode", "acceptEdits"]));
    }

    #[test]
    fn plan_permission_uses_plan_value_per_agent() {
        let opts = RunOpts {
            prompt: "do something".to_string(),
            permission: Some("plan".to_string()),
            ..Default::default()
        };

        let mut gemini_cmd = Command::new("agy");
        CliAgent::new("gemini", "agy").push_args(&mut gemini_cmd, &opts);
        assert!(args_of(&gemini_cmd).windows(2).any(|w| w == ["--mode", "plan"]));

        let mut claude_cmd = Command::new("claude");
        CliAgent::new("claude", "claude").push_args(&mut claude_cmd, &opts);
        assert!(args_of(&claude_cmd)
            .windows(2)
            .any(|w| w == ["--permission-mode", "plan"]));
    }

    #[test]
    fn resume_and_stream_flags_differ_per_agent() {
        let opts = RunOpts {
            prompt: "hi".to_string(),
            resume: Some("abc".to_string()),
            stream: true,
            ..Default::default()
        };

        let mut claude_cmd = Command::new("claude");
        CliAgent::new("claude", "claude").push_args(&mut claude_cmd, &opts);
        let args = args_of(&claude_cmd);
        assert!(args.windows(2).any(|w| w == ["--resume", "abc"]));
        assert!(args.windows(2).any(|w| w == ["--output-format", "stream-json"]));
        assert!(args.iter().any(|a| a == "--verbose"));
        assert!(args.iter().any(|a| a == "--include-partial-messages"));

        let mut gemini_cmd = Command::new("agy");
        CliAgent::new("gemini", "agy").push_args(&mut gemini_cmd, &opts);
        let args = args_of(&gemini_cmd);
        assert!(args.windows(2).any(|w| w == ["--conversation", "abc"]));
        assert!(args.windows(2).any(|w| w == ["--output-format", "stream-json"]));
        assert!(!args.iter().any(|a| a == "--verbose"));

        let mut codex_cmd = Command::new("codex");
        CliAgent::new("codex", "codex").push_args(&mut codex_cmd, &opts);
        let args = args_of(&codex_cmd);
        assert!(args.windows(2).any(|w| w == ["resume", "abc"]));
        assert!(args.iter().any(|a| a == "--json"));
    }

    #[test]
    fn system_prompt_is_claude_only() {
        let opts = RunOpts {
            prompt: "hi".to_string(),
            system_prompt: Some("be brief".to_string()),
            ..Default::default()
        };
        let mut claude_cmd = Command::new("claude");
        CliAgent::new("claude", "claude").push_args(&mut claude_cmd, &opts);
        assert!(args_of(&claude_cmd).windows(2).any(|w| w == ["--append-system-prompt", "be brief"]));

        let mut gemini_cmd = Command::new("agy");
        CliAgent::new("gemini", "agy").push_args(&mut gemini_cmd, &opts);
        assert!(!args_of(&gemini_cmd).iter().any(|a| a == "--append-system-prompt"));
    }

    #[test]
    fn no_permission_set_means_no_permission_flag() {
        let opts = RunOpts {
            prompt: "do something".to_string(),
            ..Default::default()
        };
        let mut cmd = Command::new("claude");
        CliAgent::new("claude", "claude").push_args(&mut cmd, &opts);
        assert!(!args_of(&cmd).iter().any(|a| a == "--permission-mode"));
    }
}
