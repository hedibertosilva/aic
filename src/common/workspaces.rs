use crate::diagnostics::{GitContext, git_context};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
pub fn list(root: &Path) -> Result<Vec<GitContext>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain", "-z"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        bail!("Worktree list unavailable");
    }
    let paths = String::from_utf8_lossy(&output.stdout);
    Ok(paths
        .split('\0')
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter_map(|p| git_context(Path::new(p)).ok())
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
