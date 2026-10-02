use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::agent::{AgentProcess, CliAgent, RunOpts};
use crate::chat::{self, ChatEvent};
use crate::config::{Config, SandboxKind};
use crate::sandbox::Sandbox;

const INDEX_HTML: &str = include_str!("../web/chat.html");
const CHAT_CSS: &str = include_str!("../web/chat.css");
const CHAT_JS: &str = include_str!("../web/chat.js");
const FAVICON_SVG: &str = include_str!("../web/favicon.svg");

/// Cap on the request body, so a stray client can't make us buffer forever.
const MAX_BODY: u64 = 1 << 20;

/// How often an idle turn writes a blank line, so a closed tab is noticed
/// (and its agent killed) even while the agent is silent.
const HEARTBEAT: Duration = Duration::from_secs(2);

struct App {
    repo_root: PathBuf,
    config: Config,
    /// Host headers we accept; guards against DNS rebinding when listening
    /// on loopback. `None` when bound elsewhere (the user opted in).
    allowed_hosts: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ChatRequest {
    agent: String,
    prompt: String,
    #[serde(default)]
    session_id: Option<String>,
    /// Persona / group-chat framing from the page, sent on every turn.
    #[serde(default)]
    system: Option<String>,
    /// This participant may edit the repo (writable mount, edit permissions).
    #[serde(default)]
    write: bool,
    /// Per-participant overrides of the agent's configured model/effort.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
}

pub fn run(repo_root: &Path, host: &str, port: u16) -> Result<()> {
    let config = Config::load_merged(repo_root)?;
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "::1");
    let allowed_hosts = loopback.then(|| {
        vec![format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")]
    });
    let app = Arc::new(App { repo_root: repo_root.to_path_buf(), config, allowed_hosts });

    let server = tiny_http::Server::http((host, port))
        .map_err(|e| anyhow!("binding {host}:{port}: {e}"))?;
    println!("aic chat on http://{host}:{port}  (repo: {})", repo_root.display());
    if !loopback {
        eprintln!(
            "warning: listening on {host}: anyone who can reach this port can run your agents \
             (skip_permissions = {}).",
            app.config.skip_permissions
        );
    }

    for request in server.incoming_requests() {
        let app = Arc::clone(&app);
        thread::spawn(move || {
            if let Err(err) = handle(&app, request) {
                eprintln!("request failed: {err:#}");
            }
        });
    }
    Ok(())
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}

fn header_value(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn respond_text(request: Request, status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Result<()> {
    let response = Response::from_data(body.into())
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"));
    request.respond(response)?;
    Ok(())
}

fn handle(app: &App, mut request: Request) -> Result<()> {
    if let Some(allowed) = &app.allowed_hosts {
        let ok = header_value(&request, "Host").is_some_and(|h| allowed.contains(&h));
        if !ok {
            return respond_text(request, 403, "text/plain", "forbidden host");
        }
    }

    let path = request.url().split('?').next().unwrap_or("/").to_string();
    match (request.method().clone(), path.as_str()) {
        (Method::Get, "/") => respond_text(request, 200, "text/html; charset=utf-8", INDEX_HTML),
        (Method::Get, "/chat.css") => respond_text(request, 200, "text/css; charset=utf-8", CHAT_CSS),
        (Method::Get, "/chat.js") => respond_text(request, 200, "text/javascript; charset=utf-8", CHAT_JS),
        (Method::Get, "/favicon.svg" | "/favicon.ico") => {
            respond_text(request, 200, "image/svg+xml", FAVICON_SVG)
        }
        (Method::Get, "/api/agents") => {
            let mut names: Vec<&String> = app.config.agents.keys().collect();
            names.sort();
            let details: serde_json::Map<String, serde_json::Value> = names
                .iter()
                .map(|name| {
                    let cfg = &app.config.agents[*name];
                    let kind = if CliAgent::new(name.as_str(), cfg.binary.as_str()).is_agy() { "agy" } else { "claude" };
                    let info = serde_json::json!({ "kind": kind, "model": cfg.model, "effort": cfg.effort });
                    (name.to_string(), info)
                })
                .collect();
            let body = serde_json::json!({
                "agents": names,
                "details": details,
                "default": app.config.executor,
                "repo": app.repo_root.display().to_string(),
                "sandboxed": app.config.sandbox != SandboxKind::None,
            });
            respond_text(request, 200, "application/json", body.to_string())
        }
        (Method::Get, "/api/skills") => {
            let body = serde_json::to_string(&list_skills(app))?;
            respond_text(request, 200, "application/json", body)
        }
        (Method::Post, "/api/chat") => {
            // A cross-site page can POST text/plain without a preflight, so
            // insist on JSON and on a same-origin Origin when one is sent.
            let json = header_value(&request, "Content-Type")
                .is_some_and(|c| c.starts_with("application/json"));
            let same_origin = match (header_value(&request, "Origin"), header_value(&request, "Host")) {
                (Some(origin), Some(host)) => origin == format!("http://{host}"),
                (Some(_), None) => false,
                (None, _) => true,
            };
            if !json || !same_origin {
                return respond_text(request, 403, "text/plain", "forbidden");
            }

            let mut body = String::new();
            request.as_reader().take(MAX_BODY).read_to_string(&mut body)?;
            let req: ChatRequest = match serde_json::from_str(&body) {
                Ok(req) => req,
                Err(err) => return respond_text(request, 400, "text/plain", format!("bad request: {err}")),
            };
            chat_turn(app, request, req)
        }
        _ => respond_text(request, 404, "text/plain", "not found"),
    }
}

/// One entry of `GET /api/skills`, read from a `SKILL.md` frontmatter.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Skill {
    name: String,
    description: String,
    argument_hint: String,
    /// Runs only for a participant with edit access: an explicit
    /// `aic-requires-write` field, else `allowed-tools` naming an editing tool.
    requires_write: bool,
}

/// Skills the agents can see: the repo's `.claude/skills/`, plus
/// `~/.claude/skills/` when agents run on the host (the sandbox doesn't
/// mount it). Repo skills win on a name clash.
fn list_skills(app: &App) -> Vec<Skill> {
    let mut dirs = vec![app.repo_root.join(".claude").join("skills")];
    if app.config.sandbox == SandboxKind::None {
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join(".claude").join("skills"));
        }
    }
    let mut skills: Vec<Skill> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path().join("SKILL.md")) else { continue };
            let fallback = entry.file_name().to_string_lossy().into_owned();
            if let Some(skill) = parse_skill(&text, &fallback) {
                if !skills.iter().any(|s| s.name == skill.name) {
                    skills.push(skill);
                }
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Reads the few frontmatter fields the chat needs. Not a YAML parser: one
/// `key: value` per line, plus `allowed-tools` as a `- item` list or inline.
fn parse_skill(text: &str, fallback_name: &str) -> Option<Skill> {
    let text = text.replace("\r\n", "\n");
    let front = text.strip_prefix("---\n")?;
    let front = &front[..front.find("\n---")?];
    let unquote = |v: &str| v.trim().trim_matches(|c: char| c == '"' || c == '\'').to_string();

    let (mut name, mut description, mut hint, mut explicit_write) = (None, String::new(), String::new(), None);
    let mut tools: Vec<String> = Vec::new();
    let mut key = String::new();
    for line in front.lines() {
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            if key == "allowed-tools" {
                tools.push(unquote(item));
            }
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        key = k.trim().to_string();
        match key.as_str() {
            "name" => name = Some(unquote(v)).filter(|n| !n.is_empty()),
            "description" => description = unquote(v),
            "argument-hint" => hint = unquote(v),
            "aic-requires-write" => explicit_write = Some(unquote(v) == "true"),
            "allowed-tools" => {
                let inline = v.trim().trim_start_matches('[').trim_end_matches(']');
                tools.extend(inline.split(',').map(unquote).filter(|t| !t.is_empty()));
            }
            _ => {}
        }
    }
    // "Bash(git:*)" names the Bash tool.
    let edits = tools.iter().any(|t| {
        let tool = t.split('(').next().unwrap_or("").trim();
        matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
    });
    Some(Skill {
        name: name.unwrap_or_else(|| fallback_name.to_string()),
        description,
        argument_hint: hint,
        requires_write: explicit_write.unwrap_or(edits),
    })
}

/// A `Read` fed by a channel, so tiny_http sends whatever the agent thread
/// produces as it arrives (chunked, since the length is unknown).
struct ChannelReader {
    rx: Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn send(tx: &Sender<Vec<u8>>, event: &ChatEvent) -> bool {
    let mut line = serde_json::to_vec(event).expect("ChatEvent serializes");
    line.push(b'\n');
    tx.send(line).is_ok()
}

fn build_agent(app: &App, name: &str) -> Result<CliAgent> {
    let cfg = app.config.agent(name)?;
    Ok(CliAgent::with_defaults(
        name.to_string(),
        cfg.binary.clone(),
        cfg.model.clone(),
        cfg.effort.clone(),
    )
    .with_sandbox(Sandbox::from_config(&app.config)?))
}

fn failed(error: String) -> ChatEvent {
    ChatEvent::Done { ok: false, text: String::new(), error: Some(error), usage: None }
}

/// Answers one chat turn as NDJSON: `session`, then `delta`s, then `done`.
/// Blank lines in between are heartbeats.
fn chat_turn(app: &App, request: Request, req: ChatRequest) -> Result<()> {
    if req.prompt.trim().is_empty() {
        return respond_text(request, 400, "text/plain", "empty prompt");
    }
    let agent = match build_agent(app, &req.agent) {
        Ok(agent) => agent,
        Err(err) => return respond_text(request, 400, "text/plain", format!("{err:#}")),
    };
    let sandboxed = app.config.sandbox != SandboxKind::None;
    let resume = req.session_id.filter(|s| !s.is_empty());

    // Tell the agent what it may do, so it says so instead of hunting for a
    // workaround (copying files to /tmp, writing patches nobody can apply).
    let access_note = if req.write {
        "You may edit files in this repository when the user asks you to."
    } else if sandboxed {
        "This chat has the repository mounted read-only: you cannot edit files. If asked to change something, don't look for a workaround (no copies in /tmp, no patch files); reply with the proposed change as text and say that someone with edit access has to apply it."
    } else {
        "You are not allowed to edit files in this chat. If asked to change something, reply with the proposed change as text instead of making it."
    };
    let system = [Some(access_note), req.system.as_deref().filter(|s| !s.trim().is_empty())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut prompt = req.prompt;
    let mut system_prompt = Some(system.clone());
    if agent.is_agy() {
        // No system-prompt flag: fold it into the first turn, which the
        // session then remembers.
        if resume.is_none() {
            prompt = format!("{system}\n\n{prompt}");
        }
        system_prompt = None;
    }

    let opts = RunOpts {
        prompt,
        cwd: Some(app.repo_root.clone()),
        system_prompt,
        model: req.model.filter(|s| !s.trim().is_empty()),
        effort: req.effort.filter(|s| !s.trim().is_empty()),
        // A read-only participant can still read: the sandbox's read-only
        // mount makes skipping prompts safe. Without a sandbox, fall back to
        // plan mode (no edits) and keep prompts on.
        skip_permissions: app.config.skip_permissions && (req.write || sandboxed),
        permission: if req.write {
            Some(app.config.permission_mode.clone())
        } else if sandboxed {
            None
        } else {
            Some("plan".to_string())
        },
        read_only: !req.write,
        resume,
        stream: true,
        ..Default::default()
    };

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let response = Response::new(
        StatusCode(200),
        vec![
            header("Content-Type", "application/x-ndjson"),
            header("Cache-Control", "no-store"),
            header("X-Accel-Buffering", "no"),
        ],
        ChannelReader { rx, buf: Vec::new(), pos: 0 },
        None,
        None,
    );
    let responder = thread::spawn(move || request.respond(response));

    match agent.spawn_stream(&opts) {
        Err(err) => {
            send(&tx, &failed(format!("{err:#}")));
        }
        Ok(process) => {
            // Stop in the page closes the connection. Writes are what notice
            // that, so keep writing while the agent is quiet, and kill it
            // (with everything it started) as soon as one fails.
            let finished = Arc::new(AtomicBool::new(false));
            let watcher = {
                let (tx, killer, finished) = (tx.clone(), process.killer.clone(), Arc::clone(&finished));
                thread::spawn(move || {
                    while !finished.load(Ordering::Relaxed) {
                        thread::park_timeout(HEARTBEAT);
                        if finished.load(Ordering::Relaxed) {
                            break;
                        }
                        if tx.send(b"\n".to_vec()).is_err() {
                            killer.kill();
                            break;
                        }
                    }
                })
            };
            let result = pump_agent(agent.is_agy(), process, &tx);
            finished.store(true, Ordering::Relaxed);
            watcher.thread().unpark();
            let _ = watcher.join();
            if let Err(err) = result {
                send(&tx, &failed(format!("{err:#}")));
            }
        }
    }
    drop(tx);
    responder.join().map_err(|_| anyhow!("response thread panicked"))??;
    Ok(())
}

fn pump_agent(is_agy: bool, process: AgentProcess, tx: &Sender<Vec<u8>>) -> Result<()> {
    let AgentProcess { mut child, killer } = process;
    let stdout = child.stdout.take().context("agent stdout was not piped")?;
    let mut stderr = child.stderr.take().context("agent stderr was not piped")?;
    let stderr_thread = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let mut streamed = false;
    let mut finished = false;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Some(event) = chat::parse_line(is_agy, &line) else { continue };
        let event = match event {
            // The agent only reports the reply at the end (no deltas seen):
            // surface it as one chunk so the page has something to render.
            ChatEvent::Done { ok: true, ref text, .. } if !streamed && !text.is_empty() => {
                if !send(tx, &ChatEvent::Delta { text: text.clone() }) {
                    killer.kill();
                    break;
                }
                event
            }
            ChatEvent::Delta { .. } => {
                streamed = true;
                event
            }
            other => other,
        };
        finished |= matches!(event, ChatEvent::Done { .. });
        if !send(tx, &event) {
            // The browser went away; don't leave the agent running.
            killer.kill();
            break;
        }
    }

    let status = child.wait()?;
    let stderr_text = stderr_thread.join().unwrap_or_default();
    if !finished {
        let detail = stderr_text.trim();
        bail!(
            "agent exited with {status} without finishing the turn{}",
            if detail.is_empty() { String::new() } else { format!(": {detail}") }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_frontmatter_with_list_tools() {
        let text = "---\nname: aic-plan\ndescription: \"AIC: Scaffold a plan\"\nargument-hint: \"<slug> [title]\"\nallowed-tools:\n  - Bash\n  - Read\n  - Edit\n---\n\nbody";
        assert_eq!(
            parse_skill(text, "dir"),
            Some(Skill {
                name: "aic-plan".into(),
                description: "AIC: Scaffold a plan".into(),
                argument_hint: "<slug> [title]".into(),
                requires_write: true,
            })
        );
    }

    #[test]
    fn skill_inline_tools_explicit_flag_and_fallback_name() {
        let read_only = "---\ndescription: look around\nallowed-tools: [Read, \"Bash(git:*)\"]\n---\n";
        let skill = parse_skill(read_only, "peek").unwrap();
        assert_eq!(skill.name, "peek");
        assert!(!skill.requires_write);

        let flagged = "---\r\nname: x\r\nallowed-tools: Read\r\naic-requires-write: true\r\n---\r\n";
        assert!(parse_skill(flagged, "x").unwrap().requires_write);

        let overridden = "---\nname: y\nallowed-tools:\n  - Edit\naic-requires-write: false\n---\n";
        assert!(!parse_skill(overridden, "y").unwrap().requires_write);

        assert_eq!(parse_skill("no frontmatter", "z"), None);
    }
}
