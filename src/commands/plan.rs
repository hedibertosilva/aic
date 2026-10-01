use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::agent::CliAgent;
use crate::config::{Config, Mode};
use crate::review::{run_review_loop, LoopOutcome, ReviewVerdict};
use crate::state::{PlanStatus, State};
use crate::templates::plan_template;

fn repo_relative(repo_root: &Path, path: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

pub fn new(repo_root: &Path, slug: &str, title: Option<String>) -> Result<()> {
    let plan_dir = repo_root.join(".aic").join("plans").join(slug);
    fs::create_dir_all(plan_dir.join("reviews")).context("creating plan directory")?;
    let plan_path = plan_dir.join("plan.md");
    if plan_path.exists() {
        bail!("{} already exists", plan_path.display());
    }

    let title = title.unwrap_or_else(|| slug.to_string());
    fs::write(&plan_path, plan_template(&title)).context("writing plan.md")?;

    let mut state = State::load(repo_root)?;
    {
        let feature = state.feature_mut(slug);
        feature.plan.status = PlanStatus::Draft;
        feature.plan.path = repo_relative(repo_root, &plan_path);
    }
    state.save(repo_root)?;

    println!("Created {}", plan_path.display());
    Ok(())
}

#[derive(Default)]
pub struct ReviewOverrides {
    pub reviewer: Option<String>,
    pub executor: Option<String>,
    pub mode: Option<Mode>,
    pub max_rounds: Option<u32>,
}

pub fn review(repo_root: &Path, slug: &str, overrides: ReviewOverrides) -> Result<()> {
    let config = Config::load_merged(repo_root)?;
    let mut state = State::load(repo_root)?;

    let plan_path = {
        let feature = state.feature(slug).with_context(|| {
            format!("no feature '{slug}' found; run `aic plan new {slug}` first")
        })?;
        repo_root.join(&feature.plan.path)
    };
    if !plan_path.exists() {
        bail!("plan file {} is missing", plan_path.display());
    }

    let reviewer_name = overrides.reviewer.unwrap_or_else(|| config.reviewer.clone());
    let executor_name = overrides.executor.unwrap_or_else(|| config.executor.clone());
    let mode = overrides.mode.unwrap_or(config.mode);
    let max_rounds = overrides.max_rounds.unwrap_or(config.max_rounds);

    let reviewer_cfg = config.agent(&reviewer_name)?;
    let reviewer = CliAgent::with_defaults(
        reviewer_name.clone(),
        reviewer_cfg.binary.clone(),
        reviewer_cfg.model.clone(),
        reviewer_cfg.effort.clone(),
    );
    let executor_cfg = config.agent(&executor_name)?;
    let executor = CliAgent::with_defaults(
        executor_name.clone(),
        executor_cfg.binary.clone(),
        executor_cfg.model.clone(),
        executor_cfg.effort.clone(),
    );

    let reviews_dir = plan_path.parent().unwrap().join("reviews");
    fs::create_dir_all(&reviews_dir)?;

    let existing_rounds = state.feature(slug).map_or(0, |f| f.plan.rounds.len() as u32);

    let slug_owned = slug.to_string();
    let plan_path_owned = plan_path.clone();
    let repo_root_owned = repo_root.to_path_buf();
    let reviews_dir_owned = reviews_dir.clone();
    let reviewer_name_for_reports = reviewer_name.clone();

    let outcome = run_review_loop(
        &reviewer,
        &executor,
        mode,
        max_rounds,
        config.skip_permissions,
        existing_rounds,
        repo_root,
        |content| review_prompt(&slug_owned, content),
        |issues| revise_prompt(&plan_path_owned, issues),
        move |round_number, verdict, _content| {
            let path = save_review_report(
                &reviews_dir_owned,
                round_number,
                &reviewer_name_for_reports,
                verdict,
            )?;
            Ok(repo_relative(&repo_root_owned, &path))
        },
        fs::read_to_string(&plan_path).context("reading plan.md")?,
        || fs::read_to_string(&plan_path).context("re-reading plan.md after revision"),
    )?;

    {
        let feature = state.feature_mut(slug);
        for review_round in &outcome.rounds {
            feature.plan.rounds.push(review_round.round.clone());
        }
        feature.plan.status = if outcome.approved {
            PlanStatus::Approved
        } else {
            PlanStatus::InReview
        };
    }
    state.save(repo_root)?;

    print_outcome(&outcome, existing_rounds);
    Ok(())
}

fn review_prompt(slug: &str, content: &str) -> String {
    format!(
        "You are reviewing a feature proposal (plan) named '{slug}' before any code is \
written. Read it carefully and decide whether it is ready to be executed as-is. \
Respond strictly per the provided JSON schema: 'approved' should be true only if there \
is nothing blocking, 'summary' a short overall assessment, 'issues' a list of concrete, \
actionable problems (empty if approved).\n\n---\n{content}\n---"
    )
}

fn revise_prompt(plan_path: &Path, issues: &[String]) -> String {
    let issues_list = issues
        .iter()
        .map(|issue| format!("- {issue}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Edit the plan at '{}' to address the following reviewer issues, keeping the \
rest of the document intact:\n{issues_list}",
        plan_path.display()
    )
}

fn save_review_report(
    reviews_dir: &Path,
    round_number: u32,
    reviewer_name: &str,
    verdict: &ReviewVerdict,
) -> Result<PathBuf> {
    let path = reviews_dir.join(format!("plan-r{round_number}.md"));
    let issues = if verdict.issues.is_empty() {
        "(none)".to_string()
    } else {
        verdict
            .issues
            .iter()
            .map(|issue| format!("- {issue}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body = format!(
        "# Review round {round_number} ({reviewer_name})\n\n\
**Approved:** {}\n\n\
## Summary\n\n{}\n\n\
## Issues\n\n{issues}\n",
        verdict.approved, verdict.summary,
    );
    fs::write(&path, body)?;
    Ok(path)
}

fn print_outcome(outcome: &LoopOutcome, existing_rounds: u32) {
    for (i, round) in outcome.rounds.iter().enumerate() {
        println!(
            "Round {}: approved={} — {}",
            existing_rounds + i as u32 + 1,
            round.verdict.approved,
            round.verdict.summary
        );
    }
    println!("{}", if outcome.approved { "Approved." } else { "Not approved yet." });
}
