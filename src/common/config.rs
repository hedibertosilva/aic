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

/// Settings of the plan -> dev -> review flow (`aic workflow ...`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkflowConfig {
    /// Agent that executes plans and fixes issues.
    pub executor: String,
    /// Agent that reviews plans and diffs.
    pub reviewer: String,
    pub mode: Mode,
    /// Cap on review rounds in `auto` mode.
    pub max_rounds: u32,
    pub default_base_branch: String,
    /// Extra patterns (on top of the built-in lockfile list) to drop from
    /// the diff sent to the reviewer in `aic workflow dev review`.
    pub git_diff_exclude_patterns: Vec<String>,
    /// Patterns that re-include a path even if an exclusion above (built-in,
    /// config, `.gitignore`, `linguist-generated`) dropped it.
    pub git_diff_include_patterns: Vec<String>,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        WorkflowConfig {
            executor: "claude".to_string(),
            reviewer: "gemini".to_string(),
            mode: Mode::Manual,
            max_rounds: 3,
            default_base_branch: String::new(),
            git_diff_exclude_patterns: Vec::new(),
            git_diff_include_patterns: Vec::new(),
        }
    }
}

/// How agents are allowed to act; shared by the chat and the workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    /// `acceptEdits` or `plan`, for runs that may edit files.
    pub mode: String,
    /// Opt-in: auto-approve every tool permission request for the
    /// configured agents (`--dangerously-skip-permissions`). Off by
    /// default. Headless runs can't answer an interactive permission
    /// prompt, so without this, an agent that tries to read a file or run a
    /// command outside what `mode` already allows gets silently denied and
    /// returns an empty response. Only turn this on if you trust the
    /// configured agents to run unattended on this repo.
    pub skip: bool,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        PermissionsConfig { mode: "acceptEdits".to_string(), skip: false }
    }
}

/// Container isolation of every agent invocation (see `sandbox.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Container runtime. `none` keeps the pre-sandbox behavior of running
    /// agents directly on the host.
    pub runtime: SandboxKind,
    /// Image the sandbox runs; `aic sandbox build` produces the default.
    pub image: String,
    /// Bind the host's agy/Codex executables read-only instead of baking
    /// them into the image. Claude remains installed in the image.
    pub bind_agents: bool,
    /// Extra `host:container[:ro]` binds (caches, toolchains).
    pub mounts: Vec<String>,
    /// Extra host roots from which conversation-scoped paths may be added.
    /// The parent of the repository opened by aic is always allowed.
    pub path_roots: Vec<String>,
    /// Names of host environment variables forwarded into the sandbox.
    pub env: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            runtime: SandboxKind::Docker,
            image: DEFAULT_SANDBOX_IMAGE.to_string(),
            bind_agents: false,
            mounts: Vec::new(),
            path_roots: Vec::new(),
            env: Vec::new(),
        }
    }
}

/// Settings of the web chat (`aic chat`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatConfig {
    /// Explicit agent preference for new chats. Empty requires a user choice.
    pub default_agent: String,
    /// Serialize writers from different conversations that share a worktree.
    pub serialize_worktree_writes: bool,
    /// Maximum implementation/review rounds allowed by Loop Engineering.
    pub loop_max_rounds: u32,
}

impl Default for ChatConfig {
    fn default() -> Self {
        ChatConfig {
            default_agent: String::new(),
            serialize_worktree_writes: true,
            loop_max_rounds: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub workflow: WorkflowConfig,
    pub permissions: PermissionsConfig,
    pub sandbox: SandboxConfig,
    pub chat: ChatConfig,
    pub agents: HashMap<String, AgentConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let mut agents = HashMap::new();
        for (name, binary) in [("claude", "claude"), ("gemini", "agy"), ("codex", "codex")] {
            agents.insert(
                name.to_string(),
                AgentConfig {
                    binary: binary.to_string(),
                    model: String::new(),
                    effort: String::new(),
                },
            );
        }
        Config {
            workflow: WorkflowConfig::default(),
            permissions: PermissionsConfig::default(),
            sandbox: SandboxConfig::default(),
            chat: ChatConfig::default(),
            agents,
        }
    }
}

/// Keys that used to live at the top level of the config file, with the
/// section and key they moved to.
const LEGACY_KEYS: &[(&str, &str, &str)] = &[
    ("executor", "workflow", "executor"),
    ("reviewer", "workflow", "reviewer"),
    ("mode", "workflow", "mode"),
    ("max_rounds", "workflow", "max_rounds"),
    ("default_base_branch", "workflow", "default_base_branch"),
    ("git_diff_exclude_patterns", "workflow", "git_diff_exclude_patterns"),
    ("git_diff_include_patterns", "workflow", "git_diff_include_patterns"),
    ("permission_mode", "permissions", "mode"),
    ("skip_permissions", "permissions", "skip"),
    ("sandbox", "sandbox", "runtime"),
    ("sandbox_image", "sandbox", "image"),
    ("sandbox_bind_agents", "sandbox", "bind_agents"),
    ("sandbox_mounts", "sandbox", "mounts"),
    ("sandbox_path_roots", "sandbox", "path_roots"),
    ("sandbox_env", "sandbox", "env"),
];

/// Moves legacy top-level keys into their sections, so config files (and
/// repo overrides) written by older versions keep working. A key already
/// present in its section wins.
fn migrate_legacy_keys(table: &mut toml::Table) {
    for (old, section, key) in LEGACY_KEYS {
        // The new `sandbox` is a table; only a plain value is the old key.
        if table.get(*old).is_none_or(|value| value.is_table()) {
            continue;
        }
        let Some(value) = table.remove(*old) else { continue };
        if !table.contains_key(*section) {
            table.insert(section.to_string(), toml::Value::Table(toml::Table::new()));
        }
        if let Some(toml::Value::Table(target)) = table.get_mut(*section) {
            if !target.contains_key(*key) {
                target.insert(key.to_string(), value);
            }
        }
    }
}

fn parse_config_table(text: &str) -> Result<toml::Table> {
    let mut table: toml::Table = toml::from_str(text)?;
    migrate_legacy_keys(&mut table);
    Ok(table)
}

/// Deep-merges `over` into `base`: tables merge key by key, anything else
/// (including lists) is replaced.
fn merge_tables(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        let both_tables = matches!(
            (base.get(&key), &value),
            (Some(toml::Value::Table(_)), toml::Value::Table(_))
        );
        if both_tables {
            if let (Some(toml::Value::Table(target)), toml::Value::Table(over)) = (base.get_mut(&key), value) {
                merge_tables(target, over);
            }
        } else {
            base.insert(key, value);
        }
    }
}

impl Config {
    /// Applies a repo-level `.aic/config.toml` (same schema as the global
    /// file, any subset of it) on top of this config.
    fn apply_overrides(&mut self, overrides: toml::Table) -> Result<()> {
        let mut merged = toml::Value::try_from(&*self)?;
        if let toml::Value::Table(base) = &mut merged {
            merge_tables(base, overrides);
        }
        *self = merged.try_into()?;
        Ok(())
    }

    pub fn agent(&self, name: &str) -> Result<&AgentConfig> {
        self.agents
            .get(name)
            .with_context(|| format!("no agent named '{name}' configured (check `aic config show`)"))
    }

    /// Preserve the explicit preference, including unavailable names; never fall back.
    pub fn chat_default_agent(&self) -> &str {
        &self.chat.default_agent
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
        let mut config = Self::from_toml(&text).with_context(|| format!("parsing {}", path.display()))?;
        config.ensure_builtin_agents();
        Ok(config)
    }

    /// Parses a config file's text, accepting the legacy flat layout too.
    fn from_toml(text: &str) -> Result<Config> {
        Ok(toml::Value::Table(parse_config_table(text)?).try_into()?)
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
            let overrides = parse_config_table(&text)
                .with_context(|| format!("parsing {}", repo_path.display()))?;
            config
                .apply_overrides(overrides)
                .with_context(|| format!("applying {}", repo_path.display()))?;
        }
        config.apply_sandbox_env(in_sandbox());
        Ok(config)
    }

    /// The outer container already isolates whatever `aic` starts inside
    /// it, and there's no runtime in there to nest another one.
    fn apply_sandbox_env(&mut self, in_sandbox: bool) {
        if in_sandbox {
            self.sandbox.runtime = SandboxKind::None;
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

    fn overrides(text: &str) -> toml::Table {
        parse_config_table(text).unwrap()
    }

    #[test]
    fn default_config_has_claude_gemini_and_codex_agents() {
        let config = Config::default();
        assert_eq!(config.agent("claude").unwrap().binary, "claude");
        assert_eq!(config.agent("gemini").unwrap().binary, "agy");
        assert_eq!(config.agent("codex").unwrap().binary, "codex");
        assert_eq!(config.workflow.mode, Mode::Manual);
        assert_eq!(config.workflow.max_rounds, 3);
    }

    #[test]
    fn sandbox_defaults_to_docker_and_parses_from_toml() {
        let config = Config::default();
        assert_eq!(config.sandbox.runtime, SandboxKind::Docker);
        assert_eq!(config.sandbox.image, "aic-sandbox");
        let parsed = Config::from_toml("[sandbox]\nruntime = \"podman\"\nenv = [\"A\"]").unwrap();
        assert_eq!(parsed.sandbox.runtime, SandboxKind::Podman);
        assert_eq!(parsed.sandbox.env, vec!["A"]);
        let none = Config::from_toml("[sandbox]\nruntime = \"none\"").unwrap();
        assert_eq!(none.sandbox.runtime, SandboxKind::None);
    }

    #[test]
    fn legacy_flat_config_is_read_into_sections() {
        let config = Config::from_toml(
            "executor = 'codex'\nreviewer = 'claude'\nmode = 'auto'\nmax_rounds = 5\n\
             permission_mode = 'plan'\nskip_permissions = true\n\
             sandbox = 'podman'\nsandbox_image = 'img'\nsandbox_bind_agents = true\n\
             sandbox_mounts = ['/a:/b']\nsandbox_path_roots = ['/work']\nsandbox_env = ['K']\n\
             default_base_branch = 'dev'\ngit_diff_exclude_patterns = ['x']\n\
             [agents.claude]\nbinary = 'custom-claude'\n",
        )
        .unwrap();
        assert_eq!(config.workflow.executor, "codex");
        assert_eq!(config.workflow.reviewer, "claude");
        assert_eq!(config.workflow.mode, Mode::Auto);
        assert_eq!(config.workflow.max_rounds, 5);
        assert_eq!(config.workflow.default_base_branch, "dev");
        assert_eq!(config.workflow.git_diff_exclude_patterns, vec!["x"]);
        assert_eq!(config.permissions.mode, "plan");
        assert!(config.permissions.skip);
        assert_eq!(config.sandbox.runtime, SandboxKind::Podman);
        assert_eq!(config.sandbox.image, "img");
        assert!(config.sandbox.bind_agents);
        assert_eq!(config.sandbox.mounts, vec!["/a:/b"]);
        assert_eq!(config.sandbox.path_roots, vec!["/work"]);
        assert_eq!(config.sandbox.env, vec!["K"]);
        assert_eq!(config.agent("claude").unwrap().binary, "custom-claude");
    }

    #[test]
    fn a_key_in_its_section_wins_over_the_legacy_one() {
        let config = Config::from_toml("executor = 'old'\n[workflow]\nexecutor = 'new'\n").unwrap();
        assert_eq!(config.workflow.executor, "new");
    }

    #[test]
    fn repo_override_can_disable_sandbox() {
        let mut config = Config::default();
        config.apply_overrides(overrides("[sandbox]\nruntime = 'none'")).unwrap();
        assert_eq!(config.sandbox.runtime, SandboxKind::None);
        assert_eq!(config.sandbox.image, "aic-sandbox");
    }

    #[test]
    fn legacy_repo_override_still_applies() {
        let mut config = Config::default();
        config.apply_overrides(overrides("sandbox = 'none'\nmode = 'auto'")).unwrap();
        assert_eq!(config.sandbox.runtime, SandboxKind::None);
        assert_eq!(config.workflow.mode, Mode::Auto);
    }

    #[test]
    fn agent_binds_are_opt_in_and_can_be_overridden_per_repo() {
        let mut config = Config::from_toml("[sandbox]\nruntime = 'podman'").unwrap();
        assert!(!config.sandbox.bind_agents);
        config.apply_overrides(overrides("[sandbox]\nbind_agents = true")).unwrap();
        assert!(config.sandbox.bind_agents);
        config.apply_overrides(overrides("[sandbox]\nbind_agents = false")).unwrap();
        assert!(!config.sandbox.bind_agents);
    }

    #[test]
    fn inside_a_sandbox_never_nests_another() {
        let mut config = Config::default();
        config.apply_sandbox_env(false);
        assert_eq!(config.sandbox.runtime, SandboxKind::Docker);
        config.apply_sandbox_env(true);
        assert_eq!(config.sandbox.runtime, SandboxKind::None);
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
        let parsed = Config::from_toml(&text).unwrap();
        assert_eq!(parsed.workflow.executor, config.workflow.executor);
        assert_eq!(parsed.sandbox.runtime, config.sandbox.runtime);
        assert_eq!(parsed.agents.len(), config.agents.len());
    }

    #[test]
    fn older_config_gains_codex_without_overwriting_agents() {
        let mut config = Config::from_toml("[agents.claude]\nbinary = 'custom-claude'\n").unwrap();
        config.ensure_builtin_agents();
        assert_eq!(config.agent("claude").unwrap().binary, "custom-claude");
        assert_eq!(config.agent("codex").unwrap().binary, "codex");
    }

    #[test]
    fn overrides_only_touch_specified_fields() {
        let mut config = Config::default();
        config.apply_overrides(overrides("[workflow]\nmode = 'auto'\nmax_rounds = 5")).unwrap();
        assert_eq!(config.workflow.mode, Mode::Auto);
        assert_eq!(config.workflow.max_rounds, 5);
        // untouched fields keep their defaults
        assert_eq!(config.workflow.executor, "claude");
        assert_eq!(config.workflow.reviewer, "gemini");
    }

    #[test]
    fn overrides_can_add_a_new_agent_without_dropping_existing_ones() {
        let mut config = Config::default();
        config
            .apply_overrides(overrides("[agents.local-llm]\nbinary = 'ollama'\nmodel = 'qwen'"))
            .unwrap();
        assert_eq!(config.agent("local-llm").unwrap().binary, "ollama");
        assert_eq!(config.agent("claude").unwrap().binary, "claude");
    }

    #[test]
    fn override_of_one_agent_field_keeps_the_rest_of_that_agent() {
        let mut config = Config::default();
        config.apply_overrides(overrides("[agents.claude]\nmodel = 'opus'")).unwrap();
        assert_eq!(config.agent("claude").unwrap().model, "opus");
        assert_eq!(config.agent("claude").unwrap().binary, "claude");
    }

    #[test]
    fn chat_default_agent_never_inherits_workflow() {
        let mut config = Config::default();
        assert_eq!(config.chat_default_agent(), "");
        config.chat.default_agent = "codex".into();
        assert_eq!(config.chat_default_agent(), "codex");
        config.chat.default_agent = "not-configured".into();
        assert_eq!(config.chat_default_agent(), "not-configured");
    }

    #[test]
    fn worktree_writer_serialization_is_safe_by_default() {
        assert!(Config::default().chat.serialize_worktree_writes);
        assert_eq!(Config::default().chat.loop_max_rounds, 3);
        let older = Config::from_toml("[chat]\ndefault_agent = 'codex'").unwrap();
        assert!(older.chat.serialize_worktree_writes);
        assert_eq!(older.chat.loop_max_rounds, 3);
        let opted_out = Config::from_toml(
            "[chat]\ndefault_agent = 'codex'\nserialize_worktree_writes = false",
        )
        .unwrap();
        assert!(!opted_out.chat.serialize_worktree_writes);
    }
}
