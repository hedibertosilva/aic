use std::process::Command;

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

    pub fn run(&self, opts: &RunOpts) -> Result<AgentRunResult> {
        let mut cmd = match &self.sandbox {
            Some(sandbox) => self.sandboxed_command(sandbox, opts)?,
            None => {
                let mut cmd = Command::new(&self.binary);
                self.push_args(&mut cmd, opts);
                if let Some(cwd) = &opts.cwd {
                    cmd.current_dir(cwd);
                }
                cmd
            }
        };

        let output = cmd.output().with_context(|| {
            format!(
                "running '{}' (configured as agent '{}'); is it installed and on PATH?",
                self.binary, self.name
            )
        })?;

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
    fn sandboxed_command(&self, sandbox: &Sandbox, opts: &RunOpts) -> Result<Command> {
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
        };
        sandbox.command(&spec)
    }

    /// Builds the subprocess args. `claude` and `agy` share `-p`,
    /// `--output-format json` and `--json-schema`, but diverge on how edits
    /// are allowed: `claude` uses `--permission-mode acceptEdits`, `agy` uses
    /// `--mode accept-edits`.
    fn push_args(&self, cmd: &mut Command, opts: &RunOpts) {
        cmd.arg("-p").arg(&opts.prompt);
        cmd.arg("--output-format").arg("json");

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
