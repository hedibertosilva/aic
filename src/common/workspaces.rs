use crate::diagnostics::{GitContext, git_context};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

fn worktree_paths(output: &[u8], separator: char) -> Vec<PathBuf> {
    String::from_utf8_lossy(output)
        .split(separator)
        .filter_map(|line| {
            line.strip_suffix('\r')
                .unwrap_or(line)
                .strip_prefix("worktree ")
        })
        .map(PathBuf::from)
        .collect()
}

pub fn list(root: &Path) -> Result<Vec<GitContext>> {
    let mut output = Command::new("git")
        .args(["worktree", "list", "--porcelain", "-z"])
        .current_dir(root)
        .output()?;
    let separator = if output.status.success() {
        '\0'
    } else {
        // `git worktree list -z` was added after Git 2.34. Retry using the
        // older porcelain format so supported installations can still list
        // ordinary worktree paths.
        output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(root)
            .output()?;
        '\n'
    };
    if !output.status.success() {
        bail!("Worktree list unavailable");
    }
    Ok(worktree_paths(&output.stdout, separator)
        .iter()
        .filter_map(|p| git_context(p).ok())
        .collect())
}
pub fn resolve(root: &Path, requested: Option<&str>) -> Result<PathBuf> {
    let Some(requested) = requested.filter(|p| !p.is_empty()) else {
        return Ok(root.to_path_buf());
    };
    let selected = Path::new(requested)
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("Selected checkout unavailable"))?;
    if !list(root)?
        .iter()
        .any(|g| Path::new(&g.checkout).canonicalize().ok().as_ref() == Some(&selected))
    {
        bail!("Select a registered worktree of this repository");
    }
    Ok(selected)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nul_terminated_worktree_list() {
        assert_eq!(
            worktree_paths(
                b"worktree /repo/main\0HEAD abc\0\0worktree /repo/other\0HEAD def\0",
                '\0'
            ),
            vec![PathBuf::from("/repo/main"), PathBuf::from("/repo/other")]
        );
    }

    #[test]
    fn parses_legacy_line_terminated_worktree_list() {
        assert_eq!(
            worktree_paths(
                b"worktree /repo/main\nHEAD abc\n\nworktree /repo/other\r\nHEAD def\r\n",
                '\n'
            ),
            vec![PathBuf::from("/repo/main"), PathBuf::from("/repo/other")]
        );
    }

    #[test]
    fn arbitrary_directory_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(resolve(root.path(), other.path().to_str()).is_err());
        assert_eq!(
            resolve(root.path(), root.path().to_str()).unwrap(),
            root.path().canonicalize().unwrap()
        );
    }
}
