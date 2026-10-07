use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::git;

/// Lockfiles / generated files that are never worth a reviewer's attention.
/// Matched by file name at any depth.
const DEFAULT_EXCLUDES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "npm-shrinkwrap.json",
    "go.sum",
    "poetry.lock",
    "Pipfile.lock",
    "Gemfile.lock",
    "composer.lock",
    "mix.lock",
    "flake.lock",
];

/// Glob matcher: `*` and `?` stay within a path segment, `**` crosses `/`.
fn glob_match(pat: &[char], s: &[char]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some(('*', rest)) if rest.first() == Some(&'*') => {
            let rest = &rest[1..];
            // `**/` also matches zero directories, so let it swallow the `/`.
            let after_slash = rest.strip_prefix(&['/']);
            (0..=s.len()).any(|i| {
                let at_boundary = i == 0 || s[i - 1] == '/';
                glob_match(rest, &s[i..])
                    || (at_boundary && after_slash.is_some_and(|r| glob_match(r, &s[i..])))
            })
        }
        Some(('*', rest)) => {
            let max = s.iter().position(|c| *c == '/').unwrap_or(s.len());
            (0..=max).any(|i| glob_match(rest, &s[i..]))
        }
        Some(('?', rest)) => s.first().is_some_and(|c| *c != '/') && glob_match(rest, &s[1..]),
        Some((c, rest)) => s.first() == Some(c) && glob_match(rest, &s[1..]),
    }
}

fn glob_str(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match(&p, &t)
}

/// Gitignore-flavoured matching against a repo-relative path. A pattern
/// without `/` matches any path segment (so `node_modules` covers
/// `node_modules/a/b.js` and `Cargo.lock` covers `crates/x/Cargo.lock`); one
/// with `/` is anchored at the repo root and also matches everything under a
/// matched directory. A trailing `/` is stripped, so `vendor/` behaves like
/// `vendor` (files and directories alike).
///
/// Supported syntax is a subset of gitignore: `*`, `?` and `**` only. There is
/// no `!` negation (use the include patterns instead) and no `[...]` classes.
/// Matching is plain backtracking, which is fine for the short patterns this
/// is meant for but can be slow with many `*`/`**` in one pattern.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    let pattern = pattern.trim_end_matches('/');
    if !pattern.contains('/') {
        return path.split('/').any(|seg| glob_str(pattern, seg));
    }
    let pattern = pattern.trim_start_matches('/');
    glob_str(pattern, path) || glob_str(&format!("{pattern}/**"), path)
}

fn matches_any<S: AsRef<str>>(patterns: &[S], path: &str) -> bool {
    patterns.iter().any(|p| pattern_matches(p.as_ref(), path))
}

/// Applies the exclusion/inclusion pipeline to a list of changed paths:
/// built-in defaults, config excludes, git's own ignore/generated markers,
/// then config includes re-adding anything that matches (include wins).
/// Original order is preserved.
pub fn filter_paths(repo_root: &Path, config: &Config, paths: Vec<String>) -> Result<Vec<String>> {
    let mut excluded = vec![false; paths.len()];
    for (i, path) in paths.iter().enumerate() {
        excluded[i] = matches_any(DEFAULT_EXCLUDES, path)
            || matches_any(&config.workflow.git_diff_exclude_patterns, path);
    }

    let remaining: Vec<String> = paths
        .iter()
        .zip(&excluded)
        .filter(|(_, e)| !**e)
        .map(|(p, _)| p.clone())
        .collect();
    let mut git_excluded: HashSet<String> =
        git::ignored_paths(repo_root, &remaining)?.into_iter().collect();
    git_excluded.extend(git::generated_paths(repo_root, &remaining)?);

    Ok(paths
        .into_iter()
        .zip(excluded)
        .filter(|(p, e)| {
            let is_excluded = *e || git_excluded.contains(p);
            !is_excluded || matches_any(&config.workflow.git_diff_include_patterns, p)
        })
        .map(|(p, _)| p)
        .collect())
}

/// The diff for `diff_args` (e.g. `["--staged"]`, `["a..b"]`), filtered
/// unless `no_filter` is set.
pub fn filtered_diff(
    repo_root: &Path,
    config: &Config,
    diff_args: &[String],
    no_filter: bool,
) -> Result<String> {
    if no_filter {
        return git::diff_paths_unfiltered(repo_root, diff_args);
    }
    let changed = git::changed_files(repo_root, diff_args)?;
    let kept = filter_paths(repo_root, config, changed)?;
    git::diff_paths(repo_root, diff_args, &kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_names_match_at_any_depth_and_as_directories() {
        assert!(pattern_matches("Cargo.lock", "Cargo.lock"));
        assert!(pattern_matches("Cargo.lock", "crates/x/Cargo.lock"));
        assert!(pattern_matches("node_modules", "node_modules/a/b.js"));
        assert!(!pattern_matches("Cargo.lock", "src/Cargo.lock.rs"));
    }

    #[test]
    fn globs_respect_segments() {
        assert!(pattern_matches("*.min.js", "web/app.min.js"));
        assert!(pattern_matches("dist/**", "dist/a/b.js"));
        assert!(pattern_matches("vendor/", "vendor/x/y.go"));
        assert!(pattern_matches("docs/*.md", "docs/a.md"));
        assert!(!pattern_matches("docs/*.md", "docs/sub/a.md"));
        assert!(!pattern_matches("docs/*.md", "other/docs/a.md"));
    }

    #[test]
    fn double_star_slash_matches_zero_directories() {
        assert!(pattern_matches("**/*.js", "app.js"));
        assert!(pattern_matches("**/*.js", "a/b/app.js"));
        assert!(pattern_matches("src/**/*.rs", "src/main.rs"));
        assert!(pattern_matches("src/**/*.rs", "src/a/b/main.rs"));
        assert!(pattern_matches("a/**/b", "a/b"));
        assert!(!pattern_matches("a/**/b", "a/xb"));
    }
}
