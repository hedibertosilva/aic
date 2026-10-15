//! Container isolation for agent invocations.
//!
//! Every `claude`/`agy` run can be wrapped as `<runtime> run … <image>
//! <binary> <args>` so an agent running unattended can only touch the
//! mounted repo (plus any `[sandbox] mounts`), not the rest of `$HOME`. The
//! threat model is *accidental* destructive commands: the network stays open
//! and agent credentials are mounted in, so this is not a confidentiality
//! boundary.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::config::{Config, SandboxKind, CONFIG_DIR_ENV, IN_SANDBOX_ENV};

/// `$HOME` inside the container. The image provides it writable by any uid.
const CONTAINER_HOME: &str = "/home/aic";

/// Where the host's `aic` binary shows up inside the container.
const CONTAINER_AIC: &str = "/usr/local/bin/aic";

/// Where the host's global aic config dir shows up inside the container.
const CONTAINER_CONFIG: &str = "/etc/aic";

/// Resolve symlinks before copying or binding a host executable.
pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|p| p.canonicalize().ok())
}

/// Standalone Codex installations put the helper beside the real binary,
/// not necessarily beside the symlink exposed on PATH.
pub(crate) fn codex_code_mode_host(codex: &Path) -> Option<PathBuf> {
    let codex = codex.canonicalize().ok()?;
    let helper = codex.parent()?.join("codex-code-mode-host");
    helper.is_file().then_some(helper)
}

fn host_agent_binaries() -> Vec<(String, PathBuf)> {
    let mut binaries = Vec::new();
    if let Some(agy) = find_on_path("agy") {
        binaries.push(("agy".into(), agy));
    }
    if let Some(codex) = find_on_path("codex") {
        if let Some(helper) = codex_code_mode_host(&codex)
            .or_else(|| find_on_path("codex-code-mode-host"))
        {
            binaries.push(("codex-code-mode-host".into(), helper));
        }
        binaries.push(("codex".into(), codex));
    }
    binaries
}

/// Whether the repo is mounted writable (executor) or read-only (reviewer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadWrite,
    ReadOnly,
}

/// A server-validated host directory granted to one conversation. It is
/// mounted at the same absolute path inside the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditionalPath {
    pub path: PathBuf,
    pub access: Access,
}

/// Per-invocation inputs to `Sandbox::run_args`.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub repo_root: PathBuf,
    /// Shared git dir, mounted too when it lives outside `repo_root`
    /// (linked worktrees).
    pub git_dir: Option<PathBuf>,
    /// Working directory inside the container; must be under `repo_root`.
    pub workdir: PathBuf,
    pub access: Access,
    pub interactive: bool,
    /// File name of the agent binary (`claude`, `agy`, ...), which picks
    /// which credential mounts apply.
    pub agent: String,
    /// Agent argv, after the binary name.
    pub args: Vec<String>,
    /// Environment entries supplied only for this invocation.
    pub env: Vec<(String, String)>,
    /// Conversation-scoped paths, separate from global configured mounts.
    pub additional_paths: Vec<AdditionalPath>,
    /// Share the host network so this invocation can reach a service bound
    /// to the host loopback interface.
    pub host_network: bool,
    /// Container name, so the caller can `<runtime> kill` it: killing the
    /// `run` client alone leaves the container running.
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Sandbox {
    pub kind: SandboxKind,
    pub image: String,
    pub mounts: Vec<String>,
    pub env: Vec<String>,
    host_home: PathBuf,
    data_dir: PathBuf,
    uid: u32,
    gid: u32,
    /// The running `aic`, bind-mounted so agents (and the aic skills) can
    /// call it inside. Always the host's version, no image rebuild needed.
    aic_binary: Option<PathBuf>,
    /// Host global config dir, mounted read-only so `aic` inside sees the
    /// same agents, models and permission mode.
    config_dir: Option<PathBuf>,
    /// All available host agents are mounted, including when the outer
    /// agent is Claude, so an `aic` inside can start another agent.
    bound_binaries: Option<Vec<(String, PathBuf)>>,
}

impl Sandbox {
    /// `None` when the config says `[sandbox] runtime = "none"`.
    pub fn from_config(config: &Config) -> Result<Option<Sandbox>> {
        if config.sandbox.runtime == SandboxKind::None {
            return Ok(None);
        }
        let host_home = PathBuf::from(std::env::var("HOME").context("HOME environment variable is not set")?);
        let data_dir = match std::env::var("XDG_DATA_HOME") {
            Ok(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
            _ => host_home.join(".local").join("share"),
        }
        .join("aic")
        .join("sandbox");
        let meta = std::fs::metadata(&host_home)
            .with_context(|| format!("reading {}", host_home.display()))?;
        Ok(Some(Sandbox {
            kind: config.sandbox.runtime,
            image: config.sandbox.image.clone(),
            mounts: config.sandbox.mounts.clone(),
            env: config.sandbox.env.clone(),
            host_home,
            data_dir,
            uid: meta.uid(),
            gid: meta.gid(),
            aic_binary: std::env::current_exe().ok().filter(|p| p.is_file()),
            config_dir: Config::global_dir().ok().filter(|p| p.is_dir()),
            bound_binaries: config.sandbox.bind_agents.then(host_agent_binaries),
        }))
    }

    /// `<runtime> kill <name>` for a container started with `RunSpec::name`.
    pub fn kill_command(&self, name: &str) -> Command {
        let mut cmd = Command::new(self.runtime());
        cmd.args(["kill", name]).stdout(Stdio::null()).stderr(Stdio::null());
        cmd
    }

    fn runtime(&self) -> &'static str {
        self.kind.label()
    }

    /// Fails with a clear message if the runtime binary is missing or the
    /// image hasn't been built. Run before starting any agent.
    pub fn preflight(&self) -> Result<()> {
        if find_on_path(self.runtime()).is_none() {
            bail!("sandbox runtime '{}' not found on PATH; install it or set [sandbox] runtime = \"none\"", self.runtime());
        }
        let mut command = Command::new(self.runtime());
        command.args(["image", "inspect", &self.image]);
        if crate::diagnostics::command_succeeds(command, std::time::Duration::from_secs(3))? { return Ok(()); }
        bail!("sandbox image '{}' not found; run 'aic sandbox build' or set [sandbox] runtime = \"none\"", self.image)

    }

    /// Creates host-side state dirs that get bind-mounted, so the runtime
    /// doesn't create them root-owned.
    fn prepare(&self, agent: &str) -> Result<()> {
        self.validate_bound_agent(agent)?;
        let dir = match agent {
            "claude" => self.claude_state_dir(),
            "agy" => self.agy_state_dir(),
            _ => return Ok(()),
        };
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(())
    }

    fn validate_bound_agent(&self, agent: &str) -> Result<()> {
        if let Some(binaries) = &self.bound_binaries {
            let required: &[&str] = match agent {
                "agy" => &["agy"],
                "codex" => &["codex", "codex-code-mode-host"],
                _ => &[],
            };
            for name in required {
                if !binaries.iter().any(|(binary, _)| binary == *name) {
                    bail!(
                        "[sandbox] bind_agents is enabled but host binary '{name}' was not found; install it on PATH (the Codex helper may also be beside codex) or disable bind_agents and rebuild the image"
                    );
                }
            }
        }
        Ok(())
    }

    fn claude_state_dir(&self) -> PathBuf {
        self.data_dir.join("claude")
    }

    fn agy_state_dir(&self) -> PathBuf {
        self.data_dir.join("agy")
    }

    /// Builds the full `<runtime> run …` command for `spec`, creating any
    /// needed host state dirs first.
    pub fn command(&self, spec: &RunSpec) -> Result<Command> {
        self.prepare(&spec.agent)?;
        let mut cmd = Command::new(self.runtime());
        cmd.args(self.run_args(spec));
        Ok(cmd)
    }

    /// Interactive `agy` for `aic agent login`: same mounts as a normal
    /// run, plus the host network so the browser can reach the OAuth
    /// callback listener the CLI opens.
    pub fn login_command(&self, repo_root: &Path, agent: &str) -> Result<Command> {
        self.prepare(agent)?;
        let mut cmd = Command::new(self.runtime());
        cmd.args(self.build_run_args(&login_spec(repo_root, agent), true));
        Ok(cmd)
    }

    /// The argument list after `<runtime>`. Pure, so it can be unit-tested
    /// without a container.
    pub fn run_args(&self, spec: &RunSpec) -> Vec<String> {
        self.build_run_args(spec, false)
    }

    fn build_run_args(&self, spec: &RunSpec, host_network: bool) -> Vec<String> {
        let mut args: Vec<String> = vec!["run".into(), "--rm".into()];
        if host_network || spec.host_network {
            args.push("--network".into());
            args.push("host".into());
        }
        if spec.interactive {
            args.push("-it".into());
        } else {
            // Queries such as Codex's stdio RPC need stdin without a TTY.
            // Headless turns still use Stdio::null() on the runtime client.
            args.push("-i".into());
        }
        if let Some(name) = &spec.name {
            args.push("--name".into());
            args.push(name.clone());
        }
        // SELinux: disable labeling for this container rather than
        // relabeling the user's repo and $HOME files with :z/:Z.
        args.push("--security-opt".into());
        args.push("label=disable".into());

        match self.kind {
            SandboxKind::Podman => args.push("--userns=keep-id".into()),
            _ => {
                args.push("--user".into());
                args.push(format!("{}:{}", self.uid, self.gid));
            }
        }
        args.push("-e".into());
        args.push(format!("HOME={CONTAINER_HOME}"));
        // `aic` inside runs agents directly instead of nesting a sandbox.
        args.push("-e".into());
        args.push(format!("{IN_SANDBOX_ENV}=1"));

        let root = path_str(&spec.repo_root);
        let suffix = if spec.access == Access::ReadOnly { ":ro" } else { "" };
        args.push("-v".into());
        args.push(format!("{root}:{root}{suffix}"));
        if spec.access == Access::ReadOnly {
            args.push("--tmpfs".into());
            args.push("/tmp".into());
        }
        if let Some(git_dir) = &spec.git_dir {
            if !git_dir.starts_with(&spec.repo_root) {
                let g = path_str(git_dir);
                args.push("-v".into());
                args.push(format!("{g}:{g}{suffix}"));
            }
        }
        for additional in &spec.additional_paths {
            let path = path_str(&additional.path);
            let suffix = if additional.access == Access::ReadOnly {
                ":ro"
            } else {
                ""
            };
            args.push("-v".into());
            args.push(format!("{path}:{path}{suffix}"));
        }

        let gitconfig = self.host_home.join(".gitconfig");
        if gitconfig.exists() {
            args.push("-v".into());
            args.push(format!("{}:{CONTAINER_HOME}/.gitconfig:ro", path_str(&gitconfig)));
        }
        if let Some(aic) = &self.aic_binary {
            args.push("-v".into());
            args.push(format!("{}:{CONTAINER_AIC}:ro", path_str(aic)));
        }
        if let Some(binaries) = &self.bound_binaries {
            for (name, binary) in binaries {
                args.push("-v".into());
                args.push(format!("{}:/usr/local/bin/{name}:ro", path_str(binary)));
            }
        }
        if let Some(dir) = &self.config_dir {
            // Not under $HOME/.config: the runtime would create that parent
            // root-owned, breaking other tools that write there.
            args.push("-v".into());
            args.push(format!("{}:{CONTAINER_CONFIG}:ro", path_str(dir)));
            args.push("-e".into());
            args.push(format!("{CONFIG_DIR_ENV}={CONTAINER_CONFIG}"));
        }
        self.push_credential_mounts(&mut args, &spec.agent);

        for mount in &self.mounts {
            args.push("-v".into());
            args.push(mount.clone());
        }
        for name in &self.env {
            args.push("-e".into());
            args.push(name.clone());
        }
        // Per-run values come last so a configured pass-through with the
        // same name cannot replace a server-owned value.
        for (name, value) in &spec.env {
            args.push("-e".into());
            args.push(format!("{name}={value}"));
        }

        args.push("-w".into());
        args.push(path_str(&spec.workdir));
        args.push(self.image.clone());
        args.push(spec.agent.clone());
        args.extend(spec.args.iter().cloned());
        args
    }

    /// Agent login state. `claude` gets a container-owned state dir (it
    /// writes sessions/history there, so a plain RO bind of `~/.claude`
    /// would break it) with only the credentials file bound in from the
    /// host, writable so token refreshes persist. `agy` keeps its login in
    /// the host keyring (D-Bus Secret Service), which the container can't
    /// reach, so it gets a container-owned `~/.gemini` with its own login
    /// (file storage) instead of the host's `~/.gemini`.
    fn push_credential_mounts(&self, args: &mut Vec<String>, agent: &str) {
        match agent {
            "claude" => {
                let state = path_str(&self.claude_state_dir());
                args.push("-v".into());
                args.push(format!("{state}:{CONTAINER_HOME}/.claude"));
                // Keeps `.claude.json` inside the persisted state dir.
                args.push("-e".into());
                args.push(format!("CLAUDE_CONFIG_DIR={CONTAINER_HOME}/.claude"));
                let creds = self.host_home.join(".claude").join(".credentials.json");
                if creds.exists() {
                    args.push("-v".into());
                    args.push(format!(
                        "{}:{CONTAINER_HOME}/.claude/.credentials.json",
                        path_str(&creds)
                    ));
                }
            }
            "agy" => {
                args.push("-v".into());
                args.push(format!("{}:{CONTAINER_HOME}/.gemini", path_str(&self.agy_state_dir())));
            }
            "codex" => {
                let codex = self.host_home.join(".codex");
                if codex.exists() {
                    args.push("-v".into());
                    args.push(format!("{}:{CONTAINER_HOME}/.codex", path_str(&codex)));
                }
            }
            _ => {}
        }
    }

    /// `<runtime> build` arguments for `aic sandbox build`.
    pub fn build_args(&self, tag: &str, context: &Path, build_args: &[(String, String)]) -> Vec<String> {
        let mut args = vec!["build".to_string(), "-t".into(), tag.into()];
        for (k, v) in build_args {
            args.push("--build-arg".into());
            args.push(format!("{k}={v}"));
        }
        args.push(path_str(context));
        args
    }

    pub fn runtime_name(&self) -> &'static str {
        self.runtime()
    }
}

fn login_spec(repo_root: &Path, agent: &str) -> RunSpec {
    RunSpec {
        repo_root: repo_root.to_path_buf(),
        git_dir: None,
        workdir: repo_root.to_path_buf(),
        access: Access::ReadOnly,
        interactive: true,
        agent: agent.to_string(),
        args: Vec::new(),
        env: Vec::new(),
        additional_paths: Vec::new(),
        host_network: false,
        name: None,
    }
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(kind: SandboxKind) -> Sandbox {
        Sandbox {
            kind,
            image: "aic-sandbox".into(),
            mounts: vec!["/cache:/cache:ro".into()],
            env: vec!["ANTHROPIC_API_KEY".into()],
            host_home: PathBuf::from("/nonexistent-home"),
            data_dir: PathBuf::from("/nonexistent-home/.local/share/aic/sandbox"),
            uid: 1000,
            gid: 1001,
            aic_binary: Some(PathBuf::from("/host/bin/aic")),
            config_dir: Some(PathBuf::from("/nonexistent-home/.config/aic")),
            bound_binaries: None,
        }
    }

    fn spec(access: Access, interactive: bool) -> RunSpec {
        RunSpec {
            repo_root: PathBuf::from("/work/repo"),
            git_dir: Some(PathBuf::from("/work/repo/.git")),
            workdir: PathBuf::from("/work/repo"),
            access,
            interactive,
            agent: "claude".into(),
            args: vec!["-p".into(), "hi".into()],
            env: Vec::new(),
            additional_paths: Vec::new(),
            host_network: false,
            name: None,
        }
    }

    fn has_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    #[test]
    fn podman_uses_keep_id_and_docker_uses_user() {
        let podman = sandbox(SandboxKind::Podman).run_args(&spec(Access::ReadWrite, false));
        assert!(podman.contains(&"--userns=keep-id".to_string()));
        assert!(!podman.contains(&"--user".to_string()));

        let docker = sandbox(SandboxKind::Docker).run_args(&spec(Access::ReadWrite, false));
        assert!(has_pair(&docker, "--user", "1000:1001"));
        assert!(!docker.contains(&"--userns=keep-id".to_string()));
    }

    #[test]
    fn common_flags_home_and_selinux() {
        let args = sandbox(SandboxKind::Docker).run_args(&spec(Access::ReadWrite, false));
        assert_eq!(&args[..2], ["run", "--rm"]);
        assert!(has_pair(&args, "--security-opt", "label=disable"));
        assert!(has_pair(&args, "-e", "HOME=/home/aic"));
        assert!(has_pair(&args, "-w", "/work/repo"));
        assert!(!args.contains(&"-it".to_string()));
    }

    #[test]
    fn executor_mounts_repo_read_write_and_reviewer_read_only_with_tmpfs() {
        let rw = sandbox(SandboxKind::Docker).run_args(&spec(Access::ReadWrite, false));
        assert!(has_pair(&rw, "-v", "/work/repo:/work/repo"));
        assert!(!rw.contains(&"--tmpfs".to_string()));

        let ro = sandbox(SandboxKind::Docker).run_args(&spec(Access::ReadOnly, false));
        assert!(has_pair(&ro, "-v", "/work/repo:/work/repo:ro"));
        assert!(has_pair(&ro, "--tmpfs", "/tmp"));
    }

    #[test]
    fn worktree_git_dir_outside_repo_is_mounted_with_same_access() {
        let mut s = spec(Access::ReadOnly, false);
        s.git_dir = Some(PathBuf::from("/main/.git"));
        let ro = sandbox(SandboxKind::Docker).run_args(&s);
        assert!(has_pair(&ro, "-v", "/main/.git:/main/.git:ro"));

        s.access = Access::ReadWrite;
        let rw = sandbox(SandboxKind::Docker).run_args(&s);
        assert!(has_pair(&rw, "-v", "/main/.git:/main/.git"));
    }

    #[test]
    fn conversation_paths_keep_their_individual_access() {
        let mut s = spec(Access::ReadWrite, false);
        s.additional_paths = vec![
            AdditionalPath {
                path: "/work/docs".into(),
                access: Access::ReadOnly,
            },
            AdditionalPath {
                path: "/work/api".into(),
                access: Access::ReadWrite,
            },
        ];
        let args = sandbox(SandboxKind::Docker).run_args(&s);
        assert!(has_pair(&args, "-v", "/work/docs:/work/docs:ro"));
        assert!(has_pair(&args, "-v", "/work/api:/work/api"));
    }

    #[test]
    fn git_dir_inside_repo_is_not_mounted_twice() {
        let args = sandbox(SandboxKind::Docker).run_args(&spec(Access::ReadWrite, false));
        assert!(!args.iter().any(|a| a.starts_with("/work/repo/.git")));
    }

    #[test]
    fn aic_inside_gets_the_host_binary_config_and_no_nested_sandbox() {
        let args = sandbox(SandboxKind::Podman).run_args(&spec(Access::ReadOnly, false));
        assert!(has_pair(&args, "-e", "AIC_IN_SANDBOX=1"));
        assert!(has_pair(&args, "-v", "/host/bin/aic:/usr/local/bin/aic:ro"));
        assert!(has_pair(&args, "-v", "/nonexistent-home/.config/aic:/etc/aic:ro"));
        assert!(has_pair(&args, "-e", "AIC_CONFIG_DIR=/etc/aic"));

        let mut bare = sandbox(SandboxKind::Docker);
        bare.aic_binary = None;
        bare.config_dir = None;
        let args = bare.run_args(&spec(Access::ReadWrite, false));
        assert!(!args.iter().any(|a| a.contains("/usr/local/bin/aic") || a.contains("/etc/aic")));
        assert!(has_pair(&args, "-e", "AIC_IN_SANDBOX=1"));
    }

    #[test]
    fn named_runs_can_be_killed_by_name() {
        let mut s = spec(Access::ReadWrite, false);
        assert!(!sandbox(SandboxKind::Docker).run_args(&s).contains(&"--name".to_string()));
        s.name = Some("aic-1-2".into());
        assert!(has_pair(&sandbox(SandboxKind::Docker).run_args(&s), "--name", "aic-1-2"));
        let kill = sandbox(SandboxKind::Podman).kill_command("aic-1-2");
        assert_eq!(kill.get_program(), "podman");
        assert_eq!(kill.get_args().collect::<Vec<_>>(), ["kill", "aic-1-2"]);
    }

    #[test]
    fn binds_all_host_agents_read_only_even_when_claude_is_selected() {
        let mut bound = sandbox(SandboxKind::Podman);
        bound.bound_binaries = Some(vec![
            ("agy".into(), "/host/bin/agy".into()),
            ("codex".into(), "/host/releases/codex".into()),
            ("codex-code-mode-host".into(), "/host/releases/codex-code-mode-host".into()),
        ]);
        let args = bound.run_args(&spec(Access::ReadWrite, false));
        assert!(has_pair(&args, "-v", "/host/bin/agy:/usr/local/bin/agy:ro"));
        assert!(has_pair(&args, "-v", "/host/releases/codex:/usr/local/bin/codex:ro"));
        assert!(has_pair(&args, "-v", "/host/releases/codex-code-mode-host:/usr/local/bin/codex-code-mode-host:ro"));
        assert!(bound.validate_bound_agent("codex").is_ok());
        assert!(bound.validate_bound_agent("agy").is_ok());
        let unbound = sandbox(SandboxKind::Podman).run_args(&spec(Access::ReadWrite, false));
        assert!(!unbound.iter().any(|arg| arg.contains("/usr/local/bin/codex") || arg.contains("/usr/local/bin/agy")));
    }

    #[test]
    fn bind_mode_requires_codex_helper_and_does_not_fall_back_to_image() {
        let mut bound = sandbox(SandboxKind::Docker);
        bound.bound_binaries = Some(vec![("codex".into(), "/host/bin/codex".into())]);
        assert!(bound.validate_bound_agent("codex").unwrap_err().to_string().contains("codex-code-mode-host"));
        assert!(bound.validate_bound_agent("agy").unwrap_err().to_string().contains("agy"));
        assert!(bound.validate_bound_agent("claude").is_ok());
        assert!(sandbox(SandboxKind::Docker).validate_bound_agent("codex").is_ok());
    }

    #[test]
    fn interactive_adds_tty_flags() {
        let args = sandbox(SandboxKind::Podman).run_args(&spec(Access::ReadWrite, true));
        assert!(args.contains(&"-it".to_string()));
    }

    #[test]
    fn extra_mounts_and_env_names_are_forwarded() {
        let args = sandbox(SandboxKind::Docker).run_args(&spec(Access::ReadWrite, false));
        assert!(has_pair(&args, "-v", "/cache:/cache:ro"));
        assert!(has_pair(&args, "-e", "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn claude_gets_state_dir_and_config_dir_but_not_host_claude_dir() {
        let args = sandbox(SandboxKind::Docker).run_args(&spec(Access::ReadWrite, false));
        assert!(has_pair(
            &args,
            "-v",
            "/nonexistent-home/.local/share/aic/sandbox/claude:/home/aic/.claude"
        ));
        assert!(has_pair(&args, "-e", "CLAUDE_CONFIG_DIR=/home/aic/.claude"));
        assert!(!args.iter().any(|a| a.starts_with("/nonexistent-home/.claude:")));
    }

    #[test]
    fn agy_gets_container_owned_gemini_dir_not_host_one() {
        let mut spec = spec(Access::ReadWrite, false);
        spec.agent = "agy".into();
        let args = sandbox(SandboxKind::Docker).run_args(&spec);
        assert!(has_pair(
            &args,
            "-v",
            "/nonexistent-home/.local/share/aic/sandbox/agy:/home/aic/.gemini"
        ));
        assert!(!args.iter().any(|a| a.starts_with("/nonexistent-home/.gemini:")));
    }

    #[test]
    fn login_command_uses_host_network_and_interactive_agy() {
        let args = sandbox(SandboxKind::Docker).build_run_args(&login_spec(Path::new("/work/repo"), "agy"), true);
        assert!(has_pair(&args, "--network", "host"));
        assert!(args.contains(&"-it".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("agy"));
        assert!(has_pair(&args, "-v", "/nonexistent-home/.local/share/aic/sandbox/agy:/home/aic/.gemini"));
    }

    #[test]
    fn image_binary_and_agent_args_come_last() {
        let args = sandbox(SandboxKind::Docker).run_args(&spec(Access::ReadWrite, false));
        let n = args.len();
        assert_eq!(&args[n - 4..], ["aic-sandbox", "claude", "-p", "hi"]);
    }

    #[test]
    fn build_args_include_tag_build_args_and_context() {
        let args = sandbox(SandboxKind::Docker).build_args(
            "mytag",
            Path::new("/ctx"),
            &[("CLAUDE_VERSION".into(), "1.2.3".into())],
        );
        assert_eq!(
            args,
            ["build", "-t", "mytag", "--build-arg", "CLAUDE_VERSION=1.2.3", "/ctx"]
        );
    }
}
