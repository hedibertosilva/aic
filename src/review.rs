use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::Deserialize;

use crate::agent::{AgentRunResult, RunOpts};
use crate::config::Mode;
use crate::state::Round;

/// Lets `run_review_loop` work against a real `CliAgent` or a fake one in
/// tests, without pulling `std::process::Command` into unit tests.
pub trait AgentRunner {
    fn name(&self) -> &str;
    fn run(&self, opts: &RunOpts) -> Result<AgentRunResult>;
}

impl AgentRunner for crate::agent::CliAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self, opts: &RunOpts) -> Result<AgentRunResult> {
        crate::agent::CliAgent::run(self, opts)
    }
}

/// Shape every reviewer is asked (via `--json-schema`) to answer in.
pub const REVIEW_JSON_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "approved": {"type": "boolean"},
    "summary": {"type": "string"},
    "issues": {"type": "array", "items": {"type": "string"}}
  },
  "required": ["approved", "summary", "issues"]
}"#;

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewVerdict {
    pub approved: bool,
    pub summary: String,
    #[serde(default)]
    pub issues: Vec<String>,
}

/// One completed round: the reviewer's verdict, plus a `Round` ready to be
/// pushed onto `state.json`.
pub struct ReviewRound {
    pub verdict: ReviewVerdict,
    pub round: Round,
}

pub struct LoopOutcome {
    pub rounds: Vec<ReviewRound>,
    pub approved: bool,
}

/// Runs `reviewer` against `content` (a plan's text, or a diff) using
/// `review_prompt`. In `Mode::Auto`, if the reviewer requests changes, asks
/// `executor` to address them (via `revise_prompt`, which gets the issues
/// appended) and re-reviews, up to `max_rounds` times. A non-success agent
/// run (error, timeout) aborts immediately — it is never treated as "changes
/// requested".
pub fn run_review_loop(
    reviewer: &dyn AgentRunner,
    executor: &dyn AgentRunner,
    mode: Mode,
    max_rounds: u32,
    skip_permissions: bool,
    // How many rounds this feature already has on record (e.g.
    // `feature.plan.rounds.len()`), so report filenames and round numbers in
    // a fresh invocation continue from history instead of overwriting
    // `*-r1.md` every time `manual` mode is re-run.
    existing_rounds: u32,
    // Working directory for both agents' subprocesses — the repo root,
    // regardless of which subdirectory `aic` itself was invoked from.
    cwd: &std::path::Path,
    review_prompt: impl Fn(&str) -> String,
    revise_prompt: impl Fn(&[String]) -> String,
    mut save_report: impl FnMut(u32, &ReviewVerdict, &str) -> Result<String>,
    initial_content: String,
    mut refresh_content: impl FnMut() -> Result<String>,
) -> Result<LoopOutcome> {
    let mut rounds = Vec::new();
    let mut content = initial_content;

    let effective_max_rounds = match mode {
        Mode::Manual => 1,
        Mode::Auto => max_rounds.max(1),
    };

    for offset in 1..=effective_max_rounds {
        let round_number = existing_rounds + offset;
        let run_result = reviewer
            .run(&RunOpts {
                prompt: review_prompt(&content),
                json_schema: Some(REVIEW_JSON_SCHEMA.to_string()),
                skip_permissions,
                cwd: Some(cwd.to_path_buf()),
                ..Default::default()
            })
            .with_context(|| format!("reviewer '{}' failed on round {round_number}", reviewer.name()))?;

        if !run_result.success {
            bail!(
                "reviewer '{}' did not complete successfully on round {round_number}: {}",
                reviewer.name(),
                run_result.text
            );
        }

        let structured = run_result.structured_output.with_context(|| {
            if skip_permissions {
                format!(
                    "reviewer '{}' succeeded but returned no structured output",
                    reviewer.name()
                )
            } else {
                format!(
                    "reviewer '{}' succeeded but returned no structured output (empty response \
usually means a tool call was silently denied in headless mode — set \
`skip_permissions = true` in config if you trust this agent to run unattended, or check \
`aic config show`)",
                    reviewer.name()
                )
            }
        })?;
        let verdict: ReviewVerdict = serde_json::from_value(structured).with_context(|| {
            format!(
                "reviewer '{}' structured output didn't match the expected review schema",
                reviewer.name()
            )
        })?;

        let report_path = save_report(round_number, &verdict, &content)?;
        let round = Round {
            timestamp: Utc::now(),
            agent: reviewer.name().to_string(),
            approved: verdict.approved,
            summary: verdict.summary.clone(),
            report_path,
        };
        let approved = verdict.approved;
        rounds.push(ReviewRound { verdict, round });

        if approved {
            return Ok(LoopOutcome {
                rounds,
                approved: true,
            });
        }

        if mode == Mode::Manual || offset == effective_max_rounds {
            return Ok(LoopOutcome {
                rounds,
                approved: false,
            });
        }

        let issues = &rounds.last().unwrap().verdict.issues;
        let fix_result = executor
            .run(&RunOpts {
                prompt: revise_prompt(issues),
                permission: Some("acceptEdits".to_string()),
                skip_permissions,
                cwd: Some(cwd.to_path_buf()),
                ..Default::default()
            })
            .with_context(|| format!("executor '{}' failed on round {round_number}", executor.name()))?;

        if !fix_result.success {
            bail!(
                "executor '{}' did not complete successfully while addressing round {round_number} issues: {}",
                executor.name(),
                fix_result.text
            );
        }

        content = refresh_content()?;
    }

    Ok(LoopOutcome {
        rounds,
        approved: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeAgent {
        name: String,
        responses: RefCell<Vec<AgentRunResult>>,
    }

    impl FakeAgent {
        fn new(name: &str, responses: Vec<AgentRunResult>) -> Self {
            FakeAgent {
                name: name.to_string(),
                responses: RefCell::new(responses),
            }
        }
    }

    impl AgentRunner for FakeAgent {
        fn name(&self) -> &str {
            &self.name
        }

        fn run(&self, _opts: &RunOpts) -> Result<AgentRunResult> {
            let mut responses = self.responses.borrow_mut();
            if responses.is_empty() {
                panic!("FakeAgent '{}' called more times than it has canned responses", self.name);
            }
            Ok(responses.remove(0))
        }
    }

    fn success(structured: serde_json::Value) -> AgentRunResult {
        AgentRunResult {
            success: true,
            text: "ok".to_string(),
            structured_output: Some(structured),
        }
    }

    fn failure(text: &str) -> AgentRunResult {
        AgentRunResult {
            success: false,
            text: text.to_string(),
            structured_output: None,
        }
    }

    fn verdict(approved: bool, issues: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "approved": approved,
            "summary": if approved { "looks good" } else { "needs work" },
            "issues": issues,
        })
    }

    #[test]
    fn manual_mode_stops_after_one_round_even_if_not_approved() {
        let reviewer = FakeAgent::new("gemini", vec![success(verdict(false, &["fix the title"]))]);
        let executor = FakeAgent::new("claude", vec![]);

        let outcome = run_review_loop(
            &reviewer,
            &executor,
            Mode::Manual,
            3,
            false,
            0,
            std::path::Path::new("."),
            |content| format!("review: {content}"),
            |issues| format!("fix: {issues:?}"),
            |_round, _verdict, _content| Ok("report.md".to_string()),
            "plan v1".to_string(),
            || Ok("plan v2".to_string()),
        )
        .unwrap();

        assert!(!outcome.approved);
        assert_eq!(outcome.rounds.len(), 1);
    }

    #[test]
    fn existing_rounds_offsets_the_round_number_passed_to_save_report() {
        let reviewer = FakeAgent::new("gemini", vec![success(verdict(true, &[]))]);
        let executor = FakeAgent::new("claude", vec![]);
        let seen_round_numbers = std::cell::RefCell::new(Vec::new());

        run_review_loop(
            &reviewer,
            &executor,
            Mode::Manual,
            3,
            false,
            2, // two rounds already on record
            std::path::Path::new("."),
            |content| format!("review: {content}"),
            |issues| format!("fix: {issues:?}"),
            |round_number, _verdict, _content| {
                seen_round_numbers.borrow_mut().push(round_number);
                Ok("report.md".to_string())
            },
            "plan v1".to_string(),
            || Ok("plan v2".to_string()),
        )
        .unwrap();

        assert_eq!(*seen_round_numbers.borrow(), vec![3]);
    }

    #[test]
    fn auto_mode_loops_until_approved() {
        let reviewer = FakeAgent::new(
            "gemini",
            vec![
                success(verdict(false, &["fix the title"])),
                success(verdict(true, &[])),
            ],
        );
        let executor = FakeAgent::new(
            "claude",
            vec![AgentRunResult {
                success: true,
                text: "fixed".to_string(),
                structured_output: None,
            }],
        );

        let outcome = run_review_loop(
            &reviewer,
            &executor,
            Mode::Auto,
            3,
            false,
            0,
            std::path::Path::new("."),
            |content| format!("review: {content}"),
            |issues| format!("fix: {issues:?}"),
            |_round, _verdict, _content| Ok("report.md".to_string()),
            "plan v1".to_string(),
            || Ok("plan v2".to_string()),
        )
        .unwrap();

        assert!(outcome.approved);
        assert_eq!(outcome.rounds.len(), 2);
    }

    #[test]
    fn auto_mode_stops_at_max_rounds_without_approval() {
        let reviewer = FakeAgent::new(
            "gemini",
            vec![
                success(verdict(false, &["issue 1"])),
                success(verdict(false, &["issue 2"])),
            ],
        );
        let executor = FakeAgent::new(
            "claude",
            vec![AgentRunResult {
                success: true,
                text: "fixed".to_string(),
                structured_output: None,
            }],
        );

        let outcome = run_review_loop(
            &reviewer,
            &executor,
            Mode::Auto,
            2,
            false,
            0,
            std::path::Path::new("."),
            |content| format!("review: {content}"),
            |issues| format!("fix: {issues:?}"),
            |_round, _verdict, _content| Ok("report.md".to_string()),
            "plan v1".to_string(),
            || Ok("plan v2".to_string()),
        )
        .unwrap();

        assert!(!outcome.approved);
        assert_eq!(outcome.rounds.len(), 2);
    }

    #[test]
    fn reviewer_error_aborts_instead_of_looping() {
        let reviewer = FakeAgent::new("gemini", vec![failure("timed out")]);
        let executor = FakeAgent::new("claude", vec![]);

        let result = run_review_loop(
            &reviewer,
            &executor,
            Mode::Auto,
            3,
            false,
            0,
            std::path::Path::new("."),
            |content| format!("review: {content}"),
            |issues| format!("fix: {issues:?}"),
            |_round, _verdict, _content| Ok("report.md".to_string()),
            "plan v1".to_string(),
            || Ok("plan v2".to_string()),
        );

        assert!(result.is_err());
    }

    #[test]
    fn executor_error_aborts_instead_of_looping() {
        let reviewer = FakeAgent::new("gemini", vec![success(verdict(false, &["issue 1"]))]);
        let executor = FakeAgent::new("claude", vec![failure("permission denied")]);

        let result = run_review_loop(
            &reviewer,
            &executor,
            Mode::Auto,
            3,
            false,
            0,
            std::path::Path::new("."),
            |content| format!("review: {content}"),
            |issues| format!("fix: {issues:?}"),
            |_round, _verdict, _content| Ok("report.md".to_string()),
            "plan v1".to_string(),
            || Ok("plan v2".to_string()),
        );

        assert!(result.is_err());
    }
}
