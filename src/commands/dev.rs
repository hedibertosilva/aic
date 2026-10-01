use std::fs;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use chrono::Utc;

use crate::agent::{CliAgent, RunOpts};
use crate::cli::{ExecuteModeArg, TargetArg};
use crate::config::{Config, Mode};
use crate::diff_filter;
use crate::git;
use crate::review::{run_review_loop, LoopOutcome, ReviewVerdict};
use crate::state::{DevStatus, State};

fn repo_relative(repo_root: &Path, path: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[derive(Default)]
pub struct ExecuteOverrides {
    pub mode: Option<ExecuteModeArg>,
    pub executor: Option<String>,
    pub permission_mode: Option<String>,
}

/// Runs the executor agent against an approved plan.
///
/// In `ExecuteModeArg::Interactive` (the default) this replaces the current
/// process with the executor's (`exec()`), handing it the terminal directly.
/// Nothing in `aic` runs after that — `dev review` is always a separate,
/// later invocation.
pub fn execute(repo_root: &Path, slug: &str, overrides: ExecuteOverrides) -> Result<()> {
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

    let executor_name = overrides.executor.unwrap_or_else(|| config.executor.clone());
    let executor_cfg = config.agent(&executor_name)?;
    let executor_binary = executor_cfg.binary.clone();
    let executor_model = executor_cfg.model.clone();
    let executor_effort = executor_cfg.effort.clone();
    let permission_mode = overrides
        .permission_mode
        .unwrap_or_else(|| config.permission_mode.clone());
    let mode = overrides.mode.unwrap_or(ExecuteModeArg::Interactive);

    let start_sha = git::head_sha(repo_root)?;
    {
        let feature = state.feature_mut(slug);
        feature.dev.start_sha = Some(start_sha);
        feature.dev.status = DevStatus::Executing;
    }
    state.save(repo_root)?;

    let prompt = format!(
        "Execute the plan described in {}. Follow it closely; if you need to deviate, \
explain why.",
        plan_path.display()
    );

    match mode {
        ExecuteModeArg::Interactive => {
            let mut cmd = Command::new(&executor_binary);
            // `claude` takes the initial prompt as a bare positional
            // argument; `agy` rejects that and requires
            // `-i`/`--prompt-interactive` to start an interactive session
            // with an initial message.
            if binary_stem(&executor_binary) == "agy" {
                cmd.arg("-i").arg(&prompt);
            } else {
                cmd.arg(&prompt);
            }
            apply_permission_flag(&mut cmd, &executor_binary, &permission_mode);
            cmd.current_dir(repo_root);
            let err = cmd.exec();
            bail!("failed to exec '{executor_binary}': {err}");
        }
        ExecuteModeArg::Headless => {
            let agent = CliAgent::with_defaults(executor_name, executor_binary, executor_model, executor_effort);
            let result = agent.run(&RunOpts {
                prompt,
                permission: Some(permission_mode),
                skip_permissions: config.skip_permissions,
                cwd: Some(repo_root.to_path_buf()),
                ..Default::default()
            })?;

            let log_dir = repo_root.join(".aic").join("dev").join(slug);
            fs::create_dir_all(&log_dir)?;
            let log_path = log_dir.join("EXECUTION-LOG.md");
            append_execution_log(&log_path, result.success, &result.text)?;

            if !result.success {
                bail!("executor '{}' did not complete successfully: {}", agent.name, result.text);
            }

            let feature = state.feature_mut(slug);
            feature.dev.status = DevStatus::Done;
            feature.dev.execution_log_path = Some(repo_relative(repo_root, &log_path));
            state.save(repo_root)?;

            println!("Executed. Log: {}", log_path.display());
            Ok(())
        }
    }
}

/// The file name of `binary`, stripped of any directory and extension —
/// e.g. "/usr/local/bin/agy" and "agy" both yield "agy".
fn binary_stem(binary: &str) -> &str {
    Path::new(binary)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(binary)
}

fn apply_permission_flag(cmd: &mut Command, binary: &str, permission: &str) {
    let is_plan = permission.eq_ignore_ascii_case("plan");
    if binary_stem(binary) == "agy" {
        cmd.arg("--mode").arg(if is_plan { "plan" } else { "accept-edits" });
    } else {
        cmd.arg("--permission-mode")
            .arg(if is_plan { "plan" } else { "acceptEdits" });
    }
}

fn append_execution_log(log_path: &Path, success: bool, text: &str) -> Result<()> {
    let entry = format!(
        "## {}\n\n**Success:** {success}\n\n{text}\n\n---\n\n",
        Utc::now().to_rfc3339()
    );
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;
    file.write_all(entry.as_bytes())?;
    Ok(())
}

#[derive(Default)]
pub struct ReviewOverrides {
    pub target: Option<TargetArg>,
    pub range: Option<String>,
    pub reviewer: Option<String>,
    pub executor: Option<String>,
    pub mode: Option<Mode>,
    pub max_rounds: Option<u32>,
    pub no_filter: bool,
}

pub fn review(repo_root: &Path, slug: Option<&str>, overrides: ReviewOverrides) -> Result<()> {
    let config = Config::load_merged(repo_root)?;
    let mut state = State::load(repo_root)?;

    let no_filter = overrides.no_filter;
    let (target_desc, diff_args, anchor) = resolve_diff(repo_root, &config, &state, slug, &overrides)?;
    let diff = diff_filter::filtered_diff(repo_root, &config, &diff_args, no_filter)?;
    if diff.trim().is_empty() {
        if !no_filter && !git::diff_paths_unfiltered(repo_root, &diff_args)?.trim().is_empty() {
            println!("Nothing to review: all changes in {target_desc} were filtered out (use --no-filter to include them).");
            return Ok(());
        }
        println!("Nothing to review ({target_desc} is empty).");
        return Ok(());
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

    let (reviews_dir, is_adhoc) = match slug {
        Some(slug) => (repo_root.join(".aic").join("dev").join(slug).join("reviews"), false),
        None => (repo_root.join(".aic").join("adhoc").join("reviews"), true),
    };
    fs::create_dir_all(&reviews_dir)?;

    let existing_rounds = slug
        .and_then(|s| state.feature(s))
        .map_or(0, |f| f.dev.rounds.len() as u32);

    let repo_root_owned = repo_root.to_path_buf();
    let reviews_dir_owned = reviews_dir.clone();
    let reviewer_name_for_reports = reviewer_name.clone();
    let target_desc_owned = target_desc.clone();

    let repo_root_for_refresh = repo_root.to_path_buf();
    let config_for_refresh = config.clone();

    let outcome = run_review_loop(
        &reviewer,
        &executor,
        mode,
        max_rounds,
        config.skip_permissions,
        existing_rounds,
        repo_root,
        |content| review_prompt(&target_desc_owned, content),
        |issues| revise_prompt(issues),
        move |round_number, verdict, _content| {
            let path = save_review_report(
                &reviews_dir_owned,
                round_number,
                &reviewer_name_for_reports,
                verdict,
                is_adhoc,
            )?;
            Ok(repo_relative(&repo_root_owned, &path))
        },
        diff,
        // Re-run the filtered diff so the next round sees the executor's edits.
        move || {
            let args = vec![anchor.clone().unwrap_or_else(|| "--staged".to_string())];
            diff_filter::filtered_diff(&repo_root_for_refresh, &config_for_refresh, &args, no_filter)
        },
    )?;

    if let Some(slug) = slug {
        let feature = state.feature_mut(slug);
        for review_round in &outcome.rounds {
            feature.dev.rounds.push(review_round.round.clone());
        }
        feature.dev.status = if outcome.approved {
            DevStatus::Done
        } else {
            DevStatus::Review
        };
        state.save(repo_root)?;
    }

    print_outcome(&outcome, existing_rounds);
    Ok(())
}

/// Figures out which diff to review (as `git diff` arguments), in order: an explicit `--target`;
/// otherwise staged changes if there are any; otherwise commits since
/// `dev execute`'s recorded start sha; otherwise a diff against the
/// auto-detected default branch.
///
/// Also returns a refresh anchor: a single ref such that `git diff <ref>`
/// reproduces "everything this review's target covers" against whatever
/// the working tree looks like *right now* — staged, unstaged, and any new
/// commits. Re-running `resolve_diff` after an auto-mode executor run would
/// recompute e.g. `has_staged_changes` from scratch and could silently
/// switch targets or lose history; diffing straight from the anchor avoids
/// that. `None` means there's no fixed commit to anchor on yet (a `staged`
/// review in a repo with no commits at all).
fn resolve_diff(
    repo_root: &Path,
    config: &Config,
    state: &State,
    slug: Option<&str>,
    overrides: &ReviewOverrides,
) -> Result<(String, Vec<String>, Option<String>)> {
    let start_sha = slug.and_then(|s| state.feature(s)).and_then(|f| f.dev.start_sha.clone());

    let target = overrides.target.unwrap_or_else(|| {
        if git::has_staged_changes(repo_root).unwrap_or(false) {
            TargetArg::Staged
        } else if start_sha.is_some() {
            TargetArg::Commits
        } else {
            TargetArg::Branch
        }
    });

    match target {
        TargetArg::Staged => {
            let anchor = git::head_sha(repo_root).ok();
            Ok(("staged changes".to_string(), vec!["--staged".to_string()], anchor))
        }
        TargetArg::Commits => {
            if let Some(range) = &overrides.range {
                let (from, to) = range
                    .split_once("..")
                    .with_context(|| format!("--range '{range}' isn't in '<from>..<to>' form"))?;
                return Ok((
                    format!("commits {range}"),
                    vec![format!("{from}..{to}")],
                    Some(from.to_string()),
                ));
            }
            let start = start_sha
                .context("no start sha recorded for this feature; run `aic dev execute` first, or pass --range")?;
            Ok((
                format!("commits since {start}"),
                vec![format!("{start}..HEAD")],
                Some(start),
            ))
        }
        TargetArg::Branch => {
            let branch = git::default_branch(repo_root, Some(&config.default_base_branch))?;
            let anchor = git::merge_base(repo_root, &branch)?;
            Ok((
                format!("diff against {branch}"),
                vec![format!("{branch}...HEAD")],
                Some(anchor),
            ))
        }
    }
}

fn review_prompt(target_desc: &str, diff: &str) -> String {
    format!(
        "You are reviewing a code change ({target_desc}) for correctness, security and \
quality issues. Respond strictly per the provided JSON schema: 'approved' true only if \
there is nothing blocking, 'summary' a short overall assessment, 'issues' a list of \
concrete, actionable problems (empty if approved).\n\n```diff\n{diff}\n```"
    )
}

fn revise_prompt(issues: &[String]) -> String {
    let issues_list = issues
        .iter()
        .map(|issue| format!("- {issue}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("Address the following code review issues in the working tree:\n{issues_list}")
}

fn save_review_report(
    reviews_dir: &Path,
    round_number: u32,
    reviewer_name: &str,
    verdict: &ReviewVerdict,
    is_adhoc: bool,
) -> Result<PathBuf> {
    let filename = if is_adhoc {
        format!("{}-R{round_number}.md", Utc::now().format("%Y%m%dT%H%M%S"))
    } else {
        format!("CODE-R{round_number}.md")
    };
    let path = reviews_dir.join(filename);
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
        "# Code review round {round_number} ({reviewer_name})\n\n\
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
