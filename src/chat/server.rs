use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use crate::chat::{paths, plans};
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

static WRITE_LOCK_IDS: AtomicU64 = AtomicU64::new(1);

struct App {
    repo_root: PathBuf,
    config: RwLock<Config>,
    /// The browser is a cache; this database is the durable source of chat
    /// state.  A mutex also makes writes deterministic across request threads.
    chat_db: Mutex<Connection>,
    /// Host headers we accept; guards against DNS rebinding when listening
    /// on loopback. `None` when bound elsewhere (the user opted in).
    allowed_hosts: Option<Vec<String>>,
    /// URL of this server as seen by an agent process. Sandboxed chat turns
    /// share the host network so loopback resolves to this same instance.
    api_url: String,
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

#[derive(Deserialize)]
struct DiagnosticActionRequest {
    command: String,
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
    #[serde(default)]
    engineering_loop: Option<EngineeringLoopRequest>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineeringLoopRequest {
    id: String,
    #[serde(default = "default_engineering_loop_mode")]
    mode: String,
    state: String,
    phase: String,
    round: u32,
    turns: u32,
    max_rounds: u32,
    max_turns: u32,
    #[serde(default)]
    implementer: Option<String>,
    #[serde(default)]
    reviewer: Option<String>,
    current: String,
    objective: String,
}

fn default_engineering_loop_mode() -> String { "standard".into() }

#[derive(Deserialize)]
struct EngineeringLoopRoundRequest {
    conversation_id: String,
    decision: String,
}

#[derive(Deserialize)]
struct EngineeringLoopImplementerRequest {
    conversation_id: String,
    participant: String,
}

/// The settings the chat dialog edits (`[chat]`, `[permissions]` and
/// `[sandbox]`). They are saved to the global config; a repository override
/// can still take precedence at runtime.
#[derive(Deserialize)]
struct SettingsRequest {
    default_agent: String,
    #[serde(default)]
    confirm_default: bool,
    #[serde(default)]
    serialize_worktree_writes: Option<bool>,
    #[serde(default)]
    loop_max_rounds: Option<u32>,
    permission_mode: String,
    skip_permissions: bool,
    sandbox: SandboxKind,
    sandbox_image: String,
}

pub fn run(repo_root: &Path, host: &str, port: u16, open: bool) -> Result<()> {
    // Take the repository-wide lock before opening or recovering the chat
    // database. A second instance must not classify the first one's active
    // runs as interrupted, even when it chooses a different TCP port.
    let _server_lock = lock_chat_server(repo_root, host, port)?;
    let server = tiny_http::Server::http((host, port))
        .map_err(|e| anyhow!("binding {host}:{port}: {e}"))?;
    let config = Config::load_merged(repo_root)?;
    let mut chat_db = open_chat_db(repo_root)?;
    plans::recover(&mut chat_db, repo_root)?;
    recover_chat_runs(&mut chat_db)?;
    recover_engineering_loops(&chat_db)?;
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "::1");
    let allowed_hosts = loopback.then(|| {
        vec![format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")]
    });
    let api_host = match host { "0.0.0.0" => "127.0.0.1", "::" => "[::1]", "::1" => "[::1]", value => value };
    let app = Arc::new(App {
        repo_root: repo_root.to_path_buf(), config: RwLock::new(config), chat_db: Mutex::new(chat_db),
        allowed_hosts, api_url: format!("http://{api_host}:{port}"), models: Mutex::default(),
        quotas: Mutex::default(), active_runs: Mutex::default(),
    });

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

/// Kernel advisory lock held for the lifetime of one chat server. `flock`
/// remains effective across the host and container PID namespaces because
/// both processes open the same bind-mounted repository file.
struct ChatServerLock {
    _file: File,
}

#[derive(Clone, Deserialize, Serialize)]
struct WorktreeWriteLockOwner {
    token: String,
    pid: u32,
    #[serde(default)]
    process_pid: Option<u32>,
    agent: String,
    participant: String,
    conversation: Option<String>,
    run_id: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct WorktreeWriteLockState {
    conversation: String,
    owners: Vec<WorktreeWriteLockOwner>,
}

struct WorktreeWriteLock {
    root: PathBuf,
    token: String,
}

impl WorktreeWriteLock {
    fn attach_process(&self, pid: u32) -> Result<()> {
        with_worktree_lock_state(&self.root, |path| {
            let mut state = load_worktree_lock_state(path)?
                .context("worktree write lock disappeared before the agent started")?;
            let owner = state.owners.iter_mut().find(|owner| owner.token == self.token)
                .context("worktree write lock ownership changed before the agent started")?;
            owner.process_pid = Some(pid);
            save_worktree_lock_state(path, &state)
        })
    }
}

impl Drop for WorktreeWriteLock {
    fn drop(&mut self) {
        let result = with_worktree_lock_state(&self.root, |path| {
            let Some(mut state) = load_worktree_lock_state(path)? else { return Ok(()) };
            state.owners.retain(|owner| owner.token != self.token);
            if state.owners.is_empty() {
                if let Err(error) = std::fs::remove_file(path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(error.into());
                }
                Ok(())
            } else {
                save_worktree_lock_state(path, &state)
            }
        });
        if let Err(error) = result {
            eprintln!("releasing worktree write lock failed: {error:#}");
        }
    }
}

enum WorktreeWriteLockAttempt {
    Acquired(WorktreeWriteLock),
    Waiting(Option<WorktreeWriteLockOwner>),
}

fn process_is_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    if pid == std::process::id() {
        return true;
    }
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // Signal zero only checks whether the process exists; it does not send a signal.
    let result = unsafe { kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied
}

fn with_worktree_lock_state<T>(root: &Path, action: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    const LOCK_EX: i32 = 2;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    let dir = root.join(".aic").join("chat");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let guard_path = dir.join("worktree-write.guard");
    let guard = OpenOptions::new().create(true).read(true).write(true).open(&guard_path)
        .with_context(|| format!("opening {}", guard_path.display()))?;
    if unsafe { flock(guard.as_raw_fd(), LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("locking {}", guard_path.display()));
    }
    let result = action(&dir.join("worktree-write.lock"));
    drop(guard);
    result
}

fn load_worktree_lock_state(path: &Path) -> Result<Option<WorktreeWriteLockState>> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(state) => Ok(Some(state)),
            Err(_) => {
                let _ = std::fs::remove_file(path);
                Ok(None)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn save_worktree_lock_state(path: &Path, state: &WorktreeWriteLockState) -> Result<()> {
    let temporary = path.with_file_name(format!("worktree-write.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new().write(true).create(true).truncate(true).open(&temporary)
            .with_context(|| format!("creating {}", temporary.display()))?;
        serde_json::to_writer(&mut file, state)?;
        file.flush()?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("updating {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn try_worktree_write_lock(
    root: &Path,
    owner: &WorktreeWriteLockOwner,
) -> Result<WorktreeWriteLockAttempt> {
    with_worktree_lock_state(root, |path| {
        let mut state = load_worktree_lock_state(path)?.unwrap_or_else(|| WorktreeWriteLockState {
            conversation: owner.conversation.clone().unwrap_or_else(|| owner.token.clone()),
            owners: Vec::new(),
        });
        state.owners.retain(|holder| {
            process_is_alive(holder.pid)
                || holder.process_pid.is_some_and(process_is_alive)
        });
        if state.owners.is_empty() {
            state.conversation = owner.conversation.clone().unwrap_or_else(|| owner.token.clone());
        }
        let conversation = owner.conversation.as_deref().unwrap_or(&owner.token);
        if state.conversation.as_str() != conversation {
            return Ok(WorktreeWriteLockAttempt::Waiting(state.owners.first().cloned()));
        }
        state.owners.push(owner.clone());
        save_worktree_lock_state(path, &state)?;
        Ok(WorktreeWriteLockAttempt::Acquired(WorktreeWriteLock {
            root: root.to_path_buf(),
            token: owner.token.clone(),
        }))
    })
}

fn lock_chat_server(repo_root: &Path, host: &str, port: u16) -> Result<ChatServerLock> {
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe extern "C" { fn flock(fd: i32, operation: i32) -> i32; }

    let dir = repo_root.join(".aic").join("chat");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("server.lock");
    let mut file = OpenOptions::new().create(true).read(true).write(true).open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    // SAFETY: flock accepts an open file descriptor and integer flags only.
    if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            let mut owner = String::new();
            let _ = file.read_to_string(&mut owner);
            let owner = if owner.trim().is_empty() { String::new() } else { format!(" ({})", owner.trim()) };
            bail!("another aic chat instance is already running for this repository{owner}");
        }
        return Err(error).with_context(|| format!("locking {}", path.display()));
    }
    file.set_len(0)?;
    file.rewind()?;
    writeln!(file, "pid {}, http://{}:{}", std::process::id(), host, port)?;
    Ok(ChatServerLock { _file: file })
}

fn open_chat_db(repo_root: &Path) -> Result<Connection> {
    let path = repo_root.join(".aic").join("chat").join("chat.sqlite3");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let db = Connection::open(&path).with_context(|| format!("opening {}", path.display()))?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() { std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?; }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
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
             force_write INTEGER NOT NULL DEFAULT 0,
             waiting_for_write_lock INTEGER NOT NULL DEFAULT 0,
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
         CREATE TABLE IF NOT EXISTS chat_engineering_loops (
             id TEXT PRIMARY KEY,
             conversation_id TEXT NOT NULL,
             checkout TEXT NOT NULL,
             state TEXT NOT NULL,
             phase TEXT NOT NULL,
             round INTEGER NOT NULL,
             turns INTEGER NOT NULL,
             max_rounds INTEGER NOT NULL,
             max_turns INTEGER NOT NULL,
             mode TEXT NOT NULL DEFAULT 'standard',
             implementer TEXT NOT NULL,
             reviewer TEXT,
             current_participant TEXT NOT NULL,
             current_run_id TEXT,
             objective TEXT NOT NULL,
             reason TEXT,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
             finished_at INTEGER
         );
         CREATE INDEX IF NOT EXISTS chat_run_events_run ON chat_run_events(run_id, seq);",
    )?;
    // Older databases predate the per-run lock override. SQLite has no
    // `ADD COLUMN IF NOT EXISTS`, so a duplicate-column error is harmless.
    let _ = db.execute(
        "ALTER TABLE chat_runs ADD COLUMN force_write INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = db.execute(
        "ALTER TABLE chat_engineering_loops ADD COLUMN mode TEXT NOT NULL DEFAULT 'standard'",
        [],
    );
    let _ = db.execute(
        "ALTER TABLE chat_runs ADD COLUMN waiting_for_write_lock INTEGER NOT NULL DEFAULT 0",
        [],
    );
    consumption::init(&db)?;
    paths::init(&db)?;
    plans::init(&db)?;
    crate::share::local::init(&db)?;
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
        tx.execute(
            "UPDATE chat_runs SET state=?2,waiting_for_write_lock=0,finished_at=?3 WHERE id=?1",
            params![id, next, now_millis()],
        )?;
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

#[derive(Debug, PartialEq, Eq)]
enum ForceChatRun {
    Forced,
    NotWaiting,
    NotRunning(String),
    Missing,
}

fn force_chat_run(app: &App, id: &str) -> Result<ForceChatRun> {
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    let changed = db.execute(
        "UPDATE chat_runs SET force_write=1
         WHERE id=?1 AND state='running' AND waiting_for_write_lock=1",
        [id],
    )?;
    if changed == 1 {
        return Ok(ForceChatRun::Forced);
    }
    let run: Option<(String, bool)> = db
        .query_row(
            "SELECT state,waiting_for_write_lock FROM chat_runs WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(match run {
        None => ForceChatRun::Missing,
        Some((state, _)) if state != "running" => ForceChatRun::NotRunning(state),
        Some(_) => ForceChatRun::NotWaiting,
    })
}

fn set_chat_run_waiting_for_write_lock(app: &App, id: Option<&str>, waiting: bool) -> Result<()> {
    if let Some(id) = id {
        let db = app.chat_db.lock().expect("chat database lock poisoned");
        db.execute(
            "UPDATE chat_runs SET waiting_for_write_lock=?2 WHERE id=?1 AND state='running'",
            params![id, waiting],
        )?;
    }
    Ok(())
}

fn chat_run_is_forced(app: &App, id: Option<&str>) -> bool {
    let Some(id) = id else { return false };
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    db.query_row("SELECT force_write FROM chat_runs WHERE id=?1", [id], |row| row.get::<_, bool>(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or(false)
}

fn chat_run_was_stopped(app: &App, id: Option<&str>) -> bool {
    let Some(id) = id else { return false };
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    db.query_row("SELECT state FROM chat_runs WHERE id=?1", [id], |row| row.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .is_some_and(|state| matches!(state.as_str(), "stopping" | "stopped"))
}

#[derive(Debug)]
struct EngineeringLoopRecord {
    conversation_id: String,
    checkout: String,
    state: String,
    phase: String,
    round: u32,
    turns: u32,
    max_rounds: u32,
    max_turns: u32,
    mode: String,
    implementer: Option<String>,
    reviewer: Option<String>,
    current_participant: String,
    current_run_id: Option<String>,
    objective: String,
}

fn engineering_loop_record(db: &Connection, id: &str) -> Result<Option<EngineeringLoopRecord>> {
    db.query_row(
        "SELECT conversation_id,checkout,state,phase,round,turns,max_rounds,max_turns,
                mode,implementer,reviewer,current_participant,current_run_id,objective
         FROM chat_engineering_loops WHERE id=?1",
        [id],
        |row| Ok(EngineeringLoopRecord {
            conversation_id: row.get(0)?, checkout: row.get(1)?, state: row.get(2)?,
            phase: row.get(3)?, round: row.get(4)?, turns: row.get(5)?,
            max_rounds: row.get(6)?, max_turns: row.get(7)?, mode: row.get(8)?,
            implementer: Some(row.get::<_,String>(9)?), reviewer: row.get(10)?,
            current_participant: row.get(11)?, current_run_id: row.get(12)?, objective: row.get(13)?,
        }),
    ).optional().map(|record| record.map(|mut record| {
        if record.implementer.as_deref() == Some("") { record.implementer = None; }
        record
    })).map_err(Into::into)
}

fn conversation_participant(db: &Connection, conversation: &str, name: &str) -> Result<Option<(bool, bool)>> {
    let state: Option<String> = db.query_row("SELECT state_json FROM chat_state WHERE id=1", [], |row| row.get(0)).optional()?;
    let Some(state) = state else { return Ok(None) };
    let value: serde_json::Value = serde_json::from_str(&state)?;
    Ok(value["convs"].as_array().and_then(|conversations| {
        conversations.iter().find(|item| item["id"].as_str() == Some(conversation))
    }).and_then(|conversation| conversation["participants"].as_array()).and_then(|participants| {
        participants.iter().find(|participant| participant["name"].as_str() == Some(name))
    }).map(|participant| (
        !participant["inactive"].as_bool().unwrap_or(false),
        participant["write"].as_bool().unwrap_or(false),
    )))
}

fn begin_engineering_loop_turn(
    app: &App,
    request: &EngineeringLoopRequest,
    conversation: &str,
    participant: &str,
    run_id: &str,
    checkout: &Path,
    max_rounds: u32,
) -> Result<()> {
    if !valid_run_id(&request.id) { bail!("invalid Loop Engineering id") }
    if !matches!(request.mode.as_str(), "standard" | "review_first") {
        bail!("invalid Loop Engineering mode")
    }
    if request.state != "running" { bail!("Loop Engineering is not running") }
    if !matches!(request.phase.as_str(), "implementing" | "reviewing") {
        bail!("invalid Loop Engineering phase")
    }
    if request.objective.trim().is_empty() { bail!("Loop Engineering objective is empty") }
    if request.max_rounds == 0 || request.max_turns != request.max_rounds.saturating_mul(2)
        || request.round == 0 || request.round > request.max_rounds
        || request.turns >= request.max_turns
    {
        bail!("Loop Engineering limits are invalid")
    }
    if request.current != participant { bail!("participant is not the current Loop Engineering actor") }
    let reviewer = request.reviewer.as_deref().context("Loop Engineering reviewer is required")?;
    if request.implementer.as_deref() == Some(reviewer) {
        bail!("implementer and reviewer must be different")
    }

    let db = app.chat_db.lock().expect("chat database lock poisoned");
    let Some((active, writable)) = conversation_participant(&db, conversation, participant)? else {
        bail!("Loop Engineering participant is not in this conversation")
    };
    if !active { bail!("Loop Engineering participant is inactive") }
    if request.phase == "implementing" && !writable {
        bail!("Loop Engineering implementer must have write permission")
    }
    if request.phase == "implementing" && request.implementer.as_deref() != Some(participant) {
        bail!("Loop Engineering implementer is not the current participant")
    }
    let Some((active, _)) = conversation_participant(&db, conversation, reviewer)? else {
        bail!("Loop Engineering reviewer is not in this conversation")
    };
    if !active { bail!("Loop Engineering reviewer is inactive") }
    if let Some(implementer) = request.implementer.as_deref() {
        let Some((active, writable)) = conversation_participant(&db, conversation, implementer)? else {
            bail!("Loop Engineering implementer is not in this conversation")
        };
        if !active || !writable { bail!("Loop Engineering implementer must be active with write permission") }
    }

    match engineering_loop_record(&db, &request.id)? {
        None => {
            let valid_start = request.round == 1 && request.turns == 0 && match request.mode.as_str() {
                "standard" => request.phase == "implementing"
                    && request.implementer.as_deref() == Some(request.current.as_str()),
                "review_first" => request.phase == "reviewing" && request.current == reviewer,
                _ => false,
            };
            if request.max_rounds > 20 || request.max_rounds > max_rounds || !valid_start {
                bail!("invalid initial Loop Engineering state")
            }
            db.execute(
                "INSERT INTO chat_engineering_loops(
                    id,conversation_id,checkout,state,phase,round,turns,max_rounds,max_turns,
                    mode,implementer,reviewer,current_participant,current_run_id,objective,created_at,updated_at
                 ) VALUES (?1,?2,?3,'running',?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15)",
                params![request.id,conversation,checkout.to_string_lossy(),request.phase,request.round,
                    request.turns,request.max_rounds,request.max_turns,request.mode,
                    request.implementer.as_deref().unwrap_or(""),request.reviewer,request.current,
                    run_id,request.objective,now_millis()],
            )?;
        }
        Some(record) => {
            if record.conversation_id != conversation || record.state != "running"
                || record.checkout.as_str() != checkout.to_string_lossy().as_ref()
                || record.objective.as_str() != request.objective.as_str()
                || record.current_run_id.is_some() || record.phase != request.phase
                || record.round != request.round || record.turns != request.turns
                || record.max_rounds != request.max_rounds || record.max_turns != request.max_turns
                || record.mode != request.mode
                || record.implementer != request.implementer || record.reviewer != request.reviewer
                || record.current_participant != request.current
            {
                bail!("Loop Engineering state changed; refresh before continuing")
            }
            db.execute(
                "UPDATE chat_engineering_loops SET current_run_id=?2,updated_at=?3 WHERE id=?1",
                params![request.id,run_id,now_millis()],
            )?;
        }
    }
    Ok(())
}

fn normalize_engineering_loop_verdict(raw: &str) -> Option<&'static str> {
    let normalized = raw.trim().to_lowercase().replace('-', "_").replace(' ', "_");
    match normalized.as_str() {
        "approved" | "aprovado" | "aprovada" => Some("approved"),
        "changes_requested"
        | "change_requested"
        | "request_changes"
        | "alterações_solicitadas"
        | "alteracoes_solicitadas"
        | "mudanças_solicitadas"
        | "mudancas_solicitadas" => Some("changes_requested"),
        _ => None,
    }
}

fn normalized_engineering_loop_verdict(value: &serde_json::Value) -> Option<&'static str> {
    let object = value.as_object()?;
    let raw = object.iter().find_map(|(key, value)| {
        if key.eq_ignore_ascii_case("verdict") { value.as_str() } else { None }
    })?;
    normalize_engineering_loop_verdict(raw)
}

fn merge_engineering_loop_verdict(
    found: &mut Option<&'static str>,
    candidate: &'static str,
) -> bool {
    if found.is_some_and(|current| current != candidate) {
        return false
    }
    *found = Some(candidate);
    true
}

fn json_engineering_loop_verdict(text: &str) -> Option<&'static str> {
    let mut found = None;
    for (start, ch) in text.char_indices() {
        if ch != '{' {
            continue
        }
        let mut values = serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<serde_json::Value>();
        let Some(Ok(value)) = values.next() else {
            continue
        };
        let Some(candidate) = normalized_engineering_loop_verdict(&value) else {
            continue
        };
        if !merge_engineering_loop_verdict(&mut found, candidate) {
            return None
        }
    }
    found
}

fn engineering_loop_directive(text: &str) -> Option<&'static str> {
    // Prefer the documented marker, but tolerate capitalization and spacing
    // differences produced by different agents.
    let mut rest = text;
    let mut tagged = None;
    while let Some(start) = rest.find("<!--") {
        rest = &rest[start + 4..];
        let Some(end) = rest.find("-->") else {
            break
        };
        let body = rest[..end].trim();
        if body.get(..8).is_some_and(|prefix| prefix.eq_ignore_ascii_case("aic-loop")) {
            if let Some(candidate) = json_engineering_loop_verdict(&body[8..])
                && !merge_engineering_loop_verdict(&mut tagged, candidate)
            {
                return None;
            }
        }
        rest = &rest[end + 3..];
    }
    if tagged.is_some() {
        return tagged
    }

    // Some CLIs preserve the JSON but wrap it in a Markdown code block or
    // omit the HTML comment. It is still an explicit, machine-readable
    // verdict, so do not pause an otherwise valid review for presentation.
    if let Some(verdict) = json_engineering_loop_verdict(text) {
        return Some(verdict)
    }

    // A final `Verdict: approved` line is another common structured form.
    let mut labelled = None;
    for line in text.lines() {
        let line = line
            .trim()
            .trim_matches(|ch: char| matches!(ch, '*' | '_' | '`' | '-' | ' '));
        let Some(separator) = line.find(':').or_else(|| line.find('=')) else {
            continue
        };
        let (key, raw) = line.split_at(separator);
        let raw = &raw[1..];
        if !key.trim().trim_matches(|ch: char| matches!(ch, '*' | '_' | '`'))
            .eq_ignore_ascii_case("verdict")
        {
            continue
        }
        let raw = raw
            .trim()
            .trim_matches(|ch: char| matches!(ch, '*' | '_' | '`' | '.' | ' '));
        let Some(candidate) = normalize_engineering_loop_verdict(raw) else {
            continue
        };
        if !merge_engineering_loop_verdict(&mut labelled, candidate) {
            return None
        }
    }
    if labelled.is_some() {
        return labelled
    }

    // Finally accept an explicit standalone decision. This covers reviewers
    // that obey the semantic instruction but drop all protocol decoration.
    let mut standalone = None;
    for line in text.lines() {
        let raw = line.trim().trim_matches(|ch: char| {
            matches!(ch, '*' | '_' | '`' | '#' | '-' | ':' | '.' | ' ')
        });
        let Some(candidate) = normalize_engineering_loop_verdict(raw) else {
            continue
        };
        if !merge_engineering_loop_verdict(&mut standalone, candidate) {
            return None
        }
    }
    standalone
}

fn engineering_loop_system(request: &EngineeringLoopRequest) -> String {
    let header = format!(
        "Backend-enforced Loop Engineering {}, round {}/{}. Objective: {}",
        request.id, request.round, request.max_rounds, request.objective
    );
    if request.phase == "reviewing" {
        let routing = request.implementer.as_deref().map_or_else(
            || "If changes are requested, the loop waits for the user to select a writable implementer.".to_string(),
            |implementer| format!("The backend always returns requested changes to {implementer}."),
        );
        return format!(
            "{header}\nYou are the reviewer. Review the implementer's work and end your response with exactly one directive: \
             <!-- aic-loop {{\"verdict\":\"approved\"}} --> or \
             <!-- aic-loop {{\"verdict\":\"changes_requested\"}} -->. \
             {routing}"
        );
    }
    let reviewer = request.reviewer.as_deref().unwrap_or("the selected reviewer");
    format!(
        "{header}\nYou are the implementer. Complete the work. Review by {reviewer} is mandatory \
         and starts automatically after this turn; no handoff directive is required."
    )
}

fn finish_engineering_loop_turn(app: &App, loop_id: &str, run_id: &str, output: &str, ok: bool) -> Result<()> {
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    let Some(record) = engineering_loop_record(&db, loop_id)? else { return Ok(()) };
    if record.state != "running" || record.current_run_id.as_deref() != Some(run_id) { return Ok(()) }
    let turns = record.turns + 1;
    let finish = |state: &str, reason: &str| -> Result<()> {
        db.execute(
            "UPDATE chat_engineering_loops SET state=?2,turns=?3,current_run_id=NULL,reason=?4,
                    updated_at=?5,finished_at=?5 WHERE id=?1 AND state='running' AND current_run_id=?6",
            params![loop_id,state,turns,reason,now_millis(),run_id],
        )?;
        Ok(())
    };
    if !ok { return finish("paused", "The current participant failed") }
    let verdict = engineering_loop_directive(output);
    if record.phase == "implementing" {
        if turns >= record.max_turns { return finish("limit_reached", "Turn limit reached") }
        let Some(target) = record.reviewer.as_deref() else {
            return finish("paused", "Loop Engineering has no reviewer")
        };
        if !conversation_participant(&db, &record.conversation_id, target)?.is_some_and(|(active, _)| active) {
            return finish("paused", "Requested reviewer is unavailable")
        }
        db.execute(
            "UPDATE chat_engineering_loops SET phase='reviewing',turns=?2,reviewer=?3,
                    current_participant=?3,current_run_id=NULL,updated_at=?4
             WHERE id=?1 AND state='running' AND current_run_id=?5",
            params![loop_id,turns,target,now_millis(),run_id],
        )?;
        return Ok(())
    }
    match verdict {
        Some("approved") => finish("approved", "Approved by reviewer"),
        Some("changes_requested") => {
            if record.implementer.is_none() {
                db.execute(
                    "UPDATE chat_engineering_loops
                     SET state='awaiting_implementer',turns=?2,current_run_id=NULL,
                         reason='Select a writable implementer to address the requested changes',
                         updated_at=?3,finished_at=NULL
                     WHERE id=?1 AND state='running' AND current_run_id=?4",
                    params![loop_id,turns,now_millis(),run_id],
                )?;
                return Ok(())
            }
            if record.round >= record.max_rounds || turns >= record.max_turns {
                db.execute(
                    "UPDATE chat_engineering_loops
                     SET state='awaiting_round_approval',turns=?2,current_run_id=NULL,
                         reason='The configured round limit was reached without approval. Continue for one more round?',
                         updated_at=?3,finished_at=NULL
                     WHERE id=?1 AND state='running' AND current_run_id=?4",
                    params![loop_id,turns,now_millis(),run_id],
                )?;
                return Ok(())
            }
            db.execute(
                "UPDATE chat_engineering_loops SET phase='implementing',round=round+1,turns=?2,
                        current_participant=implementer,current_run_id=NULL,updated_at=?3
                 WHERE id=?1 AND state='running' AND current_run_id=?4",
                params![loop_id,turns,now_millis(),run_id],
            )?;
            Ok(())
        }
        _ => finish("paused", "Reviewer did not provide a structured verdict"),
    }
}

enum EngineeringLoopImplementerDecision {
    Missing,
    Conflict(String),
    Applied(serde_json::Value),
}

fn select_engineering_loop_implementer(
    app: &App,
    id: &str,
    conversation_id: &str,
    participant: &str,
) -> Result<EngineeringLoopImplementerDecision> {
    if participant.trim().is_empty() {
        return Ok(EngineeringLoopImplementerDecision::Conflict("implementer is required".into()))
    }
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    let Some(record) = engineering_loop_record(&db, id)? else {
        return Ok(EngineeringLoopImplementerDecision::Missing)
    };
    if record.conversation_id != conversation_id {
        return Ok(EngineeringLoopImplementerDecision::Conflict("Loop Engineering belongs to another conversation".into()))
    }
    if record.state != "awaiting_implementer" || record.current_run_id.is_some() {
        return Ok(EngineeringLoopImplementerDecision::Conflict("Loop Engineering is not waiting for an implementer".into()))
    }
    if record.reviewer.as_deref() == Some(participant) {
        return Ok(EngineeringLoopImplementerDecision::Conflict("implementer and reviewer must be different".into()))
    }
    if !conversation_participant(&db, conversation_id, participant)?
        .is_some_and(|(active, writable)| active && writable)
    {
        return Ok(EngineeringLoopImplementerDecision::Conflict(
            "Loop Engineering implementer must be active with write permission".into(),
        ))
    }
    let now = now_millis();
    if record.round >= record.max_rounds || record.turns >= record.max_turns {
        db.execute(
            "UPDATE chat_engineering_loops
             SET implementer=?2,state='awaiting_round_approval',current_participant=?2,
                 reason='The configured round limit was reached without approval. Continue for one more round?',
                 updated_at=?3,finished_at=NULL
             WHERE id=?1 AND conversation_id=?4 AND state='awaiting_implementer' AND current_run_id IS NULL",
            params![id,participant,now,conversation_id],
        )?;
        return Ok(EngineeringLoopImplementerDecision::Applied(serde_json::json!({
            "id":id, "state":"awaiting_round_approval", "phase":"reviewing",
            "round":record.round, "turns":record.turns, "implementer":participant,
            "reviewer":record.reviewer, "current":participant,
            "reason":"The configured round limit was reached without approval. Continue for one more round?",
        })))
    }
    let Some(round) = record.round.checked_add(1) else {
        return Ok(EngineeringLoopImplementerDecision::Conflict("round limit cannot be increased further".into()))
    };
    db.execute(
        "UPDATE chat_engineering_loops
         SET implementer=?2,state='running',phase='implementing',round=?3,
             current_participant=?2,reason=NULL,updated_at=?4,finished_at=NULL
         WHERE id=?1 AND conversation_id=?5 AND state='awaiting_implementer' AND current_run_id IS NULL",
        params![id,participant,round,now,conversation_id],
    )?;
    Ok(EngineeringLoopImplementerDecision::Applied(serde_json::json!({
        "id":id, "state":"running", "phase":"implementing", "round":round,
        "turns":record.turns, "max_rounds":record.max_rounds, "max_turns":record.max_turns,
        "mode":record.mode, "implementer":participant, "reviewer":record.reviewer,
        "current":participant,
    })))
}

enum EngineeringLoopRoundDecision {
    Missing,
    Conflict(String),
    Applied(serde_json::Value),
}

fn decide_engineering_loop_round(
    app: &App,
    id: &str,
    conversation_id: &str,
    decision: &str,
) -> Result<EngineeringLoopRoundDecision> {
    if !matches!(decision, "approve" | "decline") {
        return Ok(EngineeringLoopRoundDecision::Conflict("invalid round decision".into()))
    }
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    let Some(record) = engineering_loop_record(&db, id)? else {
        return Ok(EngineeringLoopRoundDecision::Missing)
    };
    if record.conversation_id != conversation_id {
        return Ok(EngineeringLoopRoundDecision::Conflict("Loop Engineering belongs to another conversation".into()))
    }
    if record.state != "awaiting_round_approval" || record.current_run_id.is_some() {
        return Ok(EngineeringLoopRoundDecision::Conflict("Loop Engineering is not waiting for a round decision".into()))
    }
    let now = now_millis();
    if decision == "decline" {
        db.execute(
            "UPDATE chat_engineering_loops
             SET state='declined',reason='Another round was declined by the user',updated_at=?2,finished_at=?2
             WHERE id=?1 AND conversation_id=?3 AND state='awaiting_round_approval' AND current_run_id IS NULL",
            params![id,now,conversation_id],
        )?;
        return Ok(EngineeringLoopRoundDecision::Applied(serde_json::json!({
            "id":id, "state":"declined", "round":record.round,
            "max_rounds":record.max_rounds, "max_turns":record.max_turns,
            "reason":"Another round was declined by the user",
        })))
    }
    let Some(implementer) = record.implementer.as_deref() else {
        return Ok(EngineeringLoopRoundDecision::Conflict("Select an implementer before approving another round".into()))
    };
    if !conversation_participant(&db, conversation_id, implementer)?
        .is_some_and(|(active, writable)| active && writable)
    {
        return Ok(EngineeringLoopRoundDecision::Conflict(
            "Loop Engineering implementer is unavailable or no longer has write permission".into(),
        ))
    }
    let Some(round) = record.round.checked_add(1) else {
        return Ok(EngineeringLoopRoundDecision::Conflict("round limit cannot be increased further".into()))
    };
    let Some(max_rounds) = record.max_rounds.checked_add(1) else {
        return Ok(EngineeringLoopRoundDecision::Conflict("round limit cannot be increased further".into()))
    };
    let Some(max_turns) = record.max_turns.checked_add(2) else {
        return Ok(EngineeringLoopRoundDecision::Conflict("turn limit cannot be increased further".into()))
    };
    db.execute(
        "UPDATE chat_engineering_loops
         SET state='running',phase='implementing',round=?2,max_rounds=?3,max_turns=?4,
             current_participant=implementer,reason=NULL,updated_at=?5,finished_at=NULL
         WHERE id=?1 AND conversation_id=?6 AND state='awaiting_round_approval' AND current_run_id IS NULL",
        params![id,round,max_rounds,max_turns,now,conversation_id],
    )?;
    Ok(EngineeringLoopRoundDecision::Applied(serde_json::json!({
        "id":id, "state":"running", "phase":"implementing", "round":round,
        "max_rounds":max_rounds, "max_turns":max_turns,
        "current":implementer,
    })))
}

fn stop_engineering_loop(app: &App, id: &str) -> Result<Option<(String, Option<String>)>> {
    let run = {
        let db = app.chat_db.lock().expect("chat database lock poisoned");
        let Some(record) = engineering_loop_record(&db, id)? else { return Ok(None) };
        if matches!(record.state.as_str(), "running" | "paused" | "awaiting_round_approval" | "awaiting_implementer") {
            db.execute(
                "UPDATE chat_engineering_loops SET state='stopped',reason='Stopped by user',updated_at=?2,finished_at=?2 WHERE id=?1",
                params![id,now_millis()],
            )?;
            ("stopped".to_string(), record.current_run_id)
        } else { (record.state, record.current_run_id) }
    };
    if let Some(run_id) = run.1.as_deref() { let _ = stop_chat_run(app, run_id)?; }
    Ok(Some(run))
}

fn engineering_loop_snapshot(app: &App, id: &str) -> Result<Option<serde_json::Value>> {
    let db = app.chat_db.lock().expect("chat database lock poisoned");
    db.query_row(
        "SELECT conversation_id,checkout,state,phase,round,turns,max_rounds,max_turns,
                mode,implementer,reviewer,current_participant,current_run_id,objective,reason,
                created_at,updated_at,finished_at
         FROM chat_engineering_loops WHERE id=?1",
        [id],
        |row| Ok(serde_json::json!({
            "id":id, "conversation_id":row.get::<_,String>(0)?, "checkout":row.get::<_,String>(1)?,
            "state":row.get::<_,String>(2)?, "phase":row.get::<_,String>(3)?,
            "round":row.get::<_,u32>(4)?, "turns":row.get::<_,u32>(5)?,
            "max_rounds":row.get::<_,u32>(6)?, "max_turns":row.get::<_,u32>(7)?,
            "mode":row.get::<_,String>(8)?,
            "implementer":match row.get::<_,String>(9)?.as_str() { "" => None, value => Some(value.to_string()) },
            "reviewer":row.get::<_,Option<String>>(10)?,
            "current":row.get::<_,String>(11)?, "current_run_id":row.get::<_,Option<String>>(12)?,
            "objective":row.get::<_,String>(13)?, "reason":row.get::<_,Option<String>>(14)?,
            "created_at":row.get::<_,i64>(15)?, "updated_at":row.get::<_,i64>(16)?,
            "finished_at":row.get::<_,Option<i64>>(17)?,
        })),
    ).optional().map_err(Into::into)
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
        tx.execute(
            "UPDATE chat_runs SET state=?2,waiting_for_write_lock=0,finished_at=?3 WHERE id=?1",
            params![id,if stopped {"stopped"} else {"interrupted"},now_millis()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn recover_engineering_loops(db: &Connection) -> Result<()> {
    db.execute(
        "UPDATE chat_engineering_loops
         SET state='paused',current_run_id=NULL,
             reason='The chat server restarted during the current loop turn',updated_at=?1
         WHERE state='running' AND current_run_id IS NOT NULL
           AND EXISTS (
               SELECT 1 FROM chat_runs
               WHERE chat_runs.id=chat_engineering_loops.current_run_id
                 AND chat_runs.state IN ('completed','failed','stopped','interrupted')
           )",
        [now_millis()],
    )?;
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
    let public_source = req.state.clone();
    let revision = plans::save_snapshot(&mut db, &app.repo_root, req.state, req.drafts, req.revision)?;
    drop(db);
    // Sharing is best-effort and asynchronous: a relay outage must never
    // prevent or delay local chat persistence.
    crate::share::local::queue_sync(&app.repo_root,public_source);
    Ok(revision)
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
        "serialize_worktree_writes": config.chat.serialize_worktree_writes,
        "loop_max_rounds": config.chat.loop_max_rounds,
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
    if path == "/api/shares" || path.starts_with("/api/shares/") {
        let json_request = header_value(&request, "Content-Type").is_some_and(|value| value.starts_with("application/json"));
        if !same_origin(&request)
            || (request.method() != &Method::Get && request.method() != &Method::Delete && !json_request)
        {
            return respond_text(request, 403, "text/plain", "forbidden");
        }
        let mut body = String::new();
        if request.method() != &Method::Get && request.method() != &Method::Delete {
            request.as_reader().take(MAX_BODY + 1).read_to_string(&mut body)?;
            if body.len() as u64 > MAX_BODY { return respond_text(request, 413, "text/plain", "Share request is too large"); }
        }
        let result = {
            let db = app.chat_db.lock().expect("chat database lock poisoned");
            if path == "/api/shares" && request.method() == &Method::Get {
                crate::share::local::list(&db)
            } else if path == "/api/shares/config" && request.method() == &Method::Post {
                serde_json::from_str::<crate::share::local::ConfigureRequest>(&body)
                    .map_err(anyhow::Error::from).and_then(|payload| crate::share::local::configure(&db,payload))
            } else if path == "/api/shares" && request.method() == &Method::Post {
                serde_json::from_str::<crate::share::local::ShareRequest>(&body)
                    .map_err(anyhow::Error::from).and_then(|payload| crate::share::local::create(&db,payload))
            } else if let Some(rest) = path.strip_prefix("/api/shares/") {
                let (id,action)=rest.split_once('/').map_or((rest,""),|(id,action)|(id,action));
                if id.is_empty() || id.contains('/') {
                    Err(anyhow!("Invalid share id"))
                } else if action.is_empty() && request.method() == &Method::Put {
                    serde_json::from_str::<crate::share::local::ShareRequest>(&body)
                        .map_err(anyhow::Error::from).and_then(|payload| crate::share::local::update(&db,id,payload))
                } else if action == "rotate-password" && request.method() == &Method::Post {
                    crate::share::local::rotate_password(&db,id)
                } else if action.is_empty() && request.method() == &Method::Delete {
                    crate::share::local::revoke(&db,id).map(|_|serde_json::json!({"revoked":true}))
                } else { Err(anyhow!("Unknown share operation")) }
            } else { Err(anyhow!("Unknown share operation")) }
        };
        return match result {
            Ok(value)=>respond_text(request,200,"application/json",value.to_string()),
            Err(error)=>respond_text(request,400,"application/json",serde_json::json!({"error":error.to_string()}).to_string()),
        };
    }
    if let Some(id) = path
        .strip_prefix("/api/conversations/")
        .and_then(|rest| rest.strip_suffix("/paths"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
    {
        let sandbox_runtime = current_config(app).sandbox.runtime;
        // Host path policy is global-only: a repository override must never
        // be able to widen its own view of the host filesystem.
        let path_roots = Config::load_global()?.sandbox.path_roots;
        let json = header_value(&request, "Content-Type")
            .is_some_and(|value| value.starts_with("application/json"));
        if !same_origin(&request) || (request.method() != &Method::Get && !json) {
            return respond_text(request, 403, "text/plain", "forbidden");
        }
        if request.method() != &Method::Get && sandbox_runtime == SandboxKind::None {
            return respond_text(
                request,
                409,
                "application/json",
                serde_json::json!({
                    "error":"Conversation paths require the Docker or Podman sandbox",
                    "code":409
                })
                .to_string(),
            );
        }
        let mut body = String::new();
        if request.method() != &Method::Get {
            request.as_reader().take(MAX_BODY + 1).read_to_string(&mut body)?;
            if body.len() as u64 > MAX_BODY {
                return respond_text(request, 413, "text/plain", "Path request is too large");
            }
        }
        let payload = if body.is_empty() {
            serde_json::json!({})
        } else {
            match serde_json::from_str(&body) {
                Ok(value) => value,
                Err(_) => return respond_text(request, 400, "text/plain", "Invalid path request"),
            }
        };
        let result = {
            let db = app.chat_db.lock().expect("chat database lock poisoned");
            match request.method() {
                &Method::Get => paths::list(&db, &app.repo_root, id)
                    .map(|items| serde_json::json!({"paths":items})),
                &Method::Post => paths::add(
                    &db,
                    &app.repo_root,
                    id,
                    &payload,
                    &path_roots,
                ),
                &Method::Delete => paths::remove(&db, &app.repo_root, id, &payload),
                _ => Err(plans::ApiError(404, "Unknown path operation").into()),
            }
        };
        return match result {
            Ok(body) => respond_text(request, 200, "application/json", body.to_string()),
            Err(error) => plan_error(request, error),
        };
    }
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
    if let Some(rest) = path.strip_prefix("/api/chat/loops/") {
        let (id, action) = rest.split_once('/').map_or((rest, ""), |(id, action)| (id, action));
        if !valid_run_id(id) { return respond_text(request, 400, "text/plain", "invalid loop id"); }
        return match (request.method().clone(), action) {
            (Method::Get, "") => match engineering_loop_snapshot(app, id)? {
                Some(body) => respond_text(request, 200, "application/json", body.to_string()),
                None => respond_text(request, 404, "text/plain", "loop not found"),
            },
            (Method::Post, "stop") => {
                let json = header_value(&request, "Content-Type").is_some_and(|c| c.starts_with("application/json"));
                if !json || !same_origin(&request) { return respond_text(request, 403, "text/plain", "forbidden"); }
                match stop_engineering_loop(app, id)? {
                    Some((state, run_id)) => respond_text(
                        request, 202, "application/json",
                        serde_json::json!({"id":id,"state":state,"current_run_id":run_id}).to_string(),
                    ),
                    None => respond_text(request, 404, "text/plain", "loop not found"),
                }
            }
            (Method::Post, "implementer") => {
                let json = header_value(&request, "Content-Type").is_some_and(|c| c.starts_with("application/json"));
                if !json || !same_origin(&request) { return respond_text(request, 403, "text/plain", "forbidden"); }
                let mut body = String::new();
                request.as_reader().take(MAX_BODY + 1).read_to_string(&mut body)?;
                if body.len() as u64 > MAX_BODY {
                    return respond_text(request, 413, "text/plain", "Loop Engineering request is too large")
                }
                let payload = match serde_json::from_str::<EngineeringLoopImplementerRequest>(&body) {
                    Ok(payload) if !payload.conversation_id.trim().is_empty() && !payload.participant.trim().is_empty() => payload,
                    _ => return respond_text(request, 400, "text/plain", "invalid implementer selection"),
                };
                match select_engineering_loop_implementer(app, id, &payload.conversation_id, &payload.participant)? {
                    EngineeringLoopImplementerDecision::Applied(body) => {
                        respond_text(request, 200, "application/json", body.to_string())
                    }
                    EngineeringLoopImplementerDecision::Missing => {
                        respond_text(request, 404, "text/plain", "loop not found")
                    }
                    EngineeringLoopImplementerDecision::Conflict(message) => {
                        respond_text(request, 409, "text/plain", message)
                    }
                }
            }
            (Method::Post, "round") => {
                let json = header_value(&request, "Content-Type").is_some_and(|c| c.starts_with("application/json"));
                if !json || !same_origin(&request) { return respond_text(request, 403, "text/plain", "forbidden"); }
                let mut body = String::new();
                request.as_reader().take(MAX_BODY + 1).read_to_string(&mut body)?;
                if body.len() as u64 > MAX_BODY {
                    return respond_text(request, 413, "text/plain", "Loop Engineering request is too large")
                }
                let payload: EngineeringLoopRoundRequest = match serde_json::from_str::<EngineeringLoopRoundRequest>(&body) {
                    Ok(payload) if matches!(payload.decision.as_str(), "approve" | "decline")
                        && !payload.conversation_id.trim().is_empty() => payload,
                    _ => return respond_text(request, 400, "text/plain", "invalid round decision"),
                };
                match decide_engineering_loop_round(app, id, &payload.conversation_id, &payload.decision)? {
                    EngineeringLoopRoundDecision::Applied(body) => {
                        respond_text(request, 200, "application/json", body.to_string())
                    }
                    EngineeringLoopRoundDecision::Missing => {
                        respond_text(request, 404, "text/plain", "loop not found")
                    }
                    EngineeringLoopRoundDecision::Conflict(message) => {
                        respond_text(request, 409, "text/plain", message)
                    }
                }
            }
            _ => respond_text(request, 404, "text/plain", "not found"),
        };
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
            (Method::Post, "force") => {
                let json = header_value(&request, "Content-Type").is_some_and(|c| c.starts_with("application/json"));
                if !json || !same_origin(&request) { return respond_text(request, 403, "text/plain", "forbidden"); }
                match force_chat_run(app, id)? {
                    ForceChatRun::Forced => respond_text(
                        request,
                        202,
                        "application/json",
                        serde_json::json!({"id":id,"state":"running","forced":true}).to_string(),
                    ),
                    ForceChatRun::NotWaiting => respond_text(
                        request, 409, "text/plain", "run is not waiting for the worktree lock",
                    ),
                    ForceChatRun::NotRunning(state) => respond_text(
                        request, 409, "text/plain", format!("run is {state}"),
                    ),
                    ForceChatRun::Missing => respond_text(request, 404, "text/plain", "run not found"),
                }
            }
            _ => respond_text(request, 404, "text/plain", "not found"),
        };
    }
    let serve_static = |req: Request, filename: &str, mime: &str, embedded: &str| -> Result<()> {
        let dev_file = app.repo_root.join("src/chat/web").join(filename);
        if dev_file.is_file() {
            if let Ok(content) = std::fs::read_to_string(&dev_file) {
                return respond_text(req, 200, mime, content);
            }
        }
        respond_text(req, 200, mime, embedded)
    };
    match (request.method().clone(), path.as_str()) {
        (Method::Get, "/") => serve_static(request, "chat.html", "text/html; charset=utf-8", INDEX_HTML),
        (Method::Get, "/chat.css") => serve_static(request, "chat.css", "text/css; charset=utf-8", CHAT_CSS),
        (Method::Get, "/chat-onboarding.js") => serve_static(request, "chat-onboarding.js", "text/javascript; charset=utf-8", ONBOARDING_JS),
        (Method::Get, "/chat.js") => serve_static(request, "chat.js", "text/javascript; charset=utf-8", CHAT_JS),
        (Method::Get, "/chat-import.js") => serve_static(request, "chat-import.js", "text/javascript; charset=utf-8", CHAT_IMPORT_JS),
        (Method::Get, "/chat-quotas.js") => serve_static(request, "chat-quotas.js", "text/javascript; charset=utf-8", CHAT_QUOTAS_JS),
        (Method::Get, "/favicon.svg" | "/favicon.ico") => {
            serve_static(request, "favicon.svg", "image/svg+xml", FAVICON_SVG)
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
            let selected = match query_agents(request.url(), &config) {
                Ok(selected) => selected,
                Err(()) => return respond_text(request, 400, "text/plain", "Select configured agents"),
            };
            let mut report = selected.as_deref().map_or_else(
                || diagnostics::snapshot(&root, &config),
                |agents| diagnostics::snapshot_agents(&root, &config, agents),
            );
            // Reuse the existing safe quota cache, without triggering generation.
            let cached = app.quotas.lock().expect("quota cache lock poisoned");
            for agent in &mut report.agents {
                if let Some(q) = cached.get(&agent.agent)
                    && q.signature == quota_signature(&config, &agent.agent) { agent.quota = if q.status.windows.iter().any(|w| w.used_pct.is_some_and(|p| p >= 100.0)) { "exhausted" } else if q.status.windows.iter().any(|w| w.used_pct.is_some_and(|p| p >= 80.0)) { "near_limit" } else if !q.status.windows.is_empty() { "available" } else { "unavailable" }.into();
                }
            }
            respond_text(request, 200, "application/json", serde_json::to_string(&report)?)
        }
        (Method::Post, "/api/diagnostics/run") => {
            if !same_origin(&request)
                || !header_value(&request, "Content-Type")
                    .is_some_and(|c| c.starts_with("application/json"))
            {
                return respond_text(request, 403, "text/plain", "forbidden");
            }
            let req: DiagnosticActionRequest =
                match serde_json::from_reader(request.as_reader().take(64 * 1024)) {
                    Ok(req) => req,
                    Err(_) => {
                        return respond_text(
                            request,
                            400,
                            "text/plain",
                            "Invalid environment action request",
                        );
                    }
                };
            let root = match crate::common::workspaces::resolve(
                &app.repo_root,
                req.checkout.as_deref(),
            ) {
                Ok(root) => root,
                Err(_) => {
                    return respond_text(
                        request,
                        400,
                        "text/plain",
                        "Select a registered worktree of this repository",
                    );
                }
            };
            let config = Config::load_merged(&root)?;
            let report = diagnostics::snapshot(&root, &config);
            let agent = report
                .agents
                .iter()
                .find(|agent| agent.action == req.command)
                .map(|agent| agent.agent.clone());
            let result = if req.command == "aic sandbox build"
                && (report.runtime.action == req.command || report.image.action == req.command)
            {
                crate::commands::sandbox::build(&config, None, None)
            } else if let Some(agent) =
                agent.filter(|_| req.command.starts_with("aic agent test "))
            {
                diagnostics::test_agent(&root, &config, &agent, Duration::from_secs(45))
                    .map(|_| ())
            } else {
                return respond_text(
                    request,
                    409,
                    "application/json",
                    serde_json::json!({
                        "error": "This action requires an interactive terminal or is no longer needed. Refresh Environment and use Copy if it still appears."
                    })
                    .to_string(),
                );
            };
            match result {
                Ok(()) => respond_text(
                    request,
                    200,
                    "application/json",
                    serde_json::json!({"ok":true}).to_string(),
                ),
                Err(err) => respond_text(
                    request,
                    422,
                    "application/json",
                    serde_json::json!({"error":format!("{err:#}")}).to_string(),
                ),
            }
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
            let config = current_config(app);
            let selected = match query_agents(request.url(), &config) {
                Ok(selected) => selected,
                Err(()) => return respond_text(request, 400, "text/plain", "Select configured agents"),
            };
            let statuses = quota_snapshot(app, selected.as_deref());
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
            struct Refresh {
                agent: Option<String>,
                #[serde(default)]
                agents: Option<Vec<String>>,
            }
            let req: Refresh = match serde_json::from_str(&body) {
                Ok(req) => req,
                Err(_) => return respond_text(request, 400, "text/plain", "invalid refresh request"),
            };
            let config = current_config(app);
            if req.agent.is_some() && req.agents.is_some() {
                return respond_text(request, 400, "text/plain", "select agent or agents, not both");
            }
            if let Some(agent) = req.agent {
                if !config.agents.contains_key(&agent) { return respond_text(request, 404, "text/plain", "unknown agent"); }
                refresh_quota(app, &agent);
            } else if let Some(mut agents) = req.agents {
                agents.sort();
                agents.dedup();
                if agents.len() > config.agents.len() || agents.iter().any(|agent| !config.agents.contains_key(agent)) {
                    return respond_text(request, 404, "text/plain", "unknown agent");
                }
                for agent in agents { refresh_quota(app, &agent); }
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

fn query_agents(url: &str, config: &Config) -> std::result::Result<Option<Vec<String>>, ()> {
    let Some(value) = query_value(url, "agents") else { return Ok(None) };
    let mut agents: Vec<String> = serde_json::from_str(&value).map_err(|_| ())?;
    agents.sort();
    agents.dedup();
    if agents.len() > config.agents.len() || agents.iter().any(|agent| !config.agents.contains_key(agent)) {
        return Err(());
    }
    Ok(Some(agents))
}

fn quota_signature(config: &Config, name: &str) -> String {
    format!("{}:{:?}:{}", config.agents[name].binary, config.sandbox.runtime, config.sandbox.image)
}

/// Snapshot returns immediately; first reads run independently in worker threads.
fn quota_snapshot(app: &Arc<App>, selected: Option<&[String]>) -> Vec<quotas::QuotaStatus> {
    let config = current_config(app);
    let mut names: Vec<_> = selected.map_or_else(
        || config.agents.keys().cloned().collect(),
        |agents| agents.to_vec(),
    );
    names.sort();
    names.dedup();
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
    if req.loop_max_rounds.is_some_and(|rounds| !(1..=20).contains(&rounds)) {
        return respond_text(request, 400, "text/plain", "loop_max_rounds must be between 1 and 20");
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
    if let Some(value) = req.serialize_worktree_writes {
        global.chat.serialize_worktree_writes = value;
    }
    if let Some(value) = req.loop_max_rounds {
        global.chat.loop_max_rounds = value;
    }
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
    if req.engineering_loop.is_some()
        && (run_id.is_none() || req.conversation_id.as_deref().is_none_or(str::is_empty)
            || req.participant.as_deref().is_none_or(str::is_empty))
    {
        return respond_text(request, 400, "text/plain", "Loop Engineering requires a durable run, conversation and participant");
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
    req.write = guard.turn.write;
    let sandboxed = config.sandbox.runtime != SandboxKind::None;
    let conversation = req.conversation_id.clone().filter(|s| !s.is_empty());
    let (additional_paths, path_note) = if let Some(id) =
        conversation.as_deref().filter(|_| sandboxed)
    {
        let global_path_roots = Config::load_global()?.sandbox.path_roots;
        let db = app.chat_db.lock().expect("chat database lock poisoned");
        let grants = match paths::list(&db, &app.repo_root, id) {
            Ok(grants) => grants,
            Err(error) => return plan_error(request, error),
        };
        let effective = match paths::effective(
            &db,
            id,
            req.write,
            &app.repo_root,
            &global_path_roots,
        ) {
            Ok(paths) => paths,
            Err(error) => return plan_error(request, error),
        };
        let note = if grants.is_empty() {
            "No additional conversation paths are mounted. The selected checkout remains the primary working directory."
                .to_string()
        } else {
            let entries = grants
                .into_iter()
                .map(|grant| {
                    let access = if req.write && grant.access == "rw" {
                        "read-write"
                    } else {
                        "read-only"
                    };
                    format!("{} ({access})", grant.path)
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Additional conversation paths: {entries}. The selected checkout remains the primary working directory."
            )
        };
        (effective, Some(note))
    } else {
        (Vec::new(), None)
    };
    if let Some(id) = &run_id {
        if let Err(err) = create_chat_run(app, id, req.conversation_id.as_deref(), req.participant.as_deref()) {
            eprintln!("creating chat run failed: {err:#}");
            return respond_text(request, 409, "text/plain", "This execution id is already in use; retry the message");
        }
    }
    if let Some(engineering_loop) = &req.engineering_loop {
        let result = begin_engineering_loop_turn(
            app,
            engineering_loop,
            req.conversation_id.as_deref().expect("validated Loop Engineering conversation"),
            req.participant.as_deref().expect("validated Loop Engineering participant"),
            run_id.as_deref().expect("validated Loop Engineering run"),
            &work_root,
            config.chat.loop_max_rounds,
        );
        if let Err(err) = result {
            if let Some(id) = &run_id {
                let db = app.chat_db.lock().expect("chat database lock poisoned");
                let _ = db.execute("DELETE FROM chat_runs WHERE id=?1", [id]);
            }
            return respond_text(request, 409, "text/plain", format!("Loop Engineering rejected: {err:#}"));
        }
    }
    let resume = req.session_id.filter(|s| !s.is_empty());
    let mut native_session = resume.clone();
    let was_resumed = resume.is_some();
    let agent_name = req.agent.clone();
    let engineering_loop_id = req.engineering_loop.as_ref().map(|engineering_loop| engineering_loop.id.clone());
    let mut accounted = false;
    let mut started = false;
    let mut engineering_loop_output = String::new();
    let mut engineering_loop_finished = false;

    // Tell the agent what it may do, so it says so instead of hunting for a
    // workaround (copying files to /tmp, writing patches nobody can apply).
    let access_note = if req.write {
        "You may edit files in this repository when the user asks you to."
    } else if sandboxed {
        "This chat has the repository mounted read-only: you cannot edit files. If asked to change something, don't look for a workaround (no copies in /tmp, no patch files); reply with the proposed change as text and say that someone with edit access has to apply it."
    } else {
        "You are not allowed to edit files in this chat. If asked to change something, reply with the proposed change as text instead of making it."
    };
    let plan_api_note = guard.turn.plan_slug.as_deref().map(|slug| format!(
        "The current chat server is available as $AIC_CHAT_API and the selected checkout as $AIC_CHAT_CHECKOUT. Create plan tasks through HTTP POST to $AIC_CHAT_API/api/plans/{slug}/tasks with Content-Type: application/json and a body containing checkout, title and the current plan version. Read the current version with GET first, then use the updated version returned by each POST for the next task. Do not start another aic chat server."
    ));
    let engineering_loop_note = req.engineering_loop.as_ref().map(engineering_loop_system);
    let system = [
        Some(access_note),
        path_note.as_deref(),
        req.system.as_deref().filter(|s| !s.trim().is_empty()),
        engineering_loop_note.as_deref(),
        (!guard.turn.system.is_empty()).then_some(guard.turn.system.as_str()),
        plan_api_note.as_deref(),
    ]
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
        } else if engineering_loop_note.is_some()
            || !guard.turn.system.is_empty()
            || path_note.is_some()
        {
            // Selected tasks and loop phases may change between turns of a
            // native session. Re-send server-owned framing on every such turn.
            let current_context = [
                path_note.as_deref(),
                engineering_loop_note.as_deref(),
                (!guard.turn.system.is_empty()).then_some(guard.turn.system.as_str()),
                plan_api_note.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n");
            prompt = format!("{access_note}\n\n{current_context}\n\n{prompt}");
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
        env: vec![
            ("AIC_CHAT_API".into(), app.api_url.clone()),
            ("AIC_CHAT_CHECKOUT".into(), work_root.to_string_lossy().into_owned()),
        ],
        additional_paths,
        host_network: sandboxed,
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
    // tiny_http starts writing the response only after its body reader yields
    // data. Open the NDJSON stream before lock coordination so the browser is
    // not left waiting for headers while the run is queued. Blank lines are
    // already defined as heartbeats and ignored by the client.
    let _ = tx.send(b"\n".to_vec());
    if let Some(execution_id) = &guard.turn.execution_id {
        emit_json(app, run_id.as_deref(), &tx, serde_json::json!({"type":"execution","id":execution_id}));
    }

    let write_lock = if req.write && config.chat.serialize_worktree_writes {
        let owner = WorktreeWriteLockOwner {
            token: format!(
                "{}-{}-{}",
                std::process::id(),
                now_millis(),
                WRITE_LOCK_IDS.fetch_add(1, Ordering::Relaxed)
            ),
            pid: std::process::id(),
            process_pid: None,
            agent: agent_name.clone(),
            participant: req
                .participant
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| agent_name.clone()),
            conversation: conversation.clone(),
            run_id: run_id.clone(),
        };
        let mut waiting_for: Option<Option<String>> = None;
        loop {
            if chat_run_was_stopped(app, run_id.as_deref()) {
                emit_chat_event(app, run_id.as_deref(), &tx, &failed("Stopped.".into()));
                if let (Some(loop_id), Some(run_id)) = (engineering_loop_id.as_deref(), run_id.as_deref()) {
                    let _ = finish_engineering_loop_turn(app, loop_id, run_id, "", false);
                }
                drop(guard);
                drop(tx);
                responder.join().map_err(|_| anyhow!("response thread panicked"))??;
                return Ok(());
            }
            if chat_run_is_forced(app, run_id.as_deref())
                || !current_config(app).chat.serialize_worktree_writes
            {
                set_chat_run_waiting_for_write_lock(app, run_id.as_deref(), false)?;
                if waiting_for.is_some() {
                    emit_json(
                        app,
                        run_id.as_deref(),
                        &tx,
                        serde_json::json!({
                            "type":"waiting", "reason":"worktree_lock", "waiting":false
                        }),
                    );
                }
                break None;
            }
            match try_worktree_write_lock(&work_root, &owner) {
                Ok(WorktreeWriteLockAttempt::Acquired(lock)) => {
                    set_chat_run_waiting_for_write_lock(app, run_id.as_deref(), false)?;
                    if waiting_for.is_some() {
                        emit_json(
                            app,
                            run_id.as_deref(),
                            &tx,
                            serde_json::json!({
                                "type":"waiting", "reason":"worktree_lock", "waiting":false
                            }),
                        );
                    }
                    break Some(lock);
                }
                Ok(WorktreeWriteLockAttempt::Waiting(holder)) => {
                    let holder_token = holder.as_ref().map(|holder| holder.token.clone());
                    if waiting_for.as_ref() != Some(&holder_token) {
                        // Persist before publishing the event: once the button is
                        // visible, its request must be accepted by the backend.
                        set_chat_run_waiting_for_write_lock(app, run_id.as_deref(), true)?;
                        let label = holder
                            .as_ref()
                            .map(|holder| holder.participant.as_str())
                            .filter(|label| !label.trim().is_empty())
                            .unwrap_or("another writer");
                        emit_json(
                            app,
                            run_id.as_deref(),
                            &tx,
                            serde_json::json!({
                                "type":"waiting", "reason":"worktree_lock",
                                "waiting":true, "holder":label
                            }),
                        );
                        waiting_for = Some(holder_token);
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(err) => {
                    emit_chat_event(
                        app,
                        run_id.as_deref(),
                        &tx,
                        &failed(format!("Could not coordinate write access: {err:#}")),
                    );
                    if let (Some(loop_id), Some(run_id)) = (engineering_loop_id.as_deref(), run_id.as_deref()) {
                        let _ = finish_engineering_loop_turn(app, loop_id, run_id, "", false);
                    }
                    drop(guard);
                    drop(tx);
                    responder.join().map_err(|_| anyhow!("response thread panicked"))??;
                    return Ok(());
                }
            }
        }
    } else {
        None
    };

    // Stop may race with the transition from the lock queue to process
    // startup. Avoid spawning at all when the request was cancelled after it
    // acquired its lease but before an active process could be registered.
    if chat_run_was_stopped(app, run_id.as_deref()) {
        emit_chat_event(app, run_id.as_deref(), &tx, &failed("Stopped.".into()));
        if let (Some(loop_id), Some(run_id)) = (engineering_loop_id.as_deref(), run_id.as_deref()) {
            let _ = finish_engineering_loop_turn(app, loop_id, run_id, "", false);
        }
        drop(guard);
        drop(write_lock);
        drop(tx);
        responder.join().map_err(|_| anyhow!("response thread panicked"))??;
        return Ok(());
    }

    match agent.spawn_stream(&opts) {
        Err(err) => {
            let _ = diagnostics::record_failure(&work_root, &config, &agent_name, &err.to_string());
            emit_chat_event(app, run_id.as_deref(), &tx, &failed(diagnostics::classify_error(&err.to_string()).1.into()));
        }
        Ok(process) => {
            if let Some(lock) = &write_lock
                && let Err(err) = lock.attach_process(process.child.id())
            {
                process.killer.kill();
                let mut child = process.child;
                let _ = child.wait();
                emit_chat_event(
                    app,
                    run_id.as_deref(),
                    &tx,
                    &failed(format!("Could not preserve write-lock ownership: {err:#}")),
                );
                if let (Some(loop_id), Some(run_id)) = (engineering_loop_id.as_deref(), run_id.as_deref()) {
                    let _ = finish_engineering_loop_turn(app, loop_id, run_id, "", false);
                }
                drop(guard);
                drop(write_lock);
                drop(tx);
                responder.join().map_err(|_| anyhow!("response thread panicked"))??;
                return Ok(());
            }
            if let Err(err) = plans::attach_process(&app.chat_db.lock().expect("chat database lock poisoned"), &guard.turn.token, &process.killer) {
                process.killer.kill();
                let mut child = process.child;
                let _ = child.wait();
                emit_chat_event(app, run_id.as_deref(), &tx, &failed("Execution could not be recorded. Preserve your draft and retry.".into()));
                if let (Some(loop_id), Some(run_id)) = (engineering_loop_id.as_deref(), run_id.as_deref()) {
                    let _ = finish_engineering_loop_turn(app, loop_id, run_id, "", false);
                }
                drop(tx);
                return Err(err);
            }
            started = true;
            if let Some(id) = &run_id {
                app.active_runs.lock().expect("active runs lock poisoned").insert(id.clone(), process.killer.clone());
            }
            // A stop can also arrive while spawn_stream or lock attachment is
            // in progress, before active_runs contains the killer. Recheck
            // after registration; from this point onward stop_chat_run can
            // always find and kill the process itself.
            if chat_run_was_stopped(app, run_id.as_deref()) {
                process.killer.kill();
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
                match &*event {
                    ChatEvent::Delta { text } => engineering_loop_output.push_str(text),
                    ChatEvent::Done { text, .. } if engineering_loop_output.is_empty() => {
                        engineering_loop_output.push_str(text)
                    }
                    _ => {}
                }
                if let ChatEvent::Session { id } = event {
                    native_session = Some(id.clone());
                    let db = app.chat_db.lock().expect("chat database lock poisoned");
                    let _ = db.execute("INSERT OR IGNORE INTO native_session_context (agent, session_id, checkout) VALUES (?1,?2,?3)", params![agent_name, id.as_str(), work_root.to_string_lossy()]);
                    if let Some(conversation) = &conversation {
                        let _ = db.execute("INSERT OR IGNORE INTO native_session_plan(agent,session_id,conversation_id,plan_slug) VALUES (?1,?2,?3,?4)",params![agent_name,id.as_str(),conversation,guard.turn.plan_slug]);
                    }
                }
                if let ChatEvent::Done { ok: false, error, .. } = event
                    && !chat_run_was_stopped(app, run_id.as_deref())
                {
                    let _ = diagnostics::record_failure(&work_root, &config, &agent_name, error.as_deref().unwrap_or("execution failed"));
                    *error = Some(diagnostics::classify_error(error.as_deref().unwrap_or("execution failed")).1.into());
                }
                if let ChatEvent::Done { ok, .. } = event {
                    guard.success = *ok;
                    if let (Some(loop_id), Some(run_id)) = (engineering_loop_id.as_deref(), run_id.as_deref()) {
                        if let Err(err) = finish_engineering_loop_turn(
                            app, loop_id, run_id, &engineering_loop_output, *ok,
                        ) {
                            eprintln!("advancing Loop Engineering failed: {err:#}");
                        }
                        engineering_loop_finished = true;
                    }
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
                emit_chat_event(app, run_id.as_deref(), &tx, event)
            });
            if let Some(id) = &run_id { app.active_runs.lock().expect("active runs lock poisoned").remove(id); }
            finished.store(true, Ordering::Relaxed);
            watcher.thread().unpark();
            let _ = watcher.join();
            if let Err(err) = result {
                guard.success = false;
                let stopped = chat_run_was_stopped(app, run_id.as_deref());
                if !stopped {
                    let _ = diagnostics::record_failure(&work_root, &config, &agent_name, &err.to_string());
                }
                let message = if stopped { "Stopped." } else { diagnostics::classify_error(&err.to_string()).1 };
                emit_chat_event(app, run_id.as_deref(), &tx, &failed(message.into()));
            }
        }
    }
    if !engineering_loop_finished
        && let (Some(loop_id), Some(run_id)) = (engineering_loop_id.as_deref(), run_id.as_deref())
        && let Err(err) = finish_engineering_loop_turn(
            app, loop_id, run_id, &engineering_loop_output, guard.success,
        )
    {
        eprintln!("advancing Loop Engineering failed: {err:#}");
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
    drop(write_lock);
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

    fn write_lock_owner(token: &str, participant: &str, conversation: &str) -> WorktreeWriteLockOwner {
        WorktreeWriteLockOwner {
            token: token.into(),
            pid: std::process::id(),
            process_pid: None,
            agent: "codex".into(),
            participant: participant.into(),
            conversation: Some(conversation.into()),
            run_id: Some(token.into()),
        }
    }

    fn test_app(root: &Path) -> App {
        App {
            repo_root: root.to_path_buf(), config: RwLock::new(Config::default()),
            chat_db: Mutex::new(open_chat_db(root).unwrap()), allowed_hosts: None, api_url: "http://127.0.0.1:7878".into(),
            models: Mutex::default(), quotas: Mutex::default(), active_runs: Mutex::default(),
        }
    }

    #[test]
    fn agent_query_filter_is_validated_and_deduplicated() {
        let config = Config::default();
        assert_eq!(
            query_agents("/?agents=%5B%22codex%22%2C%22codex%22%5D", &config).unwrap(),
            Some(vec!["codex".into()])
        );
        assert!(query_agents("/?agents=%5B%22unknown%22%5D", &config).is_err());
        assert_eq!(query_agents("/", &config).unwrap(), None);
    }

    #[test]
    fn worktree_write_lock_groups_writers_by_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let first = match try_worktree_write_lock(dir.path(), &write_lock_owner("one", "Writer One", "conversation-a")).unwrap() {
            WorktreeWriteLockAttempt::Acquired(lock) => lock,
            WorktreeWriteLockAttempt::Waiting(_) => panic!("first writer should acquire the lock"),
        };
        let path = dir.path().join(".aic/chat/worktree-write.lock");
        assert!(path.is_file());
        let peer = match try_worktree_write_lock(dir.path(), &write_lock_owner("peer", "Writer Peer", "conversation-a")).unwrap() {
            WorktreeWriteLockAttempt::Acquired(lock) => lock,
            WorktreeWriteLockAttempt::Waiting(_) => panic!("writers in the same conversation should share the lock"),
        };
        match try_worktree_write_lock(dir.path(), &write_lock_owner("two", "Writer Two", "conversation-b")).unwrap() {
            WorktreeWriteLockAttempt::Waiting(Some(holder)) => {
                assert_eq!(holder.token, "one");
                assert_eq!(holder.participant, "Writer One");
            }
            _ => panic!("second writer must wait"),
        }
        drop(first);
        assert!(path.exists(), "the same-conversation peer still holds a lease");
        drop(peer);
        assert!(!path.exists());
        assert!(matches!(
            try_worktree_write_lock(dir.path(), &write_lock_owner("two", "Writer Two", "conversation-b")).unwrap(),
            WorktreeWriteLockAttempt::Acquired(_)
        ));
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
        assert!(chat_run_was_stopped(&app, Some("run-1")));
        let done = append_chat_run_event(&app, "run-1", serde_json::json!({"type":"done","ok":false,"error":"process exited"})).unwrap();
        assert_eq!(done["stopped"], true);
        assert_eq!(chat_run_snapshot(&app, "run-1", 0).unwrap().unwrap()["state"], "stopped");
        assert!(chat_run_was_stopped(&app, Some("run-1")));
    }

    #[test]
    fn a_running_chat_run_can_bypass_the_worktree_queue() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        create_chat_run(&app, "forced-run", Some("conversation"), Some("Dev")).unwrap();
        assert!(!chat_run_is_forced(&app, Some("forced-run")));
        assert_eq!(force_chat_run(&app, "forced-run").unwrap(), ForceChatRun::NotWaiting);
        set_chat_run_waiting_for_write_lock(&app, Some("forced-run"), true).unwrap();
        assert_eq!(force_chat_run(&app, "forced-run").unwrap(), ForceChatRun::Forced);
        assert!(chat_run_is_forced(&app, Some("forced-run")));

        create_chat_run(&app, "finished-run", Some("conversation"), Some("Dev")).unwrap();
        set_chat_run_waiting_for_write_lock(&app, Some("finished-run"), true).unwrap();
        append_chat_run_event(&app, "finished-run", serde_json::json!({"type":"done","ok":true})).unwrap();
        assert_eq!(
            force_chat_run(&app, "finished-run").unwrap(),
            ForceChatRun::NotRunning("completed".into()),
        );
        assert_eq!(force_chat_run(&app, "missing-run").unwrap(), ForceChatRun::Missing);
    }

    fn loop_request(
        id: &str,
        phase: &str,
        round: u32,
        turns: u32,
        reviewer: Option<&str>,
        current: &str,
    ) -> EngineeringLoopRequest {
        EngineeringLoopRequest {
            id: id.into(), mode: "standard".into(), state: "running".into(), phase: phase.into(), round, turns,
            max_rounds: 3, max_turns: 6, implementer: Some("Impl".into()),
            reviewer: reviewer.map(str::to_owned), current: current.into(), objective: "Build it".into(),
        }
    }

    #[test]
    fn engineering_loop_accepts_equivalent_structured_verdict_formats() {
        let cases = [
            ("Done. <!-- aic-loop {\"verdict\":\"approved\"} -->", "approved"),
            ("Done. <!-- AIC-LOOP { \"Verdict\": \"APPROVED\" } -->", "approved"),
            ("```json\n{\"verdict\":\"changes requested\"}\n```", "changes_requested"),
            ("Review complete.\n\n**Verdict:** `approved`", "approved"),
            ("Review complete.\nverdict = request-changes", "changes_requested"),
            ("Review complete.\n\n## Approved", "approved"),
            ("Revisão concluída.\n\n**ALTERAÇÕES SOLICITADAS.**", "changes_requested"),
        ];
        for (output, expected) in cases {
            assert_eq!(engineering_loop_directive(output), Some(expected), "{output}");
        }
    }

    #[test]
    fn engineering_loop_rejects_missing_invalid_or_conflicting_verdicts() {
        let cases = [
            "The implementation looks approved to me.",
            "<!-- aic-loop {\"verdict\":\"maybe\"} -->",
            "{\"verdict\":\"approved\"}\n{\"verdict\":\"changes_requested\"}",
            "Verdict: approved\nVerdict: changes requested",
        ];
        for output in cases {
            assert_eq!(engineering_loop_directive(output), None, "{output}");
        }
    }

    #[test]
    fn engineering_loop_validates_and_persists_each_transition() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        let state = serde_json::json!({"convs":[{
            "id":"conversation", "participants":[
                {"name":"Impl","write":true}, {"name":"Review","write":false,"role":"Reviewer"}
            ], "log":[]
        }],"current":"conversation"});
        app.chat_db.lock().unwrap().execute(
            "INSERT INTO chat_state(id,state_json,drafts_json,updated_at) VALUES (1,?1,'{}',?2)",
            params![state.to_string(),now_millis()],
        ).unwrap();

        let root = dir.path();
        let missing_reviewer = begin_engineering_loop_turn(
            &app, &loop_request("loop-missing","implementing",1,0,None,"Impl"),
            "conversation", "Impl", "run-missing", root, 3,
        ).unwrap_err();
        assert!(missing_reviewer.to_string().contains("reviewer is required"));
        begin_engineering_loop_turn(
            &app, &loop_request("loop-1","implementing",1,0,Some("Review"),"Impl"),
            "conversation", "Impl", "run-1", root, 3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app, "loop-1", "run-1",
            "Implemented without a handoff directive.", true,
        ).unwrap();
        let review = engineering_loop_snapshot(&app,"loop-1").unwrap().unwrap();
        assert_eq!(review["phase"], "reviewing"); assert_eq!(review["turns"], 1);
        assert_eq!(review["current"], "Review");

        begin_engineering_loop_turn(
            &app, &loop_request("loop-1","reviewing",1,1,Some("Review"),"Review"),
            "conversation", "Review", "run-2", root, 3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app, "loop-1", "run-2",
            "Needs work. <!-- aic-loop {\"verdict\":\"changes_requested\",\"target\":\"Other\"} -->", true,
        ).unwrap();
        let changes = engineering_loop_snapshot(&app,"loop-1").unwrap().unwrap();
        assert_eq!(changes["phase"], "implementing"); assert_eq!(changes["round"], 2);
        assert_eq!(changes["turns"], 2); assert_eq!(changes["current"], "Impl");

        begin_engineering_loop_turn(
            &app, &loop_request("loop-1","implementing",2,2,Some("Review"),"Impl"),
            "conversation", "Impl", "run-3", root, 3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app, "loop-1", "run-3",
            "Fixed without a handoff directive.", true,
        ).unwrap();
        begin_engineering_loop_turn(
            &app, &loop_request("loop-1","reviewing",2,3,Some("Review"),"Review"),
            "conversation", "Review", "run-4", root, 3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app, "loop-1", "run-4", "Approved. <!-- aic-loop {\"verdict\":\"approved\"} -->", true,
        ).unwrap();
        let approved = engineering_loop_snapshot(&app,"loop-1").unwrap().unwrap();
        assert_eq!(approved["state"], "approved"); assert_eq!(approved["turns"], 4);
        assert!(approved["finished_at"].is_number());
    }

    #[test]
    fn review_first_loop_waits_for_a_distinct_writable_implementer() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        let review_only = serde_json::json!({"convs":[{"id":"conversation","participants":[
            {"name":"Review","write":false}
        ],"log":[]}],"current":"conversation"});
        app.chat_db.lock().unwrap().execute(
            "INSERT INTO chat_state(id,state_json,drafts_json,updated_at) VALUES (1,?1,'{}',?2)",
            params![review_only.to_string(),now_millis()],
        ).unwrap();

        let mut request = loop_request("review-first","reviewing",1,0,Some("Review"),"Review");
        request.mode = "review_first".into(); request.implementer = None;
        begin_engineering_loop_turn(
            &app,&request,"conversation","Review","review-first-1",dir.path(),3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app,"review-first","review-first-1",
            "Changes. <!-- aic-loop {\"verdict\":\"changes_requested\"} -->",true,
        ).unwrap();
        let waiting = engineering_loop_snapshot(&app,"review-first").unwrap().unwrap();
        assert_eq!(waiting["state"],"awaiting_implementer");
        assert!(waiting["implementer"].is_null());
        assert!(matches!(
            select_engineering_loop_implementer(&app,"review-first","conversation","Review").unwrap(),
            EngineeringLoopImplementerDecision::Conflict(_)
        ));

        let with_implementer = serde_json::json!({"convs":[{"id":"conversation","participants":[
            {"name":"Review","write":false},{"name":"Impl","write":true}
        ],"log":[]}],"current":"conversation"});
        app.chat_db.lock().unwrap().execute(
            "UPDATE chat_state SET state_json=?1,updated_at=?2 WHERE id=1",
            params![with_implementer.to_string(),now_millis()],
        ).unwrap();
        let EngineeringLoopImplementerDecision::Applied(selected) =
            select_engineering_loop_implementer(&app,"review-first","conversation","Impl").unwrap()
        else { panic!("writable implementer should be selected") };
        assert_eq!(selected["state"],"running"); assert_eq!(selected["phase"],"implementing");
        assert_eq!(selected["round"],2); assert_eq!(selected["current"],"Impl");

        let mut implementing = loop_request("review-first","implementing",2,1,Some("Review"),"Impl");
        implementing.mode = "review_first".into();
        begin_engineering_loop_turn(
            &app,&implementing,"conversation","Impl","review-first-2",dir.path(),3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app,"review-first","review-first-2","Corrections complete.",true,
        ).unwrap();
        let back_to_review = engineering_loop_snapshot(&app,"review-first").unwrap().unwrap();
        assert_eq!(back_to_review["phase"],"reviewing"); assert_eq!(back_to_review["current"],"Review");
    }

    #[test]
    fn stopping_engineering_loop_stops_its_current_run() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        let state = serde_json::json!({"convs":[{"id":"conversation","participants":[
            {"name":"Impl","write":true},{"name":"Review","write":false}
        ],"log":[]}],"current":"conversation"});
        app.chat_db.lock().unwrap().execute(
            "INSERT INTO chat_state(id,state_json,drafts_json,updated_at) VALUES (1,?1,'{}',?2)",
            params![state.to_string(),now_millis()],
        ).unwrap();
        create_chat_run(&app,"loop-run",Some("conversation"),Some("Impl")).unwrap();
        begin_engineering_loop_turn(
            &app, &loop_request("loop-stop","implementing",1,0,Some("Review"),"Impl"),
            "conversation", "Impl", "loop-run", dir.path(), 3,
        ).unwrap();
        assert_eq!(stop_engineering_loop(&app,"loop-stop").unwrap().unwrap().0,"stopped");
        assert!(chat_run_was_stopped(&app,Some("loop-run")));
        assert_eq!(engineering_loop_snapshot(&app,"loop-stop").unwrap().unwrap()["state"],"stopped");
    }

    #[test]
    fn engineering_loop_can_be_approved_on_its_final_allowed_turn() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        let state = serde_json::json!({"convs":[{"id":"conversation","participants":[
            {"name":"Impl","write":true},{"name":"Review","write":false}
        ],"log":[]}],"current":"conversation"});
        app.chat_db.lock().unwrap().execute(
            "INSERT INTO chat_state(id,state_json,drafts_json,updated_at) VALUES (1,?1,'{}',?2)",
            params![state.to_string(),now_millis()],
        ).unwrap();
        let mut implementing = loop_request("loop-final","implementing",1,0,Some("Review"),"Impl");
        implementing.max_rounds = 1;
        implementing.max_turns = 2;
        begin_engineering_loop_turn(
            &app, &implementing, "conversation", "Impl", "run-final-1", dir.path(), 3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app, "loop-final", "run-final-1",
            "Ready for mandatory review.", true,
        ).unwrap();
        let mut reviewing = loop_request("loop-final","reviewing",1,1,Some("Review"),"Review");
        reviewing.max_rounds = 1;
        reviewing.max_turns = 2;
        begin_engineering_loop_turn(
            &app, &reviewing, "conversation", "Review", "run-final-2", dir.path(), 3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app, "loop-final", "run-final-2",
            "Approved. <!-- aic-loop {\"verdict\":\"approved\"} -->", true,
        ).unwrap();
        let approved = engineering_loop_snapshot(&app,"loop-final").unwrap().unwrap();
        assert_eq!(approved["state"],"approved");
        assert_eq!(approved["turns"],2);
    }

    #[test]
    fn engineering_loop_requires_a_human_decision_for_each_extra_round() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        let state = serde_json::json!({"convs":[{"id":"conversation","participants":[
            {"name":"Impl","write":true},{"name":"Review","write":false}
        ],"log":[]}],"current":"conversation"});
        app.chat_db.lock().unwrap().execute(
            "INSERT INTO chat_state(id,state_json,drafts_json,updated_at) VALUES (1,?1,'{}',?2)",
            params![state.to_string(),now_millis()],
        ).unwrap();
        let mut implementing = loop_request("loop-extra","implementing",1,0,Some("Review"),"Impl");
        implementing.max_rounds = 1; implementing.max_turns = 2;
        begin_engineering_loop_turn(
            &app,&implementing,"conversation","Impl","extra-1",dir.path(),3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app,"loop-extra","extra-1",
            "Done for this round.",true,
        ).unwrap();
        let mut reviewing = loop_request("loop-extra","reviewing",1,1,Some("Review"),"Review");
        reviewing.max_rounds = 1; reviewing.max_turns = 2;
        begin_engineering_loop_turn(
            &app,&reviewing,"conversation","Review","extra-2",dir.path(),3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app,"loop-extra","extra-2",
            "Changes. <!-- aic-loop {\"verdict\":\"changes_requested\"} -->",true,
        ).unwrap();
        let waiting = engineering_loop_snapshot(&app,"loop-extra").unwrap().unwrap();
        assert_eq!(waiting["state"],"awaiting_round_approval");
        assert_eq!(waiting["round"],1); assert_eq!(waiting["turns"],2);
        assert!(matches!(
            decide_engineering_loop_round(&app,"loop-extra","other","approve").unwrap(),
            EngineeringLoopRoundDecision::Conflict(_)
        ));
        let unavailable = serde_json::json!({"convs":[{"id":"conversation","participants":[
            {"name":"Impl","write":false},{"name":"Review","write":false}
        ],"log":[]}],"current":"conversation"});
        app.chat_db.lock().unwrap().execute(
            "UPDATE chat_state SET state_json=?1,updated_at=?2 WHERE id=1",
            params![unavailable.to_string(),now_millis()],
        ).unwrap();
        assert!(matches!(
            decide_engineering_loop_round(&app,"loop-extra","conversation","approve").unwrap(),
            EngineeringLoopRoundDecision::Conflict(_)
        ));
        app.chat_db.lock().unwrap().execute(
            "UPDATE chat_state SET state_json=?1,updated_at=?2 WHERE id=1",
            params![state.to_string(),now_millis()],
        ).unwrap();
        let EngineeringLoopRoundDecision::Applied(approved) =
            decide_engineering_loop_round(&app,"loop-extra","conversation","approve").unwrap()
        else { panic!("round approval should be applied") };
        assert_eq!(approved["state"],"running"); assert_eq!(approved["round"],2);
        assert_eq!(approved["max_rounds"],2); assert_eq!(approved["max_turns"],4);

        let mut implementing = loop_request("loop-extra","implementing",2,2,Some("Review"),"Impl");
        implementing.max_rounds = 2; implementing.max_turns = 4;
        begin_engineering_loop_turn(
            &app,&implementing,"conversation","Impl","extra-3",dir.path(),3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app,"loop-extra","extra-3",
            "Again, ready for review.",true,
        ).unwrap();
        let mut reviewing = loop_request("loop-extra","reviewing",2,3,Some("Review"),"Review");
        reviewing.max_rounds = 2; reviewing.max_turns = 4;
        begin_engineering_loop_turn(
            &app,&reviewing,"conversation","Review","extra-4",dir.path(),3,
        ).unwrap();
        finish_engineering_loop_turn(
            &app,"loop-extra","extra-4",
            "Still changes. <!-- aic-loop {\"verdict\":\"changes_requested\"} -->",true,
        ).unwrap();
        assert_eq!(engineering_loop_snapshot(&app,"loop-extra").unwrap().unwrap()["state"],"awaiting_round_approval");
        let EngineeringLoopRoundDecision::Applied(declined) =
            decide_engineering_loop_round(&app,"loop-extra","conversation","decline").unwrap()
        else { panic!("round decline should be applied") };
        assert_eq!(declined["state"],"declined");
        let terminal = engineering_loop_snapshot(&app,"loop-extra").unwrap().unwrap();
        assert_eq!(terminal["state"],"declined"); assert!(terminal["finished_at"].is_number());
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
            chat_db:Mutex::new(open_chat_db(dir.path()).unwrap()), allowed_hosts:None, api_url:"http://127.0.0.1:7878".into(),
            models:Mutex::default(),quotas:Mutex::default(),active_runs:Mutex::default() });
        quota_snapshot(&app, None);
        refresh_quota(&app,"codex"); // In-flight read must not spawn another process.
        let wait = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            loop {
                let q = quota_snapshot(&app, None).into_iter().find(|q| q.agent=="codex").unwrap();
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
        assert_eq!(quota_snapshot(&app, None).into_iter().find(|q|q.agent=="codex").unwrap().fetched_at,first.fetched_at);
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
        let text = "---\nname: plan\ndescription: \"Scaffold a plan\"\nargument-hint: \"<slug> [title]\"\nallowed-tools:\n  - Bash\n  - Read\n  - Edit\n---\n\nbody";
        assert_eq!(
            parse_skill(text, "dir"),
            Some(Skill {
                name: "plan".into(),
                description: "Scaffold a plan".into(),
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
