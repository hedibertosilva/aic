use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Draft,
    InReview,
    Approved,
}

impl Default for PlanStatus {
    fn default() -> Self {
        PlanStatus::Draft
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevStatus {
    NotStarted,
    Executing,
    Review,
    Done,
}

impl Default for DevStatus {
    fn default() -> Self {
        DevStatus::NotStarted
    }
}

/// One round of a review loop (plan review or code review).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    pub timestamp: DateTime<Utc>,
    pub agent: String,
    pub approved: bool,
    pub summary: String,
    pub report_path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanState {
    pub status: PlanStatus,
    pub path: String,
    pub rounds: Vec<Round>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevState {
    pub status: DevStatus,
    pub start_sha: Option<String>,
    pub execution_log_path: Option<String>,
    pub rounds: Vec<Round>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Feature {
    pub plan: PlanState,
    pub dev: DevState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub features: HashMap<String, Feature>,
}

impl State {
    pub fn path(repo_root: &Path) -> PathBuf {
        repo_root.join(".aic").join("state.json")
    }

    pub fn load(repo_root: &Path) -> Result<State> {
        let path = Self::path(repo_root);
        if !path.exists() {
            return Ok(State::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let path = Self::path(repo_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn feature(&self, slug: &str) -> Option<&Feature> {
        self.features.get(slug)
    }

    pub fn feature_mut(&mut self, slug: &str) -> &mut Feature {
        self.features.entry(slug.to_string()).or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_round() -> Round {
        Round {
            timestamp: Utc::now(),
            agent: "gemini".to_string(),
            approved: false,
            summary: "needs more detail on rollout".to_string(),
            report_path: ".aic/plans/demo/reviews/plan-r1.md".to_string(),
        }
    }

    #[test]
    fn loading_missing_state_returns_default() {
        let dir = tempdir().unwrap();
        let state = State::load(dir.path()).unwrap();
        assert!(state.features.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        let mut state = State::default();
        {
            let feature = state.feature_mut("demo");
            feature.plan.status = PlanStatus::InReview;
            feature.plan.path = ".aic/plans/demo/plan.md".to_string();
            feature.plan.rounds.push(sample_round());
        }
        state.save(dir.path()).unwrap();

        let loaded = State::load(dir.path()).unwrap();
        let feature = loaded.feature("demo").expect("feature should exist");
        assert_eq!(feature.plan.status, PlanStatus::InReview);
        assert_eq!(feature.plan.rounds.len(), 1);
        assert_eq!(feature.dev.status, DevStatus::NotStarted);
    }

    #[test]
    fn feature_mut_creates_entry_on_first_access() {
        let mut state = State::default();
        assert!(state.feature("new-feature").is_none());
        state.feature_mut("new-feature");
        assert!(state.feature("new-feature").is_some());
    }
}
