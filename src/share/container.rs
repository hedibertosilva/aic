use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use base64::Engine;
use rand::RngCore;

const CONTAINER: &str = "aic-share-server";
const TOKEN_FILE: &str = "publisher-token";

fn data_dir(value: Option<&Path>) -> Result<PathBuf> {
    let path = value
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".aic-share"));
    fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
    path.canonicalize()
        .with_context(|| format!("resolving {}", path.display()))
}

fn token(path: &Path) -> Result<String> {
    let file = path.join(TOKEN_FILE);
    if file.exists() {
        return Ok(fs::read_to_string(&file)?.trim().to_string());
    }
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    fs::write(&file, format!("{value}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
    }
    Ok(value)
}

fn validate_runtime(runtime: &str) -> Result<()> {
    if !matches!(runtime, "docker" | "podman") {
        bail!("runtime must be docker or podman");
    }
    Ok(())
}

pub fn start(
    public_url: &str,
    host: &str,
    port: u16,
    selected: Option<&Path>,
    runtime: &str,
) -> Result<()> {
    validate_runtime(runtime)?;
    if !public_url.starts_with("https://")
        && !public_url.starts_with("http://localhost")
        && !public_url.starts_with("http://127.0.0.1")
    {
        bail!("--public-url must use https (except localhost development URLs)");
    }
    let data = data_dir(selected)?;
    let publisher_token = token(&data)?;
    let executable = std::env::current_exe()?.canonicalize()?;
    // Copy instead of relabelling/mounting the user's installed executable.
    // The data directory is the container-owned deployment bundle.
    let relay_binary = data.join("aic-relay");
    fs::copy(&executable, &relay_binary).with_context(|| {
        format!(
            "copying {} to {}",
            executable.display(),
            relay_binary.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&relay_binary, fs::Permissions::from_mode(0o755))?;
    }
    let mapping = format!("{host}:{port}:8787");
    let data_mount = format!("{}:/data:Z", data.display());
    let _ = Command::new(runtime).args(["rm", "-f", CONTAINER]).output();
    let status = Command::new(runtime)
        .args([
            "run",
            "-d",
            "--restart",
            "unless-stopped",
            "--name",
            CONTAINER,
            "-p",
            &mapping,
            "-v",
            &data_mount,
            "-e",
            &format!("AIC_SHARE_ADMIN_TOKEN={publisher_token}"),
            "debian:trixie-slim",
            "/data/aic-relay",
            "share-relay",
            "--public-url",
            public_url,
        ])
        .status()
        .with_context(|| format!("running {runtime}; is it installed?"))?;
    if !status.success() {
        bail!("{runtime} failed to start the sharing server");
    }
    println!("Sharing server started at {public_url}");
    println!("Publisher token (store it securely): {publisher_token}");
    println!("Persistent data: {}", data.display());
    Ok(())
}

pub fn stop(runtime: &str) -> Result<()> {
    validate_runtime(runtime)?;
    let status = Command::new(runtime)
        .args(["rm", "-f", CONTAINER])
        .status()?;
    if !status.success() {
        bail!("{runtime} could not stop {CONTAINER}");
    }
    Ok(())
}

pub fn status(runtime: &str) -> Result<()> {
    validate_runtime(runtime)?;
    let status = Command::new(runtime)
        .args(["ps", "-a", "--filter", &format!("name=^{CONTAINER}$")])
        .status()?;
    if !status.success() {
        bail!("{runtime} status failed");
    }
    Ok(())
}

pub fn print_token(selected: Option<&Path>) -> Result<()> {
    let data = data_dir(selected)?;
    println!("{}", token(&data)?);
    Ok(())
}
