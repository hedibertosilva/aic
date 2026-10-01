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
        Config {
            executor: "claude".to_string(),
            reviewer: "gemini".to_string(),
            mode: Mode::Manual,
            max_rounds: 3,
            permission_mode: "acceptEdits".to_string(),
            default_base_branch: String::new(),
            skip_permissions: false,
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
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
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
    /// applies it on top.
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
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_claude_and_gemini_agents() {
        let config = Config::default();
        assert_eq!(config.agent("claude").unwrap().binary, "claude");
        assert_eq!(config.agent("gemini").unwrap().binary, "agy");
        assert_eq!(config.mode, Mode::Manual);
        assert_eq!(config.max_rounds, 3);
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
