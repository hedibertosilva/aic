use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::Config;
use crate::sandbox::{Sandbox, codex_code_mode_host, find_on_path};
use crate::templates;

/// `aic sandbox build [tag]`: builds the sandbox image from the embedded
/// Dockerfile. `claude` is installed at a pinned version; `agy` is copied
/// from the host, as are `codex` and its code-mode helper. Rebuilding is
/// also how the image picks up newer versions.
pub fn build(config: &Config, tag: Option<String>, claude_version: Option<String>) -> Result<()> {
    let Some(sandbox) = Sandbox::from_config(config)? else {
        bail!("sandbox is set to \"none\"; set sandbox = \"docker\" or \"podman\" to build an image");
    };
    let tag = tag.unwrap_or_else(|| config.sandbox.image.clone());

    let tmp = tempfile::tempdir().context("creating build context")?;
    let context = tmp.path().to_path_buf();
    let (agy, codex) = if config.sandbox.bind_agents {
        println!("Host agy/Codex binaries will be bound at run time; omitting them from the image.");
        (None, None)
    } else {
        (find_on_path("agy"), find_on_path("codex"))
    };
    if let Some(agy) = &agy {
        std::fs::copy(agy, context.join("agy"))
            .with_context(|| format!("copying {} into the build context", agy.display()))?;
    } else if !config.sandbox.bind_agents {
        eprintln!("warning: 'agy' not found on PATH; the image will not contain agy");
    }
    if let Some(codex) = &codex {
        let helper = codex_code_mode_host(codex)
            .or_else(|| find_on_path("codex-code-mode-host"))
            .with_context(|| format!(
                "'codex-code-mode-host' not found beside {} or on PATH; install the matching Codex helper before building the sandbox image",
                codex.display()
            ))?;
        std::fs::copy(codex, context.join("codex"))
            .with_context(|| format!("copying {} into the build context", codex.display()))?;
        std::fs::copy(&helper, context.join("codex-code-mode-host"))
            .with_context(|| format!("copying {} into the build context", helper.display()))?;
    } else if !config.sandbox.bind_agents {
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
    if tag != config.sandbox.image {
        println!("Set sandbox.image = \"{tag}\" in your config to use it.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_helper_beside_real_codex_through_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let release = tmp.path().join("release");
        std::fs::create_dir(&release).unwrap();
        let codex = release.join("codex");
        let helper = release.join("codex-code-mode-host");
        std::fs::write(&codex, b"codex").unwrap();
        std::fs::write(&helper, b"helper").unwrap();
        let link = tmp.path().join("codex");
        std::os::unix::fs::symlink(&codex, &link).unwrap();

        assert_eq!(codex_code_mode_host(&link), Some(helper));
    }

    #[test]
    fn missing_helper_is_not_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let codex = tmp.path().join("codex");
        std::fs::write(&codex, b"codex").unwrap();
        assert_eq!(codex_code_mode_host(&codex), None);

        // A directory with the right name is not a usable helper.
        std::fs::create_dir(tmp.path().join("codex-code-mode-host")).unwrap();
        assert_eq!(codex_code_mode_host(&codex), None);
    }
}
