use std::path::Path;

use anyhow::{bail, Result};

use crate::git;
use crate::state::{Feature, State};

pub fn run(repo_root: &Path, slug_arg: Option<String>) -> Result<()> {
    let mut state = State::load(repo_root)?;
    // state.json can drift from .aic/ (files edited or removed by hand,
    // runs that died before saving), so re-read the disk on every status.
    if state.refresh(repo_root) {
        if let Err(err) = state.save(repo_root) {
            eprintln!("warning: couldn't update state.json: {err:#}");
        }
    }
    let explicit = slug_arg.is_some();
    let slug = slug_arg.or_else(|| git::current_branch_slug(repo_root).ok());

    if let Some(slug) = &slug {
        if let Some(feature) = state.feature(slug) {
            print_feature(slug, feature);
            return Ok(());
        }
        if explicit {
            bail!("no feature '{slug}' found; run `aic plan new {slug}` first");
        }
        // Inferred from the branch name, not found: fall through to
        // listing everything rather than erroring on a best-effort guess.
    }

    if state.features.is_empty() {
        println!("No features tracked yet. Run `aic plan new <slug>` to start one.");
        return Ok(());
    }

    let mut slugs: Vec<_> = state.features.keys().collect();
    slugs.sort();
    for slug in slugs {
        print_feature(slug, state.feature(slug).unwrap());
    }
    Ok(())
}

fn print_feature(slug: &str, feature: &Feature) {
    println!("## {slug}");
    println!(
        "  plan: {:?} ({} round(s))",
        feature.plan.status,
        feature.plan.rounds.len()
    );
    println!(
        "  dev:  {:?} ({} round(s))",
        feature.dev.status,
        feature.dev.rounds.len()
    );
}
