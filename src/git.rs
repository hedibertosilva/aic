use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Prefers the system git at a fixed path over whatever `git` resolves to
/// on `PATH`. On this machine `~/.local/bin` can end up ahead of `/usr/bin`
/// (e.g. a distrobox-exported binary's installer prepending it), which
/// would otherwise shadow the real `git` with an unrelated wrapper script.
fn git_binary() -> &'static str {
    if Path::new("/usr/bin/git").exists() {
        "/usr/bin/git"
    } else {
        "git"
    }
}

fn run_git(repo_root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new(git_binary())
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn is_repo(dir: &Path) -> bool {
    Command::new(git_binary())
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The top-level directory of the repo containing `dir`.
pub fn repo_root(dir: &Path) -> Result<std::path::PathBuf> {
    let out = run_git(dir, &["rev-parse", "--show-toplevel"])?;
    Ok(std::path::PathBuf::from(out))
}

pub fn head_sha(repo_root: &Path) -> Result<String> {
    run_git(repo_root, &["rev-parse", "HEAD"])
}

pub fn diff_staged(repo_root: &Path) -> Result<String> {
    run_git(repo_root, &["diff", "--staged"])
}

pub fn has_staged_changes(repo_root: &Path) -> Result<bool> {
    Ok(!diff_staged(repo_root)?.is_empty())
}

/// Everything different from the last commit — staged AND unstaged — as a
/// single diff. Used to refresh a code review's content after an auto-mode
/// executor run: the executor edits the working tree but doesn't
/// necessarily stage or commit, so re-running the *original* target
/// (`diff --staged`, or a commit range) would miss its changes entirely.
/// Falls back to the staged diff on a repo with no commits yet, where
/// `HEAD` doesn't resolve.
pub fn diff_since_head(repo_root: &Path) -> Result<String> {
    if head_sha(repo_root).is_err() {
        return diff_staged(repo_root);
    }
    run_git(repo_root, &["diff", "HEAD"])
}

pub fn diff_range(repo_root: &Path, from: &str, to: &str) -> Result<String> {
    let range = format!("{from}..{to}");
    run_git(repo_root, &["diff", &range])
}

pub fn diff_against_branch(repo_root: &Path, branch: &str) -> Result<String> {
    run_git(repo_root, &["diff", &format!("{branch}...HEAD")])
}

/// Diffs a single ref against the current working tree (staged, unstaged,
/// and anything committed since that ref — everything). Used to re-read a
/// code review's full diff after an auto-mode executor run, anchored at
/// whichever ref the review's target originally used (so a `commits` or
/// `branch` review doesn't shrink down to only the executor's latest
/// uncommitted edit on later rounds).
pub fn diff_from_anchor(repo_root: &Path, anchor: &str) -> Result<String> {
    run_git(repo_root, &["diff", anchor])
}

/// The commit `branch` and `HEAD` last had in common — the right anchor for
/// "everything this branch has changed", regardless of how `branch` itself
/// has since moved.
pub fn merge_base(repo_root: &Path, branch: &str) -> Result<String> {
    run_git(repo_root, &["merge-base", branch, "HEAD"])
}

/// The repo's default branch, detected from the `origin` remote
/// (`refs/remotes/origin/HEAD`). Falls back to `fallback` (typically the
/// user's `default_base_branch` config, if set) when there is no such remote
/// ref — e.g. a repo with no remote, or one that was never fetched.
pub fn default_branch(repo_root: &Path, fallback: Option<&str>) -> Result<String> {
    if let Ok(out) = run_git(
        repo_root,
        &["symbolic-ref", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(name) = out.strip_prefix("refs/remotes/origin/") {
            return Ok(name.to_string());
        }
    }
    if let Some(fallback) = fallback.filter(|f| !f.is_empty()) {
        return Ok(fallback.to_string());
    }
    bail!(
        "couldn't detect the default branch (no origin/HEAD and no default_base_branch configured)"
    )
}

pub fn current_branch(repo_root: &Path) -> Result<String> {
    run_git(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])
}

/// Turns a branch name into a slug usable as a feature id: lowercase,
/// non-alphanumeric runs collapsed to a single `-`, trimmed of leading /
/// trailing `-`. E.g. "feature/Auth_Flow!!" -> "auth-flow".
pub fn slugify(input: &str) -> String {
    let mut slug = String::with_capacity(input.len());
    let mut last_was_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

pub fn current_branch_slug(repo_root: &Path) -> Result<String> {
    let branch = current_branch(repo_root)?;
    Ok(slugify(&branch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        run_git(dir.path(), &["init", "-q"]).unwrap();
        run_git(dir.path(), &["config", "user.email", "test@example.com"]).unwrap();
        run_git(dir.path(), &["config", "user.name", "Test"]).unwrap();
        dir
    }

    fn commit_all(dir: &Path, message: &str) {
        run_git(dir, &["add", "-A"]).unwrap();
        run_git(dir, &["commit", "-q", "-m", message]).unwrap();
    }

    #[test]
    fn detects_a_real_repo() {
        let dir = init_repo();
        assert!(is_repo(dir.path()));
    }

    #[test]
    fn non_repo_is_not_a_repo() {
        let dir = tempdir().unwrap();
        assert!(!is_repo(dir.path()));
    }

    #[test]
    fn staged_changes_show_up_in_diff() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        run_git(dir.path(), &["add", "a.txt"]).unwrap();

        assert!(has_staged_changes(dir.path()).unwrap());
        assert!(diff_staged(dir.path()).unwrap().contains("hello"));
    }

    #[test]
    fn no_staged_changes_after_commit() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        commit_all(dir.path(), "initial");

        assert!(!has_staged_changes(dir.path()).unwrap());
    }

    #[test]
    fn diff_since_head_falls_back_to_staged_without_any_commits() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        run_git(dir.path(), &["add", "a.txt"]).unwrap();

        assert!(diff_since_head(dir.path()).unwrap().contains("hello"));
    }

    #[test]
    fn diff_since_head_sees_both_staged_and_unstaged_changes() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "v1\n").unwrap();
        fs::write(dir.path().join("b.txt"), "v1\n").unwrap();
        commit_all(dir.path(), "v1");

        // staged change to a.txt
        fs::write(dir.path().join("a.txt"), "v2\n").unwrap();
        run_git(dir.path(), &["add", "a.txt"]).unwrap();
        // unstaged change to the already-tracked b.txt
        fs::write(dir.path().join("b.txt"), "v2\n").unwrap();

        let diff = diff_since_head(dir.path()).unwrap();
        assert!(diff.contains("a.txt"));
        assert!(diff.contains("b.txt"));
    }

    #[test]
    fn diff_range_covers_commits_between_two_shas() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "v1\n").unwrap();
        commit_all(dir.path(), "v1");
        let start = head_sha(dir.path()).unwrap();

        fs::write(dir.path().join("a.txt"), "v2\n").unwrap();
        commit_all(dir.path(), "v2");
        let end = head_sha(dir.path()).unwrap();

        let diff = diff_range(dir.path(), &start, &end).unwrap();
        assert!(diff.contains("-v1"));
        assert!(diff.contains("+v2"));
    }

    #[test]
    fn default_branch_falls_back_without_origin_remote() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "v1\n").unwrap();
        commit_all(dir.path(), "v1");

        assert!(default_branch(dir.path(), None).is_err());
        assert_eq!(
            default_branch(dir.path(), Some("trunk")).unwrap(),
            "trunk"
        );
    }

    #[test]
    fn slugify_handles_separators_and_case() {
        assert_eq!(slugify("feature/Auth_Flow!!"), "feature-auth-flow");
        assert_eq!(slugify("already-a-slug"), "already-a-slug");
        assert_eq!(slugify("  spaces  everywhere  "), "spaces-everywhere");
    }

    #[test]
    fn current_branch_slug_reads_the_checked_out_branch() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "v1\n").unwrap();
        commit_all(dir.path(), "v1");
        run_git(dir.path(), &["checkout", "-q", "-b", "feature/My_Thing"]).unwrap();

        assert_eq!(current_branch_slug(dir.path()).unwrap(), "feature-my-thing");
    }
}
