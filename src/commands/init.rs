use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::state::State;

const GITIGNORE_LINE: &str = ".aic/";

pub fn run(repo_root: &Path) -> Result<()> {
    let global_path = Config::global_path()?;
    if global_path.exists() {
        println!("Global config already exists at {}", global_path.display());
    } else {
        Config::default().save_global()?;
        println!("Created global config at {}", global_path.display());
    }

    let aic_dir = repo_root.join(".aic");
    for sub in ["plans", "dev", "adhoc-reviews"] {
        fs::create_dir_all(aic_dir.join(sub))
            .with_context(|| format!("creating .aic/{sub}"))?;
    }

    let state_path = State::path(repo_root);
    if !state_path.exists() {
        State::default().save(repo_root)?;
    }

    ensure_gitignored(repo_root)?;

    println!("Initialized aic in {}", repo_root.display());
    Ok(())
}

fn ensure_gitignored(repo_root: &Path) -> Result<()> {
    let gitignore_path = repo_root.join(".gitignore");
    let existing = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path)
            .with_context(|| format!("reading {}", gitignore_path.display()))?
    } else {
        String::new()
    };

    if existing.lines().any(|line| line.trim() == GITIGNORE_LINE) {
        return Ok(());
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .with_context(|| format!("opening {}", gitignore_path.display()))?;
    // A missing trailing newline would otherwise concatenate our line onto
    // whatever the file's last line currently is.
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, "{GITIGNORE_LINE}")?;
    println!("Added '{GITIGNORE_LINE}' to .gitignore");
    Ok(())
}
