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

use crate::agent::{AgentProcess, CliAgent, Killer, RunOpts};
use crate::chat::store::{self as chat, ChatEvent};
use crate::chat::plans;
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

/// How often an idle turn writes a blank line. For durable browser runs this
/// only keeps proxies awake; losing the browser connection no longer owns or
/// stops the agent process.
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
    /// Live process handles exist only to implement an explicit Stop action.
    /// Durable status and output live in SQLite, so a browser refresh can
    /// reconnect without keeping an HTTP request alive.
    active_runs: Mutex<HashMap<String, Killer>>,
}

struct QuotaCache {
    signature: String,
    status: quotas::QuotaStatus,
}

#[derive(Deserialize)]
struct ChatStateRequest {
    state: serde_json::Value,
    drafts: serde_json::Value,
    #[serde(default)]
    revision: Option<u64>,
}

#[derive(Deserialize)]
struct PreflightRequest {
    agents: Vec<String>,
    #[serde(default)]
    checkout: Option<String>,
}

#[derive(Deserialize, Serialize)]
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
    #[serde(default)]
    participant: Option<String>,
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    binding_version: Option<u64>,
    /// Browser-generated id for a reconnectable turn. Auxiliary and older
    /// clients may omit it and retain the legacy connection-owned behavior.
    #[serde(default)]
    run_id: Option<String>,
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
    let mut chat_db = open_chat_db(repo_root)?;
    plans::recover(&mut chat_db, repo_root)?;
    recover_chat_runs(&mut chat_db)?;
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "::1");
    let allowed_hosts = loopback.then(|| {
        vec![format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")]
    });
    let app = Arc::new(App {
        repo_root: repo_root.to_path_buf(), config: RwLock::new(config), chat_db: Mutex::new(chat_db),
        allowed_hosts, models: Mutex::default(), quotas: Mutex::default(), active_runs: Mutex::default(),
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
         );
         CREATE TABLE IF NOT EXISTS chat_runs (
             id TEXT PRIMARY KEY,
             conversation_id TEXT,
             participant TEXT,
             state TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             finished_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS chat_run_events (
             run_id TEXT NOT NULL REFERENCES chat_runs(id) ON DELETE CASCADE,
             seq INTEGER NOT NULL,
             event_json TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             PRIMARY KEY (run_id, seq)
         );
         CREATE INDEX IF NOT EXISTS chat_run_events_run ON chat_run_events(run_id, seq);",
    )?;
    consumption::init(&db)?;
    plans::init(&db)?;
    Ok(db)
}

fn now_millis() -> i64 { chrono::Utc::now().timestamp_millis() }

fn valid_run_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn create_chat_run(app: &App, id: &str, conversation: Option<&str>, participant: Option<&str>) -> Result<()> {
    if !valid_run_id(id) { bail!("invalid run id"); }
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    db.execute(
        "INSERT INTO chat_runs(id,conversation_id,participant,state,created_at) VALUES (?1,?2,?3,'running',?4)",
        params![id, conversation, participant, now_millis()],
    ).context("creating durable chat run")?;
    Ok(())
}

/// Persists first, then returns the exact event sent to the page. A sequence
/// number makes replay idempotent when a live stream and a reconnect overlap.
fn append_chat_run_event(app: &App, id: &str, mut event: serde_json::Value) -> Result<serde_json::Value> {
    let mut db = app.chat_db.lock().expect("chat database lock poisoned");
    let tx = db.transaction()?;
    let state: String = tx.query_row("SELECT state FROM chat_runs WHERE id=?1", [id], |r| r.get(0))?;
    let is_done = event.get("type").and_then(|v| v.as_str()) == Some("done");
    if is_done && state == "stopping" {
        event = serde_json::json!({"type":"done","ok":false,"text":"","error":"Stopped.","stopped":true});
    }
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM chat_run_events WHERE run_id=?1", [id], |r| r.get(0),
    )?;
    if let Some(object) = event.as_object_mut() { object.insert("seq".into(), seq.into()); }
    tx.execute(
        "INSERT INTO chat_run_events(run_id,seq,event_json,created_at) VALUES (?1,?2,?3,?4)",
        params![id, seq, event.to_string(), now_millis()],
    )?;
    if is_done {
        let next = if state == "stopping" { "stopped" }
            else if event.get("ok").and_then(|v| v.as_bool()) == Some(true) { "completed" }
            else { "failed" };
        tx.execute("UPDATE chat_runs SET state=?2,finished_at=?3 WHERE id=?1", params![id, next, now_millis()])?;
    }
    tx.commit()?;
    Ok(event)
}

fn chat_run_snapshot(app: &App, id: &str, after: i64) -> Result<Option<serde_json::Value>> {
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    let run: Option<(String, Option<String>, Option<String>)> = db.query_row(
        "SELECT state,conversation_id,participant FROM chat_runs WHERE id=?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;
    let Some((state, conversation_id, participant)) = run else { return Ok(None) };
    let mut statement = db.prepare(
        "SELECT event_json FROM chat_run_events WHERE run_id=?1 AND seq>?2 ORDER BY seq",
    )?;
    let events = statement.query_map(params![id, after.max(0)], |r| r.get::<_, String>(0))?
        .filter_map(|row| row.ok().and_then(|json| serde_json::from_str(&json).ok()))
        .collect::<Vec<serde_json::Value>>();
    Ok(Some(serde_json::json!({
        "id": id, "state": state, "conversation_id": conversation_id,
        "participant": participant, "events": events,
    })))
}

fn stop_chat_run(app: &App, id: &str) -> Result<Option<String>> {
    let state = {
        let db = app.chat_db.lock().expect("chat database lock poisoned");
        let state: Option<String> = db.query_row("SELECT state FROM chat_runs WHERE id=?1", [id], |r| r.get(0)).optional()?;
        let Some(state) = state else { return Ok(None) };
        if state == "running" {
            db.execute("UPDATE chat_runs SET state='stopping' WHERE id=?1", [id])?;
            "stopping".to_string()
        } else { state }
    };
    if state == "stopping" {
        if let Some(killer) = app.active_runs.lock().expect("active runs lock poisoned").get(id).cloned() { killer.kill(); }
    }
    Ok(Some(state))
}

/// The process itself cannot survive a server restart, but its durable run
/// must still become terminal so a refreshed page does not poll forever.
fn recover_chat_runs(db: &mut Connection) -> Result<()> {
    let ids = {
        let mut statement = db.prepare("SELECT id,state FROM chat_runs WHERE state IN ('running','stopping')")?;
        statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let tx = db.transaction()?;
    for (id, previous) in ids {
        let seq: i64 = tx.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM chat_run_events WHERE run_id=?1", [&id], |r| r.get(0))?;
        let stopped = previous == "stopping";
        let event = serde_json::json!({
            "type":"done", "ok":false, "text":"",
            "error": if stopped { "Stopped." } else { "The chat server restarted before this execution finished." },
            "stopped": stopped, "seq": seq,
        });
        tx.execute("INSERT INTO chat_run_events(run_id,seq,event_json,created_at) VALUES (?1,?2,?3,?4)", params![id,seq,event.to_string(),now_millis()])?;
        tx.execute("UPDATE chat_runs SET state=?2,finished_at=?3 WHERE id=?1", params![id,if stopped {"stopped"} else {"interrupted"},now_millis()])?;
    }
    tx.commit()?;
    Ok(())
}

fn load_chat_state(app: &App) -> Result<serde_json::Value> {
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    let row: Option<(String, String)> = db
        .query_row("SELECT state_json, drafts_json FROM chat_state WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let (_, _, revision) = plans::snapshot(&db)?;
    match row {
        Some((state, drafts)) => {
            let mut state = serde_json::from_str::<serde_json::Value>(&state)?;
            plans::embed(&db, &app.repo_root, &mut state)?;
            Ok(serde_json::json!({"state":state,"drafts":serde_json::from_str::<serde_json::Value>(&drafts)?,"revision":revision}))
        },
        None => Ok(serde_json::json!({ "state": null, "drafts": null, "revision": revision })),
    }
}

fn save_chat_state(app: &App, req: ChatStateRequest) -> Result<u64> {
    let mut db = app.chat_db.lock().expect("chat database lock poisoned");
    plans::save_snapshot(&mut db, &app.repo_root, req.state, req.drafts, req.revision)
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

fn plan_error(request: Request, err: anyhow::Error) -> Result<()> {
    let (status, message) = err.downcast_ref::<plans::ApiError>()
        .map(|e| (e.0, e.1)).unwrap_or((500, "Plan operation failed; preserve your draft and retry"));
    if status == 500 { eprintln!("plan API failed: {err:#}"); }
    respond_text(request, status, "application/json", serde_json::json!({"error":message,"code":status}).to_string())
}

fn handle(app: &Arc<App>, mut request: Request) -> Result<()> {
    if let Some(allowed) = &app.allowed_hosts {
        let ok = header_value(&request, "Host").is_some_and(|h| allowed.contains(&h));
        if !ok {
            return respond_text(request, 403, "text/plain", "forbidden host");
        }
    }

    let path = request.url().split('?').next().unwrap_or("/").to_string();
    if path == "/api/plans" || path.starts_with("/api/plans/") || path.starts_with("/api/conversations/") {
        if !same_origin(&request) || (request.method() != &Method::Get && !header_value(&request, "Content-Type").is_some_and(|c| c.starts_with("application/json"))) {
            return respond_text(request, 403, "text/plain", "forbidden");
        }
        let selected = query_value(request.url(), "checkout");
        let mut body = String::new();
        if request.method() != &Method::Get {
            request.as_reader().take(MAX_BODY + 1).read_to_string(&mut body)?;
            if body.len() as u64 > MAX_BODY { return respond_text(request, 413, "text/plain", "Plan request is too large"); }
        }
        let payload: serde_json::Value = if body.is_empty() { serde_json::json!({}) } else {
            match serde_json::from_str(&body) { Ok(v) => v, Err(_) => return respond_text(request, 400, "text/plain", "Invalid plan request") }
        };
        let result = {
            let mut db = app.chat_db.lock().expect("chat database lock poisoned");
            plans::route(&mut db, &app.repo_root, request.method().as_str(), &path, selected.as_deref(), &payload)
        };
        return match result { Ok(body) => respond_text(request, 200, "application/json", body.to_string()), Err(e) => plan_error(request, e) };
    }
    if let Some(rest) = path.strip_prefix("/api/chat/runs/") {
        let (id, action) = rest.split_once('/').map_or((rest, ""), |(id, action)| (id, action));
        if !valid_run_id(id) { return respond_text(request, 400, "text/plain", "invalid run id"); }
        return match (request.method().clone(), action) {
            (Method::Get, "") => {
                let after = query_value(request.url(), "after").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                match chat_run_snapshot(app, id, after)? {
                    Some(body) => respond_text(request, 200, "application/json", body.to_string()),
                    None => respond_text(request, 404, "text/plain", "run not found"),
                }
            }
            (Method::Post, "stop") => {
                let json = header_value(&request, "Content-Type").is_some_and(|c| c.starts_with("application/json"));
                if !json || !same_origin(&request) { return respond_text(request, 403, "text/plain", "forbidden"); }
                match stop_chat_run(app, id)? {
                    Some(state) => respond_text(request, 202, "application/json", serde_json::json!({"id":id,"state":state}).to_string()),
                    None => respond_text(request, 404, "text/plain", "run not found"),
                }
            }
            _ => respond_text(request, 404, "text/plain", "not found"),
        };
    }
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
                Ok(revision) => respond_text(request, 200, "application/json", serde_json::json!({"revision":revision}).to_string()),
                Err(err) => plan_error(request, err),
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

fn send_json(tx: &Sender<Vec<u8>>, event: &serde_json::Value) -> bool {
    let mut line = serde_json::to_vec(event).expect("chat run event serializes");
    line.push(b'\n');
    tx.send(line).is_ok()
}

/// A durable run succeeds when SQLite accepted the event, even when the
/// browser-side stream has disappeared. Legacy callers without a run id keep
/// the original connection-owned behavior.
fn emit_json(app: &App, run_id: Option<&str>, tx: &Sender<Vec<u8>>, event: serde_json::Value) -> bool {
    if let Some(id) = run_id {
        match append_chat_run_event(app, id, event) {
            Ok(event) => { let _ = send_json(tx, &event); true }
            Err(err) => { eprintln!("saving chat run event failed: {err:#}"); false }
        }
    } else {
        send_json(tx, &event)
    }
}

fn emit_chat_event(app: &App, run_id: Option<&str>, tx: &Sender<Vec<u8>>, event: &ChatEvent) -> bool {
    if run_id.is_none() { return send(tx, event) }
    emit_json(app, run_id, tx, serde_json::to_value(event).expect("ChatEvent serializes"))
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
struct TurnGuard {
    app: Arc<App>,
    turn: plans::Turn,
    success: bool,
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        let mut db = self.app.chat_db.lock().expect("chat database lock poisoned");
        if let Err(e) = plans::finish_turn(&mut db, &self.app.repo_root, &self.turn.token,
            self.turn.execution_id.as_deref(), &self.turn.checkout, self.turn.plan_slug.as_deref(), self.success) {
            eprintln!("recording task execution failed: {e:#}");
        }
    }
}

fn chat_turn(app: &Arc<App>, request: Request, mut req: ChatRequest) -> Result<()> {
    if req.prompt.trim().is_empty() {
        return respond_text(request, 400, "text/plain", "empty prompt");
    }
    let run_id = req.run_id.clone().filter(|id| !id.is_empty());
    if run_id.as_deref().is_some_and(|id| !valid_run_id(id)) {
        return respond_text(request, 400, "text/plain", "invalid run id");
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
    let turn = {
        let mut db = app.chat_db.lock().expect("chat database lock poisoned");
        plans::begin_turn(&mut db, &app.repo_root, &work_root, &serde_json::to_value(&req)?)
    };
    let mut guard = match turn {
        Ok(turn) => TurnGuard { app: Arc::clone(app), turn, success: false },
        Err(e) => return plan_error(request, e),
    };
    if let Some(id) = &run_id {
        if let Err(err) = create_chat_run(app, id, req.conversation_id.as_deref(), req.participant.as_deref()) {
            eprintln!("creating chat run failed: {err:#}");
            return respond_text(request, 409, "text/plain", "This execution id is already in use; retry the message");
        }
    }
    req.write = guard.turn.write;
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
    let system = [Some(access_note), req.system.as_deref().filter(|s| !s.trim().is_empty()),
        (!guard.turn.system.is_empty()).then_some(guard.turn.system.as_str())]
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
        } else if !guard.turn.system.is_empty() {
            // A selected task may change between turns of a native session.
            // Re-send server-owned plan and access framing on every such turn.
            prompt = format!("{access_note}\n\n{}\n\n{prompt}", guard.turn.system);
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
    if let Some(execution_id) = &guard.turn.execution_id {
        emit_json(app, run_id.as_deref(), &tx, serde_json::json!({"type":"execution","id":execution_id}));
    }

    match agent.spawn_stream(&opts) {
        Err(err) => {
            let _ = diagnostics::record_failure(&work_root, &config, &agent_name, &err.to_string());
            emit_chat_event(app, run_id.as_deref(), &tx, &failed(diagnostics::classify_error(&err.to_string()).1.into()));
        }
        Ok(process) => {
            if let Err(err) = plans::attach_process(&app.chat_db.lock().expect("chat database lock poisoned"), &guard.turn.token, &process.killer) {
                process.killer.kill();
                let mut child = process.child;
                let _ = child.wait();
                emit_chat_event(app, run_id.as_deref(), &tx, &failed("Execution could not be recorded. Preserve your draft and retry.".into()));
                drop(tx);
                return Err(err);
            }
            started = true;
            if let Some(id) = &run_id {
                app.active_runs.lock().expect("active runs lock poisoned").insert(id.clone(), process.killer.clone());
            }
            // Heartbeats keep the live response moving through proxies. A
            // durable run ignores a disconnected response and continues; an
            // older client without a run id keeps the prior stop-on-disconnect
            // behavior.
            let finished = Arc::new(AtomicBool::new(false));
            let watcher = {
                let (tx, killer, finished, durable) = (tx.clone(), process.killer.clone(), Arc::clone(&finished), run_id.is_some());
                thread::spawn(move || {
                    while !finished.load(Ordering::Relaxed) {
                        thread::park_timeout(HEARTBEAT);
                        if finished.load(Ordering::Relaxed) {
                            break;
                        }
                        if tx.send(b"\n".to_vec()).is_err() {
                            if !durable { killer.kill(); }
                            break;
                        }
                    }
                })
            };
            let result = pump_agent(agent.is_agy(), agent.is_codex(), process, |event| {
                if let ChatEvent::Session { id } = event {
                    native_session = Some(id.clone());
                    let db = app.chat_db.lock().expect("chat database lock poisoned");
                    let _ = db.execute("INSERT OR IGNORE INTO native_session_context (agent, session_id, checkout) VALUES (?1,?2,?3)", params![agent_name, id.as_str(), work_root.to_string_lossy()]);
                    if let Some(conversation) = &conversation {
                        let _ = db.execute("INSERT OR IGNORE INTO native_session_plan(agent,session_id,conversation_id,plan_slug) VALUES (?1,?2,?3,?4)",params![agent_name,id.as_str(),conversation,guard.turn.plan_slug]);
                    }
                }
                if let ChatEvent::Done { ok: false, error, .. } = event {
                    let _ = diagnostics::record_failure(&work_root, &config, &agent_name, error.as_deref().unwrap_or("execution failed"));
                    *error = Some(diagnostics::classify_error(error.as_deref().unwrap_or("execution failed")).1.into());
                }
                if let ChatEvent::Done { ok, .. } = event { guard.success = *ok; }
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
                emit_chat_event(app, run_id.as_deref(), &tx, event)
            });
            if let Some(id) = &run_id { app.active_runs.lock().expect("active runs lock poisoned").remove(id); }
            finished.store(true, Ordering::Relaxed);
            watcher.thread().unpark();
            let _ = watcher.join();
            if let Err(err) = result {
                guard.success = false;
                let _ = diagnostics::record_failure(&work_root, &config, &agent_name, &err.to_string());
                emit_chat_event(app, run_id.as_deref(), &tx, &failed(diagnostics::classify_error(&err.to_string()).1.into()));
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
    drop(guard);
    drop(tx);
    responder.join().map_err(|_| anyhow!("response thread panicked"))??;
    Ok(())
}

fn pump_agent(is_agy: bool, is_codex: bool, process: AgentProcess, mut emit: impl FnMut(&mut ChatEvent) -> bool) -> Result<()> {
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
        let Some(event) = chat::parse_line_for(is_agy, is_codex, &line) else { continue };
        let event = match event {
            // The agent only reports the reply at the end (no deltas seen):
            // surface it as one chunk so the page has something to render.
            ChatEvent::Done { ok: true, ref text, .. } if !streamed && !text.is_empty() => {
                let mut delta = ChatEvent::Delta { text: text.clone() };
                if !emit(&mut delta) {
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
        let mut event = event;
        if !emit(&mut event) {
            // A durable run only reaches this branch when its event could not
            // be saved. A legacy run reaches it when its browser went away.
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
    if !status.success() { bail!("agent exited unsuccessfully: {status}"); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app(root: &Path) -> App {
        App {
            repo_root: root.to_path_buf(), config: RwLock::new(Config::default()),
            chat_db: Mutex::new(open_chat_db(root).unwrap()), allowed_hosts: None,
            models: Mutex::default(), quotas: Mutex::default(), active_runs: Mutex::default(),
        }
    }

    #[test]
    fn durable_chat_runs_replay_by_sequence_and_stop_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        create_chat_run(&app, "run-1", Some("conversation"), Some("Dev")).unwrap();
        let first = append_chat_run_event(&app, "run-1", serde_json::json!({"type":"delta","text":"one"})).unwrap();
        let second = append_chat_run_event(&app, "run-1", serde_json::json!({"type":"delta","text":"two"})).unwrap();
        assert_eq!(first["seq"], 1); assert_eq!(second["seq"], 2);
        let replay = chat_run_snapshot(&app, "run-1", 1).unwrap().unwrap();
        assert_eq!(replay["state"], "running");
        assert_eq!(replay["events"].as_array().unwrap().len(), 1);
        assert_eq!(replay["events"][0]["text"], "two");

        assert_eq!(stop_chat_run(&app, "run-1").unwrap().as_deref(), Some("stopping"));
        let done = append_chat_run_event(&app, "run-1", serde_json::json!({"type":"done","ok":false,"error":"process exited"})).unwrap();
        assert_eq!(done["stopped"], true);
        assert_eq!(chat_run_snapshot(&app, "run-1", 0).unwrap().unwrap()["state"], "stopped");
    }

    #[test]
    fn server_restart_makes_orphaned_durable_runs_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        create_chat_run(&app, "run-2", Some("conversation"), Some("Dev")).unwrap();
        append_chat_run_event(&app, "run-2", serde_json::json!({"type":"delta","text":"partial"})).unwrap();
        recover_chat_runs(&mut *app.chat_db.lock().unwrap()).unwrap();
        let replay = chat_run_snapshot(&app, "run-2", 1).unwrap().unwrap();
        assert_eq!(replay["state"], "interrupted");
        assert_eq!(replay["events"][0]["type"], "done");
        assert!(replay["events"][0]["error"].as_str().unwrap().contains("restarted"));
    }

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
            models:Mutex::default(),quotas:Mutex::default(),active_runs:Mutex::default() });
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
