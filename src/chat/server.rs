use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::agent::{AgentProcess, CliAgent, RunOpts};
use crate::chat::store::{self as chat, ChatEvent};
use crate::{consumption, quotas, diagnostics};
use crate::config::{Config, SandboxKind};
use crate::sandbox::Sandbox;

const INDEX_HTML: &str = include_str!("web/chat.html");
const CHAT_CSS: &str = include_str!("web/chat.css");
const CHAT_JS: &str = include_str!("web/chat.js");
const CHAT_IMPORT_JS: &str = include_str!("web/chat-import.js");
const CHAT_QUOTAS_JS: &str = include_str!("web/chat-quotas.js");
const ONBOARDING_JS: &str = include_str!("web/chat-onboarding.js");
const FAVICON_SVG: &str = include_str!("web/favicon.svg");

/// Cap on the request body, so a stray client can't make us buffer forever.
const MAX_BODY: u64 = 1 << 20;

/// Chat state is deliberately capped separately from prompts.  It prevents a
/// malformed local page from filling the repository disk in one request while
/// still leaving ample room for a long multi-agent conversation.
const MAX_CHAT_STATE: u64 = 64 << 20;

/// How often an idle turn writes a blank line, so a closed tab is noticed
/// (and its agent killed) even while the agent is silent.
const HEARTBEAT: Duration = Duration::from_secs(2);

/// How long a CLI gets to list its models before the dialog falls back to its built-in suggestions.
const LISTING_TIMEOUT: Duration = Duration::from_secs(8);

struct App {
    repo_root: PathBuf,
    config: RwLock<Config>,
    /// The browser is a cache; this database is the durable source of chat
    /// state.  A mutex also makes writes deterministic across request threads.
    chat_db: Mutex<Connection>,
    /// Host headers we accept; guards against DNS rebinding when listening
    /// on loopback. `None` when bound elsewhere (the user opted in).
    allowed_hosts: Option<Vec<String>>,
    /// Models each agent CLI reported, so the dialog does not spawn it every time.
    models: Mutex<HashMap<String, chat::ModelCatalog>>,
    quotas: Mutex<HashMap<String, QuotaCache>>,
}

struct QuotaCache {
    signature: String,
    status: quotas::QuotaStatus,
}

#[derive(Deserialize)]
struct ChatStateRequest {
    state: serde_json::Value,
    drafts: serde_json::Value,
}

#[derive(Deserialize)]
struct PreflightRequest {
    agents: Vec<String>,
    #[serde(default)]
    checkout: Option<String>,
}

#[derive(Deserialize)]
struct ChatRequest {
    agent: String,
    prompt: String,
    #[serde(default)]
    checkout: Option<String>,
    #[serde(default)]
    git_context: Option<diagnostics::GitContext>,
    /// Omitted by auxiliary requests (e.g. title generation). Historical
    /// imported/copied entries never trigger accounting.
    #[serde(default)]
    conversation_id: Option<String>,
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

/// The settings the chat dialog edits (`[chat]`, `[permissions]` and
/// `[sandbox]`; the workflow ones live in `aic config`). They are saved to
/// the global config; a repository override can still take precedence at
/// runtime.
#[derive(Deserialize)]
struct SettingsRequest {
    default_agent: String,
    #[serde(default)]
    confirm_default: bool,
    permission_mode: String,
    skip_permissions: bool,
    sandbox: SandboxKind,
    sandbox_image: String,
}

pub fn run(repo_root: &Path, host: &str, port: u16, open: bool) -> Result<()> {
    let config = Config::load_merged(repo_root)?;
    let chat_db = open_chat_db(repo_root)?;
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "::1");
    let allowed_hosts = loopback.then(|| {
        vec![format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")]
    });
    let app = Arc::new(App {
        repo_root: repo_root.to_path_buf(), config: RwLock::new(config), chat_db: Mutex::new(chat_db),
        allowed_hosts, models: Mutex::default(), quotas: Mutex::default(),
    });

    let server = tiny_http::Server::http((host, port))
        .map_err(|e| anyhow!("binding {host}:{port}: {e}"))?;
    println!("aic chat on http://{host}:{port}  (repo: {})", repo_root.display());
    if open { crate::common::browser::open(&format!("http://{}:{port}",if host == "::1" {"[::1]"} else {host})); }
    if !loopback {
        eprintln!(
            "warning: listening on {host}: anyone who can reach this port can run your agents \
             (skip_permissions = {}).",
            current_config(&app).permissions.skip
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

fn open_chat_db(repo_root: &Path) -> Result<Connection> {
    let path = repo_root.join(".aic").join("chat").join("chat.sqlite3");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let db = Connection::open(&path).with_context(|| format!("opening {}", path.display()))?;
    db.busy_timeout(Duration::from_secs(5))?;
    // One compact document is intentional: the web client already has a rich,
    // evolving conversation graph (forks, participant cursors and native CLI
    // sessions).  SQLite gives it atomic, durable replacement without coupling
    // UI migrations to a relational schema.
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS chat_state (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             state_json TEXT NOT NULL,
             drafts_json TEXT NOT NULL,
             updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS native_session_context (
             agent TEXT NOT NULL, session_id TEXT NOT NULL, checkout TEXT NOT NULL,
             PRIMARY KEY (agent, session_id)
         );",
    )?;
    consumption::init(&db)?;
    Ok(db)
}

fn load_chat_state(app: &App) -> Result<serde_json::Value> {
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    let row: Option<(String, String)> = db
        .query_row("SELECT state_json, drafts_json FROM chat_state WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    match row {
        Some((state, drafts)) => Ok(serde_json::json!({
            "state": serde_json::from_str::<serde_json::Value>(&state)?,
            "drafts": serde_json::from_str::<serde_json::Value>(&drafts)?,
        })),
        None => Ok(serde_json::json!({ "state": null, "drafts": null })),
    }
}

fn save_chat_state(app: &App, req: ChatStateRequest) -> Result<()> {
    if !req.state.is_object() || !req.drafts.is_object() {
        bail!("chat state and drafts must be JSON objects");
    }
    let state = serde_json::to_string(&req.state)?;
    let drafts = serde_json::to_string(&req.drafts)?;
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    db.execute(
        "INSERT INTO chat_state (id, state_json, drafts_json, updated_at) VALUES (1, ?1, ?2, unixepoch())
         ON CONFLICT(id) DO UPDATE SET state_json = excluded.state_json, drafts_json = excluded.drafts_json, updated_at = excluded.updated_at",
        params![state, drafts],
    )?;
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

fn current_config(app: &App) -> Config {
    app.config.read().expect("chat config lock poisoned").clone()
}

fn config_json(config: &Config) -> serde_json::Value {
    let mut agents: Vec<&String> = config.agents.keys().collect();
    agents.sort();
    serde_json::json!({
        "default_agent": config.chat_default_agent(),
        "permission_mode": config.permissions.mode,
        "skip_permissions": config.permissions.skip,
        "sandbox": config.sandbox.runtime,
        "sandbox_image": config.sandbox.image,
        "agents": agents,
    })
}

fn same_origin(request: &Request) -> bool {
    match (header_value(request, "Origin"), header_value(request, "Host")) {
        (Some(origin), Some(host)) => origin == format!("http://{host}"),
        (Some(_), None) => false,
        (None, _) => true,
    }
}

fn respond_text(request: Request, status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Result<()> {
    let response = Response::from_data(body.into())
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"));
    request.respond(response)?;
    Ok(())
}

fn handle(app: &Arc<App>, mut request: Request) -> Result<()> {
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
        (Method::Get, "/chat-onboarding.js") => respond_text(request, 200, "text/javascript; charset=utf-8", ONBOARDING_JS),
        (Method::Get, "/chat.js") => respond_text(request, 200, "text/javascript; charset=utf-8", CHAT_JS),
        (Method::Get, "/chat-import.js") => respond_text(request, 200, "text/javascript; charset=utf-8", CHAT_IMPORT_JS),
        (Method::Get, "/chat-quotas.js") => respond_text(request, 200, "text/javascript; charset=utf-8", CHAT_QUOTAS_JS),
        (Method::Get, "/favicon.svg" | "/favicon.ico") => {
            respond_text(request, 200, "image/svg+xml", FAVICON_SVG)
        }
        (Method::Get, "/api/worktrees") => {
            let trees = crate::common::workspaces::list(&app.repo_root)?;
            respond_text(request, 200, "application/json", serde_json::to_string(&trees)?)
        }
        (Method::Post, "/api/chat/preflight") => {
            if !same_origin(&request) { return respond_text(request, 403, "text/plain", "forbidden"); }
            let req: PreflightRequest = match serde_json::from_reader(request.as_reader().take(64 * 1024)) {
                Ok(req) => req,
                Err(_) => return respond_text(request, 400, "text/plain", "Invalid preflight request"),
            };
            let root = match crate::common::workspaces::resolve(&app.repo_root, req.checkout.as_deref()) {
                Ok(root) => root,
                Err(_) => return respond_text(request, 400, "text/plain", "Select a registered worktree"),
            };
            let config = Config::load_merged(&root)?;
            if req.agents.len() > config.agents.len() || req.agents.iter().any(|name| !config.agents.contains_key(name)) {
                return respond_text(request, 400, "text/plain", "Select configured recipient agents");
            }
            let report = diagnostics::for_send(&root, &config, &req.agents);
            respond_text(request, 200, "application/json", serde_json::to_string(&report)?)
        }
        (Method::Get, "/api/diagnostics") | (Method::Post, "/api/diagnostics/refresh") => {
            if request.method() == &Method::Post && !same_origin(&request) { return respond_text(request, 403, "text/plain", "forbidden"); }
            let requested = query_value(request.url(), "checkout");
            let root = match crate::common::workspaces::resolve(&app.repo_root, requested.as_deref()) {
                Ok(root) => root, Err(_) => return respond_text(request, 400, "text/plain", "Select a registered worktree of this repository"),
            };
            let config = Config::load_merged(&root)?;
            if root == app.repo_root { *app.config.write().expect("chat config lock poisoned") = config.clone(); }
            let mut report = diagnostics::snapshot(&root, &config);
            // Reuse the existing safe quota cache, without triggering generation.
            let cached = app.quotas.lock().expect("quota cache lock poisoned");
            for agent in &mut report.agents {
                if let Some(q) = cached.get(&agent.agent)
                    && q.signature == quota_signature(&config, &agent.agent) { agent.quota = if q.status.windows.iter().any(|w| w.used_pct.is_some_and(|p| p >= 100.0)) { "exhausted" } else if q.status.windows.iter().any(|w| w.used_pct.is_some_and(|p| p >= 80.0)) { "near_limit" } else if !q.status.windows.is_empty() { "available" } else { "unavailable" }.into();
                }
            }
            respond_text(request, 200, "application/json", serde_json::to_string(&report)?)
        }
        (Method::Get, "/api/git") => {
            let selected = query_value(request.url(), "checkout");
            let root = match crate::common::workspaces::resolve(&app.repo_root, selected.as_deref()) {
                Ok(root) => root, Err(_) => return respond_text(request, 400, "text/plain", "Select a registered worktree"),
            };
            match diagnostics::git_context(&root) {
                Ok(git) => respond_text(request, 200, "application/json", serde_json::to_string(&git)?),
                Err(_) => respond_text(request, 503, "text/plain", "Git context unavailable; check the checkout in the terminal"),
            }
        }
        (Method::Get, "/api/agents") => {
            let selected = query_value(request.url(), "checkout");
            let root = match crate::common::workspaces::resolve(&app.repo_root, selected.as_deref()) {
                Ok(root) => root, Err(_) => return respond_text(request, 400, "text/plain", "Select a registered worktree"),
            };
            let config = Config::load_merged(&root)?;
            let mut names: Vec<&String> = config.agents.keys().collect();
            names.sort();
            let details: serde_json::Map<String, serde_json::Value> = names
                .iter()
                .map(|name| {
                    let cfg = &config.agents[*name];
                    let agent = CliAgent::new(name.as_str(), cfg.binary.as_str());
                    let kind = if agent.is_codex() { "codex" } else if agent.is_agy() { "agy" } else { "claude" };
                    let info = serde_json::json!({ "kind": kind, "model": cfg.model, "effort": cfg.effort });
                    (name.to_string(), info)
                })
                .collect();
            let body = serde_json::json!({
                "agents": names,
                "details": details,
                "default": config.chat_default_agent(),
                "repo": root.display().to_string(),
                "sandboxed": config.sandbox.runtime != SandboxKind::None,
            });
            respond_text(request, 200, "application/json", body.to_string())
        }
        (Method::Get, "/api/quotas") => {
            let conversation = query_value(request.url(), "conversation").unwrap_or_default();
            let statuses = quota_snapshot(app);
            let totals = consumption::load(&app.chat_db.lock().expect("chat database lock poisoned"), &conversation)?;
            respond_text(request, 200, "application/json", serde_json::json!({"agents":statuses,"consumption":totals}).to_string())
        }
        (Method::Post, "/api/quotas/refresh") => {
            if !same_origin(&request) || !header_value(&request, "Content-Type").is_some_and(|c| c.starts_with("application/json")) {
                return respond_text(request, 403, "text/plain", "forbidden");
            }
            let mut body = String::new();
            request.as_reader().take(MAX_BODY).read_to_string(&mut body)?;
            #[derive(Deserialize)]
            struct Refresh { agent: Option<String> }
            let req: Refresh = match serde_json::from_str(&body) {
                Ok(req) => req,
                Err(_) => return respond_text(request, 400, "text/plain", "invalid refresh request"),
            };
            let config = current_config(app);
            if let Some(agent) = req.agent {
                if !config.agents.contains_key(&agent) { return respond_text(request, 404, "text/plain", "unknown agent"); }
                refresh_quota(app, &agent);
            } else {
                for agent in config.agents.keys() { refresh_quota(app, agent); }
            }
            respond_text(request, 202, "application/json", "{}")
        }
        (Method::Get, "/api/models") => {
            let agent = request
                .url()
                .split_once('?')
                .and_then(|(_, query)| query.split('&').find_map(|kv| kv.strip_prefix("agent=")))
                .unwrap_or("")
                .to_string();
            match agent_models(app, &agent) {
                Some(catalog) => respond_text(request, 200, "application/json", serde_json::to_string(&catalog)?),
                None => respond_text(request, 404, "text/plain", "unknown agent"),
            }
        }
        (Method::Get, "/api/config") => {
            respond_text(request, 200, "application/json", config_json(&current_config(app)).to_string())
        }
        (Method::Get, "/api/skills") => {
            let selected = query_value(request.url(), "checkout");
            let root = match crate::common::workspaces::resolve(&app.repo_root, selected.as_deref()) {
                Ok(root) => root, Err(_) => return respond_text(request, 400, "text/plain", "Select a registered worktree"),
            };
            let body = serde_json::to_string(&list_skills(&root, &Config::load_merged(&root)?))?;
            respond_text(request, 200, "application/json", body)
        }
        (Method::Get, "/api/chat/state") => {
            respond_text(request, 200, "application/json", load_chat_state(app)?.to_string())
        }
        (Method::Post, "/api/chat/state") => {
            let json = header_value(&request, "Content-Type")
                .is_some_and(|c| c.starts_with("application/json"));
            if !json || !same_origin(&request) {
                return respond_text(request, 403, "text/plain", "forbidden");
            }
            let mut body = String::new();
            request.as_reader().take(MAX_CHAT_STATE + 1).read_to_string(&mut body)?;
            if body.len() as u64 > MAX_CHAT_STATE {
                return respond_text(request, 413, "text/plain", "chat state is too large");
            }
            let req: ChatStateRequest = match serde_json::from_str(&body) {
                Ok(req) => req,
                Err(err) => return respond_text(request, 400, "text/plain", format!("bad chat state: {err}")),
            };
            match save_chat_state(app, req) {
                Ok(()) => respond_text(request, 204, "text/plain", Vec::new()),
                Err(err) => respond_text(request, 500, "text/plain", format!("saving chat state: {err:#}")),
            }
        }
        (Method::Post, "/api/chat") => {
            // A cross-site page can POST text/plain without a preflight, so
            // insist on JSON and on a same-origin Origin when one is sent.
            let json = header_value(&request, "Content-Type")
                .is_some_and(|c| c.starts_with("application/json"));
            if !json || !same_origin(&request) {
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
        (Method::Post, "/api/config") => {
            let json = header_value(&request, "Content-Type")
                .is_some_and(|c| c.starts_with("application/json"));
            if !json || !same_origin(&request) {
                return respond_text(request, 403, "text/plain", "forbidden");
            }
            let mut body = String::new();
            request.as_reader().take(MAX_BODY).read_to_string(&mut body)?;
            let req: SettingsRequest = match serde_json::from_str(&body) {
                Ok(req) => req,
                Err(err) => return respond_text(request, 400, "text/plain", format!("bad request: {err}")),
            };
            save_settings(app, request, req)
        }
        _ => respond_text(request, 404, "text/plain", "not found"),
    }
}

fn query_value(url: &str, key: &str) -> Option<String> {
    let value = url.split_once('?')?.1.split('&').find_map(|p| {
        let (k, v) = p.split_once('=')?; (k == key).then_some(v)
    })?;
    let mut bytes = Vec::new(); let mut i = 0;
    while i < value.len() {
        let b = value.as_bytes()[i];
        if b == b'%' {
            let hex = value.get(i+1..i+3)?;
            bytes.push(u8::from_str_radix(hex,16).ok()?); i += 3;
        } else { bytes.push(if b == b'+' { b' ' } else { b }); i += 1; }
    }
    String::from_utf8(bytes).ok()
}

fn quota_signature(config: &Config, name: &str) -> String {
    format!("{}:{:?}:{}", config.agents[name].binary, config.sandbox.runtime, config.sandbox.image)
}

/// Snapshot returns immediately; first reads run independently in worker threads.
fn quota_snapshot(app: &Arc<App>) -> Vec<quotas::QuotaStatus> {
    let config = current_config(app);
    let mut names: Vec<_> = config.agents.keys().cloned().collect(); names.sort();
    let mut cold = Vec::new();
    {
        let mut cache = app.quotas.lock().expect("quota cache poisoned");
        cache.retain(|name, _| config.agents.contains_key(name));
        for name in &names {
            let signature = quota_signature(&config, name);
            if cache.get(name).is_none_or(|entry| entry.signature != signature) {
                let agent = CliAgent::new(name, &config.agents[name].binary);
                let status = quotas::QuotaStatus::initial(&agent);
                if status.refreshable { cold.push(name.clone()); }
                cache.insert(name.clone(), QuotaCache { signature, status });
            }
        }
    }
    for name in cold { refresh_quota(app, &name); }
    let cache = app.quotas.lock().expect("quota cache poisoned");
    names.iter().filter_map(|name| cache.get(name).map(|entry| entry.status.clone())).collect()
}

fn refresh_quota(app: &Arc<App>, name: &str) {
    let config = current_config(app);
    let Some(cfg) = config.agents.get(name) else { return };
    let agent = CliAgent::new(name, &cfg.binary);
    let signature = quota_signature(&config, name);
    {
        let mut cache = app.quotas.lock().expect("quota cache poisoned");
        let entry = cache.entry(name.to_string()).or_insert_with(|| QuotaCache {
            signature: signature.clone(), status: quotas::QuotaStatus::initial(&agent),
        });
        if entry.signature != signature { *entry = QuotaCache { signature: signature.clone(), status: quotas::QuotaStatus::initial(&agent) }; }
        if !entry.status.refreshable || entry.status.reading { return; }
        entry.status.reading = true;
        entry.status.attempted_at = Some(chrono::Utc::now().to_rfc3339());
    }
    let (app, name) = (Arc::clone(app), name.to_string());
    thread::spawn(move || {
        let result = build_agent(&app, &name).and_then(|agent| quotas::read(&agent, &app.repo_root));
        let mut cache = app.quotas.lock().expect("quota cache poisoned");
        let Some(entry) = cache.get_mut(&name).filter(|entry| entry.signature == signature) else { return };
        match result {
            Ok(mut status) => { status.attempted_at = entry.status.attempted_at.clone(); entry.status = status; }
            Err(_) => {
                entry.status.state = "read_error".into(); entry.status.reading = false;
                // Keep prior windows and fetched_at. No raw stderr/account secrets in the UI.
                entry.status.error = Some(format!("Couldn't read quota. Check the {name} CLI login and sandbox, then retry."));
            }
        }
    });
}

/// What the agent's own CLI reports it can run: `codex debug models`, `agy models`.
/// `None` means an unknown agent; an empty catalog means nothing could be listed
/// (Claude has no listing command, or the CLI is missing, signed out or slow).
fn agent_models(app: &App, name: &str) -> Option<chat::ModelCatalog> {
    let config = current_config(app);
    let binary = config.agents.get(name)?.binary.clone();
    if let Some(cached) = app.models.lock().expect("models cache poisoned").get(name) {
        return Some(cached.clone());
    }
    let agent = CliAgent::new(name, binary.as_str());
    let listed = if agent.is_codex() {
        run_listing(&binary, &["debug", "models"]).map(|out| chat::parse_codex_models(&out))
    } else if agent.is_agy() {
        run_listing(&binary, &["models"]).map(|out| chat::parse_agy_models(&out))
    } else {
        None
    };
    let catalog = listed.unwrap_or_default();
    if !catalog.models.is_empty() {
        app.models.lock().expect("models cache poisoned").insert(name.to_string(), catalog.clone());
    }
    Some(catalog)
}

/// Runs a listing command and returns its stdout, giving up after `LISTING_TIMEOUT`.
fn run_listing(binary: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(LISTING_TIMEOUT).ok();
    if out.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    out
}

fn save_settings(app: &App, request: Request, req: SettingsRequest) -> Result<()> {
    if !matches!(req.permission_mode.as_str(), "acceptEdits" | "plan") {
        return respond_text(request, 400, "text/plain", "permission_mode must be acceptEdits or plan");
    }
    if req.sandbox_image.trim().is_empty() {
        return respond_text(request, 400, "text/plain", "sandbox_image cannot be empty");
    }

    let mut global = Config::load_global()?;
    let available = current_config(app);
    if !req.default_agent.is_empty() && !available.agents.contains_key(&req.default_agent) {
        return respond_text(request, 400, "text/plain", "default_agent must name a configured agent");
    }
    if req.default_agent != global.chat.default_agent {
        if !req.confirm_default { return respond_text(request, 400, "text/plain", "Confirm the default agent change explicitly"); }
        if !req.default_agent.is_empty() && !diagnostics::snapshot(&app.repo_root, &available).agents.iter().any(|a| a.agent == req.default_agent && a.readiness == "ready") {
            return respond_text(request, 409, "text/plain", "Default agent must be ready; run aic agent test in the terminal");
        }
    }
    global.chat.default_agent = req.default_agent;
    global.permissions.mode = req.permission_mode;
    global.permissions.skip = req.skip_permissions;
    global.sandbox.runtime = req.sandbox;
    global.sandbox.image = req.sandbox_image;
    global.save_global()?;

    let effective = Config::load_merged(&app.repo_root)?;
    *app.config.write().expect("chat config lock poisoned") = effective.clone();
    respond_text(request, 200, "application/json", config_json(&effective).to_string())
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
fn list_skills(root: &Path, config: &Config) -> Vec<Skill> {
    let mut dirs = vec![root.join(".claude").join("skills")];
    if config.sandbox.runtime == SandboxKind::None {
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
    let config = current_config(app);
    let cfg = config.agent(name)?;
    Ok(CliAgent::with_defaults(
        name.to_string(),
        cfg.binary.clone(),
        cfg.model.clone(),
        cfg.effort.clone(),
    )
    .with_sandbox(Sandbox::from_config(&config)?))
}

fn failed(error: String) -> ChatEvent {
    ChatEvent::Done { ok: false, text: String::new(), error: Some(error), usage: None }
}

/// Answers one chat turn as NDJSON: `session`, then `delta`s, then `done`.
/// Blank lines in between are heartbeats.
fn chat_turn(app: &Arc<App>, request: Request, req: ChatRequest) -> Result<()> {
    if req.prompt.trim().is_empty() {
        return respond_text(request, 400, "text/plain", "empty prompt");
    }
    let work_root = match crate::common::workspaces::resolve(&app.repo_root, req.checkout.as_deref()) {
        Ok(root) => root, Err(_) => return respond_text(request, 400, "text/plain", "Selected checkout is not a registered worktree"),
    };
    let config = Config::load_merged(&work_root)?;
    if let Some(expected) = &req.git_context {
        let actual = diagnostics::git_context(&work_root).ok();
        if actual.as_ref().is_none_or(|a| a.checkout != expected.checkout || a.branch != expected.branch || (a.branch.is_none() && a.sha != expected.sha)) {
            return respond_text(request, 409, "text/plain", "Git context changed before execution; review the branch notice and send again. Your draft is preserved.");
        }
    }
    if let Some(session) = &req.session_id {
        let db = app.chat_db.lock().expect("chat database lock poisoned");
        let recorded: Option<String> = db.query_row("SELECT checkout FROM native_session_context WHERE agent=?1 AND session_id=?2", params![req.agent, session], |r|r.get(0)).optional()?;
        if recorded.as_deref().is_some_and(|p| p != work_root.to_string_lossy()) || (recorded.is_none() && work_root != app.repo_root) {
            return respond_text(request, 409, "text/plain", "Native session belongs to another checkout; create a fresh participant session");
        }
    }
    let readiness = diagnostics::for_send(&work_root, &config, std::slice::from_ref(&req.agent));
    if !readiness.agents.iter().any(|a| a.agent == req.agent && a.readiness == "ready") {
        return respond_text(request, 409, "text/plain", "Agent is not ready in this context. Run aic setup or aic agent test in the terminal, then refresh readiness. Your draft is preserved.");
    }
    let agent = match config.agent(&req.agent).and_then(|cfg| Ok(CliAgent::with_defaults(&req.agent, cfg.binary.clone(), cfg.model.clone(), cfg.effort.clone()).with_sandbox(Sandbox::from_config(&config)?))) {
        Ok(agent) => agent,
        Err(err) => return respond_text(request, 400, "text/plain", format!("{err:#}")),
    };
    let sandboxed = config.sandbox.runtime != SandboxKind::None;
    let resume = req.session_id.filter(|s| !s.is_empty());
    let mut native_session = resume.clone();
    let was_resumed = resume.is_some();
    let conversation = req.conversation_id.filter(|s| !s.is_empty());
    let agent_name = req.agent.clone();
    let mut accounted = false;
    let mut started = false;

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
    if agent.is_agy() || agent.is_codex() {
        // No system-prompt flag: fold it into the first turn, which the
        // session then remembers.
        if resume.is_none() {
            prompt = format!("{system}\n\n{prompt}");
        }
        system_prompt = None;
    }

    let opts = RunOpts {
        prompt,
        cwd: Some(work_root.clone()),
        system_prompt,
        model: req.model.filter(|s| !s.trim().is_empty()),
        effort: req.effort.filter(|s| !s.trim().is_empty()),
        // A read-only participant can still read: the sandbox's read-only
        // mount makes skipping prompts safe. Without a sandbox, fall back to
        // plan mode (no edits) and keep prompts on.
        skip_permissions: config.permissions.skip && (req.write || sandboxed),
        permission: if req.write {
            Some(config.permissions.mode.clone())
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
            let _ = diagnostics::record_failure(&work_root, &config, &agent_name, &err.to_string());
                send(&tx, &failed(diagnostics::classify_error(&err.to_string()).1.into()));
        }
        Ok(process) => {
            started = true;
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
            let result = pump_agent(agent.is_agy(), agent.is_codex(), process, &tx, |event| {
                if let ChatEvent::Session { id } = event {
                    native_session = Some(id.clone());
                    let db = app.chat_db.lock().expect("chat database lock poisoned");
                    let _ = db.execute("INSERT OR IGNORE INTO native_session_context (agent, session_id, checkout) VALUES (?1,?2,?3)", params![agent_name, id.as_str(), work_root.to_string_lossy()]);
                }
                if let ChatEvent::Done { ok: false, error, .. } = event {
                    let _ = diagnostics::record_failure(&work_root, &config, &agent_name, error.as_deref().unwrap_or("execution failed"));
                    *error = Some(diagnostics::classify_error(error.as_deref().unwrap_or("execution failed")).1.into());
                }
                if let ChatEvent::Done { usage, .. } = event {
                    if !accounted {
                        if let Some(conversation) = &conversation {
                            let mut db = app.chat_db.lock().expect("chat database lock poisoned");
                            let session = native_session.as_deref().map(|id| consumption::SessionCounter { id, resumed:was_resumed });
                            // agy and Codex report cumulative native-session counters.
                            // Convert resumed results to a per-turn delta before
                            // adding them to the conversation total.
                            let cumulative = agent.is_agy() || agent.is_codex();
                            match consumption::record(&mut db, conversation, &agent_name, session, cumulative, usage.as_ref()) {
                                Ok(normalized) => *usage = normalized,
                                Err(err) => eprintln!("saving conversation usage failed: {err}"),
                            }
                        }
                        accounted = true;
                    }
                }
            });
            finished.store(true, Ordering::Relaxed);
            watcher.thread().unpark();
            let _ = watcher.join();
            if let Err(err) = result {
                let _ = diagnostics::record_failure(&work_root, &config, &agent_name, &err.to_string());
                send(&tx, &failed(diagnostics::classify_error(&err.to_string()).1.into()));
            }
        }
    }
    // A process that never started did not produce a reply or consume usage.
    // Once started, a failure without a final Usage event remains a partial
    // conversation turn and is recorded as such.
    if started && !accounted {
        if let Some(conversation) = &conversation {
            let mut db = app.chat_db.lock().expect("chat database lock poisoned");
            let session = native_session.as_deref().map(|id| consumption::SessionCounter { id, resumed:was_resumed });
            let cumulative = agent.is_agy() || agent.is_codex();
            if let Err(err) = consumption::record(&mut db, conversation, &agent_name, session, cumulative, None) {
                eprintln!("saving conversation usage failed: {err}");
            }
        }
    }
    // Auxiliary calls such as title generation omit conversation_id and
    // should not trigger an account quota read.
    if started && conversation.is_some() { refresh_quota(app, &agent_name); }
    drop(tx);
    responder.join().map_err(|_| anyhow!("response thread panicked"))??;
    Ok(())
}

fn pump_agent(is_agy: bool, is_codex: bool, process: AgentProcess, tx: &Sender<Vec<u8>>, mut observe: impl FnMut(&mut ChatEvent)) -> Result<()> {
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
        let Some(mut event) = chat::parse_line_for(is_agy, is_codex, &line) else { continue };
        observe(&mut event);
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
    fn quota_cache_deduplicates_reads_and_keeps_last_success_on_failure() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("codex");
        let count = dir.path().join("reads");
        let fail = dir.path().join("fail");
        let script = format!(r#"#!/bin/sh
printf 'read\n' >> '{}'
if [ -f '{}' ]; then exit 1; fi
sleep 0.1
while IFS= read -r line; do
 case "$line" in
  *initialize*) printf '{{"id":1,"result":{{}}}}\n' ;;
  *account/read*) printf '{{"id":2,"result":{{"account":{{"type":"chatgpt"}}}}}}\n' ;;
  *account/rateLimits/read*) printf '{{"id":3,"result":{{"rateLimits":{{"primary":{{"usedPercent":86,"windowDurationMins":300}}}}}}}}\n' ;;
 esac
done
"#,count.display(),fail.display());
        std::fs::write(&binary,script).unwrap();
        std::fs::set_permissions(&binary,std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = Config::default(); config.sandbox.runtime = SandboxKind::None;
        config.agents.get_mut("codex").unwrap().binary = binary.to_string_lossy().into_owned();
        for other in ["claude", "agy"] { if let Some(agent) = config.agents.get_mut(other) { agent.binary = "/missing-test/".to_string() + other; } }
        let app = Arc::new(App { repo_root:dir.path().to_path_buf(), config:RwLock::new(config),
            chat_db:Mutex::new(open_chat_db(dir.path()).unwrap()), allowed_hosts:None,
            models:Mutex::default(),quotas:Mutex::default() });
        quota_snapshot(&app);
        refresh_quota(&app,"codex"); // In-flight read must not spawn another process.
        let wait = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            loop {
                let q = quota_snapshot(&app).into_iter().find(|q| q.agent=="codex").unwrap();
                if !q.reading { return q; }
                assert!(std::time::Instant::now()<deadline,"quota reader did not finish");
                thread::sleep(Duration::from_millis(10));
            }
        };
        let first = wait();
        assert_eq!(first.state,"available");
        assert_eq!(first.windows[0].used_pct,Some(86.0));
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(),1);
        // A page reload reads the same in-memory cache without starting a query.
        assert_eq!(quota_snapshot(&app).into_iter().find(|q|q.agent=="codex").unwrap().fetched_at,first.fetched_at);
        std::fs::write(fail,"").unwrap();
        refresh_quota(&app,"codex");
        let failed = wait();
        assert_eq!(failed.state,"read_error");
        assert_eq!(failed.fetched_at,first.fetched_at);
        assert_eq!(failed.windows[0].used_pct,Some(86.0));
        assert!(failed.attempted_at.is_some());
    }

    #[test]
    fn chat_database_is_created_under_aic_and_keeps_json() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_chat_db(dir.path()).unwrap();
        db.execute(
            "INSERT INTO chat_state (id, state_json, drafts_json, updated_at) VALUES (1, ?1, ?2, 0)",
            params![r#"{"convs":[]}"#, r#"{}"#],
        )
        .unwrap();
        let state: String = db
            .query_row("SELECT state_json FROM chat_state WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, r#"{"convs":[]}"#);
        assert!(dir.path().join(".aic/chat/chat.sqlite3").is_file());
    }

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
