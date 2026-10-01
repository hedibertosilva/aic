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

    /// Re-reads `.aic/` and reconciles this state with what is on disk:
    /// features found under `plans/` or `dev/` are added, review rounds are
    /// rebuilt from the `PLAN-R*.md` / `CODE-R*.md` reports, and statuses
    /// follow the latest report (the log file alone never implies `Done`). Returns true if anything changed.
    pub fn refresh(&mut self, repo_root: &Path) -> bool {
        let before = serde_json::to_string(self).unwrap_or_default();
        let aic = repo_root.join(".aic");

        let mut slugs: Vec<String> = self.features.keys().cloned().collect();
        for sub in ["plans", "dev"] {
            for slug in subdirs(&aic.join(sub)) {
                if !slugs.contains(&slug) {
                    slugs.push(slug);
                }
            }
        }

        for slug in slugs {
            let plan_dir = aic.join("plans").join(&slug);
            let dev_dir = aic.join("dev").join(&slug);
            let plan_file = plan_dir.join("PLAN.md");
            let log_file = dev_dir.join("EXECUTION-LOG.md");
            let plan_rounds = scan_rounds(repo_root, &plan_dir.join("reviews"), "PLAN-R");
            let code_rounds = scan_rounds(repo_root, &dev_dir.join("reviews"), "CODE-R");

            let feature = self.feature_mut(&slug);
            if plan_file.exists() {
                feature.plan.path = relative(repo_root, &plan_file);
            }
            merge_rounds(&mut feature.plan.rounds, plan_rounds);
            feature.plan.status = match feature.plan.rounds.last() {
                Some(r) if r.approved => PlanStatus::Approved,
                Some(_) => PlanStatus::InReview,
                None => PlanStatus::Draft,
            };

            if log_file.exists() {
                feature.dev.execution_log_path = Some(relative(repo_root, &log_file));
            }
            merge_rounds(&mut feature.dev.rounds, code_rounds);
            match feature.dev.rounds.last() {
                Some(r) if r.approved => feature.dev.status = DevStatus::Done,
                Some(_) => feature.dev.status = DevStatus::Review,
                None => {}
            }
        }

        serde_json::to_string(self).unwrap_or_default() != before
    }

    pub fn feature(&self, slug: &str) -> Option<&Feature> {
        self.features.get(slug)
    }

    pub fn feature_mut(&mut self, slug: &str) -> &mut Feature {
        self.features.entry(slug.to_string()).or_default()
    }
}

fn relative(repo_root: &Path, path: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn subdirs(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// Parses `<prefix><N>.md` reports in `dir` into rounds, ordered by N.
fn scan_rounds(repo_root: &Path, dir: &Path, prefix: &str) -> Vec<Round> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(u32, Round)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(n) = name
            .strip_prefix(prefix)
            .and_then(|r| r.strip_suffix(".md"))
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let agent = text
            .lines()
            .next()
            .and_then(|l| l.rsplit_once('('))
            .and_then(|(_, r)| r.strip_suffix(')'))
            .unwrap_or("unknown")
            .to_string();
        let approved = text.lines().any(|l| l.trim() == "**Approved:** true");
        let summary = text
            .split("## Summary")
            .nth(1)
            .and_then(|r| r.split("## Issues").next())
            .unwrap_or("")
            .trim()
            .to_string();
        let timestamp = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now());
        found.push((
            n,
            Round {
                timestamp,
                agent,
                approved,
                summary,
                report_path: relative(repo_root, &path),
            },
        ));
    }
    found.sort_by_key(|(n, _)| *n);
    found.into_iter().map(|(_, r)| r).collect()
}

/// Makes `rounds` mirror the reports on disk, keeping the recorded entry
/// (with its original timestamp) whenever the report still exists.
fn merge_rounds(rounds: &mut Vec<Round>, on_disk: Vec<Round>) {
    let merged = on_disk
        .into_iter()
        .map(|disk| {
            rounds
                .iter()
                .find(|r| r.report_path == disk.report_path)
                .cloned()
                .map(|r| Round {
                    timestamp: r.timestamp,
                    ..disk.clone()
                })
                .unwrap_or(disk)
        })
        .collect();
    *rounds = merged;
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

    #[test]
    fn refresh_rebuilds_state_from_aic_dir() {
        let dir = tempdir().unwrap();
        let reviews = dir.path().join(".aic/plans/demo/reviews");
        std::fs::create_dir_all(&reviews).unwrap();
        std::fs::write(dir.path().join(".aic/plans/demo/PLAN.md"), "# demo").unwrap();
        std::fs::write(
            reviews.join("PLAN-R1.md"),
            "# Review round 1 (gemini)\n\n**Approved:** true\n\n## Summary\n\nok\n\n## Issues\n\n(none)\n",
        )
        .unwrap();

        let mut state = State::default();
        assert!(state.refresh(dir.path()));
        let feature = state.feature("demo").unwrap();
        assert_eq!(feature.plan.status, PlanStatus::Approved);
        assert_eq!(feature.plan.rounds.len(), 1);
        assert_eq!(feature.plan.rounds[0].agent, "gemini");

        std::fs::remove_file(reviews.join("PLAN-R1.md")).unwrap();
        assert!(state.refresh(dir.path()));
        assert_eq!(state.feature("demo").unwrap().plan.status, PlanStatus::Draft);
        assert!(!state.refresh(dir.path()));
    }
}
