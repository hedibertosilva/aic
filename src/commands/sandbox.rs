use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::Config;
use crate::sandbox::Sandbox;
use crate::templates;

/// Finds `name` on `PATH`, following symlinks.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|p| p.canonicalize().ok())
}

/// `aic sandbox build [tag]`: builds the sandbox image from the embedded
/// Dockerfile. `claude` is installed at a pinned version; `agy` is copied
/// from the host, so rebuilding is also how the image picks up a newer one.
pub fn build(config: &Config, tag: Option<String>, claude_version: Option<String>) -> Result<()> {
    let Some(sandbox) = Sandbox::from_config(config)? else {
        bail!("sandbox is set to \"none\"; set sandbox = \"docker\" or \"podman\" to build an image");
    };
    let tag = tag.unwrap_or_else(|| config.sandbox_image.clone());

    let tmp = tempfile::tempdir().context("creating build context")?;
    let context = tmp.path().to_path_buf();
    let agy = find_on_path("agy");
    let codex = find_on_path("codex");
    if let Some(agy) = &agy {
        std::fs::copy(agy, context.join("agy"))
            .with_context(|| format!("copying {} into the build context", agy.display()))?;
    } else {
        eprintln!("warning: 'agy' not found on PATH; the image will only contain claude");
    }
    if let Some(codex) = &codex {
        std::fs::copy(codex, context.join("codex"))
            .with_context(|| format!("copying {} into the build context", codex.display()))?;
    } else {
        eprintln!("warning: 'codex' not found on PATH; the image will not contain codex");
    }
    std::fs::write(context.join("Dockerfile"), templates::sandbox_dockerfile(agy.is_some(), codex.is_some()))?;

    let mut build_args = Vec::new();
    if let Some(v) = claude_version {
        build_args.push(("CLAUDE_VERSION".to_string(), v));
    }
    let status = Command::new(sandbox.runtime_name())
        .args(sandbox.build_args(&tag, &context, &build_args))
        .status()
        .with_context(|| format!("running '{} build'; is it installed?", sandbox.runtime_name()));
    if !status?.success() {
        bail!("{} build failed", sandbox.runtime_name());
    }

    println!("Built sandbox image '{tag}'.");
    if tag != config.sandbox_image {
        println!("Set sandbox_image = \"{tag}\" in your config to use it.");
    }
    Ok(())
}
