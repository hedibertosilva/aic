use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

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

/// Runs git with `input` on stdin and returns raw stdout. Exit status 1 is
/// tolerated (`check-ignore` uses it for "nothing matched"); anything else
/// non-zero is an error.
fn run_git_stdin(repo_root: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new(git_binary())
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running git {}", args.join(" ")))?;

    // Feed stdin from a thread so a large path list can't deadlock against
    // git filling its stdout pipe.
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child
        .wait_with_output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    let _ = writer.join();

    match output.status.code() {
        Some(0) | Some(1) => Ok(output.stdout),
        _ => bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Paths changed by `git diff <diff_args>`, relative to the repo root.
/// Renames are reported as a delete plus an add so both sides show up.
pub fn changed_files(repo_root: &Path, diff_args: &[String]) -> Result<Vec<String>> {
    let mut args: Vec<&str> = vec!["diff", "--name-only", "-z", "--no-renames"];
    args.extend(diff_args.iter().map(String::as_str));
    // `run_git` trims, which is harmless for NUL-separated output.
    Ok(split_nul(run_git(repo_root, &args)?.as_bytes()))
}

/// `git diff <diff_args>` restricted to `paths`, with renames split like
/// `changed_files`. Paths go via stdin as literal pathspecs, so magic
/// characters in file names stay literal and the argv size limit doesn't
/// apply. An empty `paths` yields an empty diff rather than git's "no
/// pathspec means everything".
pub fn diff_paths(repo_root: &Path, diff_args: &[String], paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args: Vec<&str> = vec!["diff", "--no-renames"];
    args.extend(diff_args.iter().map(String::as_str));
    args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
    let input: String = paths.iter().map(|p| format!(":(literal){p}\0")).collect();
    let out = run_git_stdin(repo_root, &args, input.as_bytes())?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// `git diff <diff_args>` with no path filtering, rename detection left at
/// git's default.
pub fn diff_paths_unfiltered(repo_root: &Path, diff_args: &[String]) -> Result<String> {
    let mut args: Vec<&str> = vec!["diff"];
    args.extend(diff_args.iter().map(String::as_str));
    run_git(repo_root, &args)
}

/// The subset of `paths` that git considers ignored by `.gitignore`
/// rules (`--no-index` so already-tracked files are still checked).
pub fn ignored_paths(repo_root: &Path, paths: &[String]) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let input = paths.join("\0") + "\0";
    let out = run_git_stdin(
        repo_root,
        &["check-ignore", "--no-index", "--stdin", "-z"],
        input.as_bytes(),
    )?;
    Ok(split_nul(&out))
}

/// The subset of `paths` marked `linguist-generated` in `.gitattributes`.
pub fn generated_paths(repo_root: &Path, paths: &[String]) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let input = paths.join("\0") + "\0";
    let out = run_git_stdin(
        repo_root,
        &["check-attr", "linguist-generated", "--stdin", "-z"],
        input.as_bytes(),
    )?;
    // Output is repeating `path NUL attribute NUL value NUL` triples.
    let fields = split_nul_keep_empty(&out);
    Ok(fields
        .chunks(3)
        .filter(|c| c.len() == 3 && matches!(c[2].as_str(), "set" | "true"))
        .map(|c| c[0].clone())
        .collect())
}

fn split_nul_keep_empty(bytes: &[u8]) -> Vec<String> {
    let mut fields: Vec<String> = bytes
        .split(|b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    // trailing NUL leaves one empty tail element
    if fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    fields
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
    fn range_diff_covers_commits_between_two_shas() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "v1\n").unwrap();
        commit_all(dir.path(), "v1");
        let start = head_sha(dir.path()).unwrap();

        fs::write(dir.path().join("a.txt"), "v2\n").unwrap();
        commit_all(dir.path(), "v2");
        let end = head_sha(dir.path()).unwrap();

        let diff = diff_paths_unfiltered(dir.path(), &[format!("{start}..{end}")]).unwrap();
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

#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::config::Config;
    use crate::diff_filter::filtered_diff;
    use std::fs;
    use tempfile::tempdir;

    fn setup() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let p = dir.path();
        run_git(p, &["init", "-q"]).unwrap();
        run_git(p, &["config", "user.email", "t@example.com"]).unwrap();
        run_git(p, &["config", "user.name", "T"]).unwrap();
        fs::write(p.join(".gitattributes"), "gen/** linguist-generated\n").unwrap();
        fs::create_dir(p.join("gen")).unwrap();
        fs::write(p.join("gen/out.txt"), "generated\n").unwrap();
        fs::write(p.join("Cargo.lock"), "lock\n").unwrap();
        fs::write(p.join("keep.rs"), "fn main() {}\n").unwrap();
        fs::write(p.join("skip.txt"), "skipme\n").unwrap();
        run_git(p, &["add", "-A"]).unwrap();
        dir
    }

    #[test]
    fn filters_defaults_generated_and_config_excludes() {
        let dir = setup();
        let mut config = Config::default();
        config.git_diff_exclude_patterns = vec!["skip.txt".into()];
        let args = vec!["--staged".to_string()];

        let diff = filtered_diff(dir.path(), &config, &args, false).unwrap();
        assert!(diff.contains("keep.rs"));
        assert!(!diff.contains("Cargo.lock"));
        assert!(!diff.contains("gen/out.txt"));
        assert!(!diff.contains("skip.txt"));

        let full = filtered_diff(dir.path(), &config, &args, true).unwrap();
        assert!(full.contains("Cargo.lock") && full.contains("gen/out.txt"));
    }

    #[test]
    fn include_patterns_win_over_every_exclusion() {
        let dir = setup();
        let mut config = Config::default();
        config.git_diff_exclude_patterns = vec!["skip.txt".into()];
        config.git_diff_include_patterns = vec!["Cargo.lock".into(), "gen/**".into(), "skip.txt".into()];
        let diff = filtered_diff(dir.path(), &config, &["--staged".to_string()], false).unwrap();
        assert!(diff.contains("Cargo.lock"));
        assert!(diff.contains("gen/out.txt"));
        assert!(diff.contains("skip.txt"));
    }

    #[test]
    fn everything_excluded_gives_an_empty_diff() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        run_git(p, &["init", "-q"]).unwrap();
        fs::write(p.join("Cargo.lock"), "lock\n").unwrap();
        run_git(p, &["add", "-A"]).unwrap();
        let diff = filtered_diff(p, &Config::default(), &["--staged".to_string()], false).unwrap();
        assert!(diff.is_empty());
    }
}
