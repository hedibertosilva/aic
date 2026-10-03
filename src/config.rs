use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Manual,
    Auto,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Manual
    }
}

/// Container runtime used to isolate agent invocations (see `sandbox.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxKind {
    Docker,
    Podman,
    /// Run agents directly on the host, with no isolation.
    None,
}

impl SandboxKind {
    pub fn label(self) -> &'static str {
        match self {
            SandboxKind::Docker => "docker",
            SandboxKind::Podman => "podman",
            SandboxKind::None => "none",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "docker" => Some(SandboxKind::Docker),
            "podman" => Some(SandboxKind::Podman),
            "none" => Some(SandboxKind::None),
            _ => None,
        }
    }
}

pub const DEFAULT_SANDBOX_IMAGE: &str = "aic-sandbox";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub binary: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub effort: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub executor: String,
    pub reviewer: String,
    pub mode: Mode,
    pub max_rounds: u32,
    pub permission_mode: String,
    pub default_base_branch: String,
    /// Opt-in: auto-approve every tool permission request for the
    /// configured agents (`--dangerously-skip-permissions`). Off by
    /// default. Headless runs can't answer an interactive permission
    /// prompt, so without this, a reviewer or executor that tries to read a
    /// file or run a command outside what `permission_mode` already allows
    /// gets silently denied and returns an empty response. Only turn this
    /// on if you trust the configured agents to run unattended on this
    /// repo.
    pub skip_permissions: bool,
    /// Extra patterns (on top of the built-in lockfile list) to drop from
    /// the diff sent to the reviewer in `aic dev review`.
    pub git_diff_exclude_patterns: Vec<String>,
    /// Patterns that re-include a path even if an exclusion above (built-in,
    /// config, `.gitignore`, `linguist-generated`) dropped it.
    pub git_diff_include_patterns: Vec<String>,
    /// Container runtime every agent invocation runs in. `none` keeps the
    /// pre-sandbox behavior of running agents directly on the host.
    pub sandbox: SandboxKind,
    /// Image the sandbox runs; `aic sandbox build` produces the default.
    pub sandbox_image: String,
    /// Bind the host's agy/Codex executables read-only instead of baking
    /// them into the image. Claude remains installed in the image.
    pub sandbox_bind_agents: bool,
    /// Extra `host:container[:ro]` binds (caches, toolchains).
    pub sandbox_mounts: Vec<String>,
    /// Names of host environment variables forwarded into the sandbox.
    pub sandbox_env: Vec<String>,
    pub agents: HashMap<String, AgentConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let mut agents = HashMap::new();
        agents.insert(
            "claude".to_string(),
            AgentConfig {
                binary: "claude".to_string(),
                model: String::new(),
                effort: String::new(),
            },
        );
        agents.insert(
            "gemini".to_string(),
            AgentConfig {
                binary: "agy".to_string(),
                model: String::new(),
                effort: String::new(),
            },
        );
        agents.insert(
            "codex".to_string(),
            AgentConfig {
                binary: "codex".to_string(),
                model: String::new(),
                effort: String::new(),
            },
        );
        Config {
            executor: "claude".to_string(),
            reviewer: "gemini".to_string(),
            mode: Mode::Manual,
            max_rounds: 3,
            permission_mode: "acceptEdits".to_string(),
            default_base_branch: String::new(),
            skip_permissions: false,
            git_diff_exclude_patterns: Vec::new(),
            git_diff_include_patterns: Vec::new(),
            sandbox: SandboxKind::Docker,
            sandbox_image: DEFAULT_SANDBOX_IMAGE.to_string(),
            sandbox_bind_agents: false,
            sandbox_mounts: Vec::new(),
            sandbox_env: Vec::new(),
            agents,
        }
    }
}

/// Subset of `Config` fields a repo-level `.aic/config.toml` may override.
/// Every field is optional so a repo only needs to list what it changes.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigOverrides {
    pub executor: Option<String>,
    pub reviewer: Option<String>,
    pub mode: Option<Mode>,
    pub max_rounds: Option<u32>,
    pub permission_mode: Option<String>,
    pub default_base_branch: Option<String>,
    pub skip_permissions: Option<bool>,
    pub git_diff_exclude_patterns: Option<Vec<String>>,
    pub git_diff_include_patterns: Option<Vec<String>>,
    pub sandbox: Option<SandboxKind>,
    pub sandbox_image: Option<String>,
    pub sandbox_bind_agents: Option<bool>,
    pub sandbox_mounts: Option<Vec<String>>,
    pub sandbox_env: Option<Vec<String>>,
    pub agents: Option<HashMap<String, AgentConfig>>,
}

impl Config {
    fn apply_overrides(&mut self, overrides: ConfigOverrides) {
        if let Some(v) = overrides.executor {
            self.executor = v;
        }
        if let Some(v) = overrides.reviewer {
            self.reviewer = v;
        }
        if let Some(v) = overrides.mode {
            self.mode = v;
        }
        if let Some(v) = overrides.max_rounds {
            self.max_rounds = v;
        }
        if let Some(v) = overrides.permission_mode {
            self.permission_mode = v;
        }
        if let Some(v) = overrides.default_base_branch {
            self.default_base_branch = v;
        }
        if let Some(v) = overrides.skip_permissions {
            self.skip_permissions = v;
        }
        if let Some(v) = overrides.git_diff_exclude_patterns {
            self.git_diff_exclude_patterns = v;
        }
        if let Some(v) = overrides.git_diff_include_patterns {
            self.git_diff_include_patterns = v;
        }
        if let Some(v) = overrides.sandbox {
            self.sandbox = v;
        }
        if let Some(v) = overrides.sandbox_image {
            self.sandbox_image = v;
        }
        if let Some(v) = overrides.sandbox_bind_agents {
            self.sandbox_bind_agents = v;
        }
        if let Some(v) = overrides.sandbox_mounts {
            self.sandbox_mounts = v;
        }
        if let Some(v) = overrides.sandbox_env {
            self.sandbox_env = v;
        }
        if let Some(agents) = overrides.agents {
            for (name, agent) in agents {
                self.agents.insert(name, agent);
            }
        }
    }

    pub fn agent(&self, name: &str) -> Result<&AgentConfig> {
        self.agents
            .get(name)
            .with_context(|| format!("no agent named '{name}' configured (check `aic config show`)"))
    }

    pub fn global_dir() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
            return Ok(PathBuf::from(dir));
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(xdg).join("aic"));
        }
        let home = std::env::var("HOME").context("HOME environment variable is not set")?;
        Ok(PathBuf::from(home).join(".config").join("aic"))
    }

    pub fn global_path() -> Result<PathBuf> {
        Ok(Self::global_dir()?.join("config.toml"))
    }

    pub fn repo_override_path(repo_root: &Path) -> PathBuf {
        repo_root.join(".aic").join("config.toml")
    }

    pub fn load_global() -> Result<Config> {
        let path = Self::global_path()?;
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut config: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        config.ensure_builtin_agents();
        Ok(config)
    }

    /// Add agents introduced by newer aic versions without replacing custom
    /// entries in an existing config file.
    fn ensure_builtin_agents(&mut self) {
        for (name, agent) in Config::default().agents {
            self.agents.entry(name).or_insert(agent);
        }
    }

    pub fn save_global(&self) -> Result<()> {
        let path = Self::global_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// Loads the global config and, if the repo has a `.aic/config.toml`,
    /// applies it on top. Inside a sandbox the result never asks for
    /// another one (see `in_sandbox`).
    pub fn load_merged(repo_root: &Path) -> Result<Config> {
        let mut config = Self::load_global()?;
        let repo_path = Self::repo_override_path(repo_root);
        if repo_path.exists() {
            let text = std::fs::read_to_string(&repo_path)
                .with_context(|| format!("reading {}", repo_path.display()))?;
            let overrides: ConfigOverrides = toml::from_str(&text)
                .with_context(|| format!("parsing {}", repo_path.display()))?;
            config.apply_overrides(overrides);
        }
        config.apply_sandbox_env(in_sandbox());
        Ok(config)
    }

    /// The outer container already isolates whatever `aic` starts inside
    /// it, and there's no runtime in there to nest another one.
    fn apply_sandbox_env(&mut self, in_sandbox: bool) {
        if in_sandbox {
            self.sandbox = SandboxKind::None;
        }
    }
}

/// Env var the sandbox sets on every container it starts.
pub const IN_SANDBOX_ENV: &str = "AIC_IN_SANDBOX";

/// Overrides the global config dir; the sandbox points it at the host's
/// config, mounted read-only.
pub const CONFIG_DIR_ENV: &str = "AIC_CONFIG_DIR";

/// Whether this `aic` runs inside an aic sandbox container.
pub fn in_sandbox() -> bool {
    std::env::var(IN_SANDBOX_ENV).is_ok_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_claude_gemini_and_codex_agents() {
        let config = Config::default();
        assert_eq!(config.agent("claude").unwrap().binary, "claude");
        assert_eq!(config.agent("gemini").unwrap().binary, "agy");
        assert_eq!(config.agent("codex").unwrap().binary, "codex");
        assert_eq!(config.mode, Mode::Manual);
        assert_eq!(config.max_rounds, 3);
    }

    #[test]
    fn sandbox_defaults_to_docker_and_parses_from_toml() {
        let config = Config::default();
        assert_eq!(config.sandbox, SandboxKind::Docker);
        assert_eq!(config.sandbox_image, "aic-sandbox");
        let parsed: Config = toml::from_str("sandbox = \"podman\"\nsandbox_env = [\"A\"]").unwrap();
        assert_eq!(parsed.sandbox, SandboxKind::Podman);
        assert_eq!(parsed.sandbox_env, vec!["A"]);
        let none: Config = toml::from_str("sandbox = \"none\"").unwrap();
        assert_eq!(none.sandbox, SandboxKind::None);
    }

    #[test]
    fn repo_override_can_disable_sandbox() {
        let mut config = Config::default();
        config.apply_overrides(ConfigOverrides {
            sandbox: Some(SandboxKind::None),
            ..Default::default()
        });
        assert_eq!(config.sandbox, SandboxKind::None);
        assert_eq!(config.sandbox_image, "aic-sandbox");
    }

    #[test]
    fn agent_binds_are_opt_in_and_can_be_overridden_per_repo() {
        let mut config: Config = toml::from_str("sandbox = 'podman'").unwrap();
        assert!(!config.sandbox_bind_agents);
        let overrides: ConfigOverrides = toml::from_str("sandbox_bind_agents = true").unwrap();
        config.apply_overrides(overrides);
        assert!(config.sandbox_bind_agents);
        config.apply_overrides(ConfigOverrides {
            sandbox_bind_agents: Some(false),
            ..Default::default()
        });
        assert!(!config.sandbox_bind_agents);
    }

    #[test]
    fn inside_a_sandbox_never_nests_another() {
        let mut config = Config::default();
        config.apply_sandbox_env(false);
        assert_eq!(config.sandbox, SandboxKind::Docker);
        config.apply_sandbox_env(true);
        assert_eq!(config.sandbox, SandboxKind::None);
    }

    #[test]
    fn unknown_agent_is_an_error() {
        let config = Config::default();
        assert!(config.agent("not-configured").is_err());
    }

    #[test]
    fn round_trips_through_toml() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.executor, config.executor);
        assert_eq!(parsed.agents.len(), config.agents.len());
    }

    #[test]
    fn older_config_gains_codex_without_overwriting_agents() {
        let mut config: Config = toml::from_str("[agents.claude]\nbinary = 'custom-claude'\n").unwrap();
        config.ensure_builtin_agents();
        assert_eq!(config.agent("claude").unwrap().binary, "custom-claude");
        assert_eq!(config.agent("codex").unwrap().binary, "codex");
    }

    #[test]
    fn overrides_only_touch_specified_fields() {
        let mut config = Config::default();
        let overrides = ConfigOverrides {
            mode: Some(Mode::Auto),
            max_rounds: Some(5),
            ..Default::default()
        };
        config.apply_overrides(overrides);
        assert_eq!(config.mode, Mode::Auto);
        assert_eq!(config.max_rounds, 5);
        // untouched fields keep their defaults
        assert_eq!(config.executor, "claude");
        assert_eq!(config.reviewer, "gemini");
    }

    #[test]
    fn overrides_can_add_a_new_agent_without_dropping_existing_ones() {
        let mut config = Config::default();
        let mut agents = HashMap::new();
        agents.insert(
            "local-llm".to_string(),
            AgentConfig {
                binary: "ollama".to_string(),
                model: "qwen".to_string(),
                effort: String::new(),
            },
        );
        config.apply_overrides(ConfigOverrides {
            agents: Some(agents),
            ..Default::default()
        });
        assert_eq!(config.agent("local-llm").unwrap().binary, "ollama");
        assert_eq!(config.agent("claude").unwrap().binary, "claude");
    }
}
