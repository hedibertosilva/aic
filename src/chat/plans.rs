//! Server-owned shared plans. SQLite transactions serialize state changes;
//! Markdown is an atomic, retryable projection rather than the concurrency lock.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug)]
pub struct ApiError(pub u16, pub &'static str);
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.1)
    }
}
impl std::error::Error for ApiError {}
fn reject<T>(status: u16, message: &'static str) -> Result<T> {
    Err(ApiError(status, message).into())
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn id(db: &Connection) -> Result<String> {
    Ok(db.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?)
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError(400, "Missing text field").into())
}
fn version(v: &Value, key: &str) -> Result<u64> {
    v[key]
        .as_u64()
        .ok_or_else(|| ApiError(409, "Fetch the current version before changing state").into())
}
fn check_version(actual: u64, expected: u64) -> Result<()> {
    if actual != expected {
        return reject(
            409,
            "State changed; refresh before retrying and preserve your draft",
        );
    }
    Ok(())
}
fn slug_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
fn short_text(s: &str) -> Result<String> {
    if s.trim().is_empty() || s.len() > 1024 || s.contains(['\n', '\r']) {
        return reject(
            400,
            "Use a nonempty single-line title of at most 1024 bytes",
        );
    }
    Ok(s.trim().to_owned())
}

fn decode_segment(value: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut i = 0;
    while i < value.len() {
        if value.as_bytes()[i] == b'%' {
            let hex = value
                .get(i + 1..i + 3)
                .ok_or(ApiError(400, "Invalid encoded route"))?;
            bytes.push(
                u8::from_str_radix(hex, 16).map_err(|_| ApiError(400, "Invalid encoded route"))?,
            );
            i += 3;
        } else {
            bytes.push(value.as_bytes()[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).map_err(|_| ApiError(400, "Invalid UTF-8 route").into())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub checked: bool,
    pub cancelled: bool,
    pub status: String,
    pub latest_execution: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Execution {
    pub id: String,
    pub task_id: String,
    pub conversation_id: String,
    pub participant: String,
    pub agent: String,
    pub checkout: String,
    pub branch: Option<String>,
    pub sha: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub state: String,
    pub files: Vec<String>,
    pub validation: Option<String>,
    #[serde(default)]
    baseline: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub slug: String,
    pub title: String,
    pub checkout: String,
    pub version: u64,
    pub content: String,
    pub tasks: Vec<Task>,
    pub executions: Vec<Execution>,
    pub tasks_markdown: String,
    pub execution_markdown: String,
    pub projection_pending: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Origin {
    pub conversation_id: String,
    pub previous_plan: Option<String>,
    pub current_plan: Option<String>,
    pub message_count: usize,
    pub checkout: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub conversation_id: String,
    pub checkout: String,
    pub plan_slug: Option<String>,
    pub version: u64,
    pub selections: BTreeMap<String, String>,
    pub origin: Option<Origin>,
}

pub fn init(db: &Connection) -> Result<()> {
    let has_revision = db
        .prepare("PRAGMA table_info(chat_state)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|s| s == "revision");
    if !has_revision {
        db.execute_batch("ALTER TABLE chat_state ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;")?;
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS chat_plans (
      checkout TEXT NOT NULL, slug TEXT NOT NULL, document TEXT NOT NULL, dirty INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY(checkout,slug));
      CREATE TABLE IF NOT EXISTS chat_plan_bindings (conversation_id TEXT PRIMARY KEY, document TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS chat_plan_turns (
        token TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, participant TEXT NOT NULL, owner TEXT NOT NULL,
        execution_id TEXT, checkout TEXT NOT NULL, plan_slug TEXT,
        stop_target TEXT,
        UNIQUE(conversation_id,participant));
      CREATE TABLE IF NOT EXISTS chat_plan_turn_history (conversation_id TEXT PRIMARY KEY);"
    )?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS native_session_plan (
        agent TEXT NOT NULL, session_id TEXT NOT NULL, conversation_id TEXT NOT NULL,
        plan_slug TEXT, PRIMARY KEY(agent,session_id));",
    )?;
    let has_stop = db
        .prepare("PRAGMA table_info(chat_plan_turns)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|s| s == "stop_target");
    if !has_stop {
        db.execute_batch("ALTER TABLE chat_plan_turns ADD COLUMN stop_target TEXT;")?;
    }
    Ok(())
}
fn owner() -> String {
    let pid = std::process::id();
    format!("{pid}:{}", process_start(pid).unwrap_or_default())
}
fn process_start(pid: u32) -> Option<String> {
    // Linux's process start tick distinguishes reused PIDs, and also permits
    // multiple live servers using the same checkout database.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}
pub fn recover(db: &mut Connection, repo: &Path) -> Result<()> {
    let dead: Vec<(
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    )> = db
        .prepare(
            "SELECT token,owner,execution_id,checkout,plan_slug,stop_target FROM chat_plan_turns",
        )?
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (token, owner, execution, checkout, slug, target) in dead {
        let alive = owner.split_once(':').is_some_and(|(pid, start)| {
            pid.parse().ok().and_then(process_start).as_deref() == Some(start)
        });
        if !alive {
            if let Some(target) = target {
                stop_orphan(&serde_json::from_str(&target)?)?;
            }
            finish_turn(
                db,
                repo,
                &token,
                execution.as_deref(),
                &checkout,
                slug.as_deref(),
                false,
            )?;
        }
    }
    project_pending(db, repo);
    Ok(())
}
pub fn attach_process(db: &Connection, token: &str, killer: &crate::agent::Killer) -> Result<()> {
    let (pid, container) = killer.recovery_target();
    let target = json!({"pid":pid,"start":process_start(pid),"container":container});
    db.execute(
        "UPDATE chat_plan_turns SET stop_target=?1 WHERE token=?2",
        params![target.to_string(), token],
    )?;
    Ok(())
}
fn stop_orphan(target: &Value) -> Result<()> {
    if let Some(container) = target["container"].as_array() {
        let runtime = container.first().and_then(Value::as_str).unwrap_or("");
        let name = container.get(1).and_then(Value::as_str).unwrap_or("");
        if matches!(runtime, "docker" | "podman")
            && name.starts_with("aic-")
            && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            let mut command = Command::new(runtime);
            command
                .args(["kill", name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if let Ok(mut process) = command.spawn() {
                let stop = std::time::Instant::now() + std::time::Duration::from_secs(3);
                loop {
                    if process.try_wait()?.is_some() {
                        break;
                    }
                    if std::time::Instant::now() >= stop {
                        let _ = process.kill();
                        let _ = process.wait();
                        return reject(
                            503,
                            "Container cleanup pending; retry recovery before executing tasks",
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }
    if let (Some(pid), Some(start)) = (target["pid"].as_u64(), target["start"].as_str()) {
        if let Ok(pid) = u32::try_from(pid) {
            if process_start(pid).as_deref() == Some(start) {
                crate::agent::Killer::process_group(pid).kill();
            }
        }
    }
    Ok(())
}

fn safe_dir(repo: &Path, root: &Path, slug: &str, create: bool) -> Result<PathBuf> {
    if !slug_ok(slug) {
        return reject(400, "Invalid plan slug");
    }
    let root = crate::common::workspaces::resolve(repo, root.to_str())?;
    let mut dir = root.clone();
    for part in [".aic", "plans", slug] {
        dir.push(part);
        match std::fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
                return reject(400, "Plan paths must be real checkout-local directories");
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                std::fs::create_dir(&dir)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return reject(404, "Plan not found");
            }
            Err(e) => return Err(e.into()),
        }
    }
    for name in ["PLAN.md", "TASKS.md", "EXECUTION.md"] {
        if let Ok(m) = std::fs::symlink_metadata(dir.join(name))
            && (!m.is_file() || m.file_type().is_symlink())
        {
            return reject(400, "Plan documents must be regular local files");
        }
    }
    Ok(dir)
}
fn read_file(path: &Path, fallback: &str) -> Result<String> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 4 * 1024 * 1024 => reject(413, "Plan document exceeds 4 MiB"),
        Ok(_) => Ok(std::fs::read_to_string(path)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(fallback.to_owned()),
        Err(e) => Err(e.into()),
    }
}
fn task_line(line: &str) -> Option<(String, String, bool, bool)> {
    let line = line.trim_start();
    let rest = line
        .strip_prefix("- [")
        .or_else(|| line.strip_prefix("* ["))?;
    let (mark, rest) = rest.split_once(']')?;
    if !matches!(mark, " " | "x" | "X" | "*") {
        return None;
    }
    let rest = rest.trim().trim_start_matches("**");
    let n = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .unwrap_or(rest.len());
    let id = &rest[..n];
    if id.is_empty() || !id.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let title = rest[n..]
        .trim()
        .trim_start_matches(['—', '-', ':'])
        .trim()
        .replace("**", "");
    Some((id.into(), title, matches!(mark, "x" | "X"), mark == "*"))
}
fn parse_tasks(markdown: &str) -> Result<Vec<Task>> {
    let mut tasks = Vec::new();
    let mut ids = BTreeSet::new();
    let mut fence = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if let Some((id, title, checked, cancelled)) = task_line(line) {
            if !ids.insert(id.clone()) {
                return reject(409, "Duplicate task identifiers in TASKS.md");
            }
            tasks.push(Task {
                id,
                title,
                checked,
                cancelled,
                status: if cancelled {
                    "cancelled"
                } else if checked {
                    "completed"
                } else {
                    "pending"
                }
                .into(),
                latest_execution: None,
            });
        }
    }
    Ok(tasks)
}
fn load_plan(db: &Connection, repo: &Path, root: &Path, slug: &str) -> Result<Plan> {
    let key = root.to_string_lossy();
    let saved: Option<String> = db
        .query_row(
            "SELECT document FROM chat_plans WHERE checkout=?1 AND slug=?2",
            params![key, slug],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(saved) = saved {
        let plan: Plan = serde_json::from_str(&saved)?;
        if let Err(e) = safe_dir(repo, root, slug, false) {
            if !plan.projection_pending || !e.downcast_ref::<ApiError>().is_some_and(|e| e.0 == 404)
            {
                return Err(e);
            }
        }
        return Ok(plan);
    }
    let dir = safe_dir(repo, root, slug, false)?;
    if !dir.join("PLAN.md").is_file() {
        return reject(404, "PLAN.md not found");
    }
    let content = read_file(&dir.join("PLAN.md"), "")?;
    let tasks_markdown = read_file(&dir.join("TASKS.md"), "# Tasks\n")?;
    let title = content
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .unwrap_or(slug)
        .to_owned();
    let p = Plan {
        slug: slug.into(),
        title,
        checkout: key.into_owned(),
        version: 1,
        content,
        tasks: parse_tasks(&tasks_markdown)?,
        executions: vec![],
        tasks_markdown,
        execution_markdown: read_file(&dir.join("EXECUTION.md"), "# Execution\n")?,
        projection_pending: false,
    };
    db.execute(
        "INSERT OR IGNORE INTO chat_plans(checkout,slug,document) VALUES (?1,?2,?3)",
        params![p.checkout, p.slug, serde_json::to_string(&p)?],
    )?;
    Ok(p)
}
fn destination(db: &Connection, repo: &Path, root: &Path, slug: &str) -> Result<()> {
    if !slug_ok(slug) {
        return reject(400, "Invalid plan slug");
    }
    let saved: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_plans WHERE checkout=?1 AND slug=?2)",
        params![root.to_string_lossy(), slug],
        |r| r.get(0),
    )?;
    if saved || std::fs::symlink_metadata(root.join(".aic/plans").join(slug)).is_ok() {
        return reject(409, "Select an unused destination plan slug");
    }
    match safe_dir(repo, root, slug, false) {
        Err(e) if e.downcast_ref::<ApiError>().is_some_and(|e| e.0 == 404) => Ok(()),
        Err(e) => Err(e),
        Ok(_) => reject(409, "Plan already exists"),
    }
}
fn task_note(p: &mut Plan, task_id: &str, participant: &str, state: &str) {
    let mut out = Vec::new();
    let mut skip = false;
    let mut fenced = false;
    for line in p.tasks_markdown.lines() {
        if skip && line.starts_with("  - Last execution:") {
            skip = false;
            continue;
        }
        skip = false;
        out.push(line.to_owned());
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced && task_line(line).is_some_and(|(id, _, _, _)| id == task_id) {
            out.push(format!(
                "  - Last execution: {} · {}",
                participant.replace(['\n', '\r'], " "),
                state
            ));
            skip = true;
        }
    }
    p.tasks_markdown = out.join("\n") + "\n";
}
fn ensure_unedited(repo: &Path, p: &Plan) -> Result<()> {
    if p.projection_pending {
        return Ok(());
    }
    let dir = safe_dir(repo, Path::new(&p.checkout), &p.slug, false)?;
    for (name, expected, fallback) in [
        ("PLAN.md", &p.content, ""),
        ("TASKS.md", &p.tasks_markdown, "# Tasks\n"),
        ("EXECUTION.md", &p.execution_markdown, "# Execution\n"),
    ] {
        if read_file(&dir.join(name), fallback)? != *expected {
            return reject(
                409,
                "Plan documents changed externally; use the plan reload endpoint before editing",
            );
        }
    }
    Ok(())
}
fn save_plan(db: &Connection, p: &mut Plan) -> Result<()> {
    p.version += 1;
    p.projection_pending = true;
    db.execute(
        "INSERT INTO chat_plans(checkout,slug,document,dirty) VALUES (?1,?2,?3,1)
        ON CONFLICT(checkout,slug) DO UPDATE SET document=excluded.document,dirty=1",
        params![p.checkout, p.slug, serde_json::to_string(&p)?],
    )?;
    Ok(())
}
fn atomic_file(path: &Path, content: &str) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("Plan parent missing")?)?;
    file.write_all(content.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub fn project_pending(db: &mut Connection, repo: &Path) {
    let result = (|| -> Result<()> {
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let dirty: Vec<String> = tx
            .prepare("SELECT document FROM chat_plans WHERE dirty=1")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for doc in dirty {
            let mut p: Plan = serde_json::from_str(&doc)?;
            let dir = safe_dir(repo, Path::new(&p.checkout), &p.slug, true)?;
            atomic_file(&dir.join("PLAN.md"), &p.content)?;
            atomic_file(&dir.join("TASKS.md"), &p.tasks_markdown)?;
            atomic_file(&dir.join("EXECUTION.md"), &p.execution_markdown)?;
            p.projection_pending = false;
            tx.execute(
                "UPDATE chat_plans SET document=?1,dirty=0 WHERE checkout=?2 AND slug=?3",
                params![serde_json::to_string(&p)?, p.checkout, p.slug],
            )?;
        }
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("plan projection pending: {e}");
    }
}

pub fn snapshot(db: &Connection) -> Result<(Value, Value, u64)> {
    let row: Option<(String, String, u64)> = db
        .query_row(
            "SELECT state_json,drafts_json,revision FROM chat_state WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        Some((s, d, v)) => Ok((serde_json::from_str(&s)?, serde_json::from_str(&d)?, v)),
        None => Ok((json!({"convs":[],"current":null}), json!({}), 0)),
    }
}
fn put_snapshot(db: &Connection, state: &Value, drafts: &Value, revision: u64) -> Result<()> {
    db.execute("INSERT INTO chat_state(id,state_json,drafts_json,updated_at,revision) VALUES (1,?1,?2,unixepoch(),?3)
        ON CONFLICT(id) DO UPDATE SET state_json=excluded.state_json,drafts_json=excluded.drafts_json,updated_at=excluded.updated_at,revision=excluded.revision",
        params![state.to_string(),drafts.to_string(),revision])?;
    Ok(())
}
fn conversation<'a>(state: &'a Value, name: &str) -> Result<&'a Value> {
    state["convs"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["id"] == name))
        .ok_or_else(|| ApiError(404, "Conversation not found; save it first").into())
}
fn checkout(repo: &Path, c: &Value) -> Result<String> {
    Ok(
        crate::common::workspaces::resolve(repo, c["checkout"].as_str())?
            .canonicalize()?
            .to_string_lossy()
            .into_owned(),
    )
}
pub fn binding(db: &Connection, repo: &Path, name: &str) -> Result<Binding> {
    let doc: Option<String> = db
        .query_row(
            "SELECT document FROM chat_plan_bindings WHERE conversation_id=?1",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(doc) = doc {
        return Ok(serde_json::from_str(&doc)?);
    }
    let (state, _, _) = snapshot(db)?;
    let c = conversation(&state, name)?;
    // Archived free chats remain readable after their worktree is removed.
    // Execution and mutations still validate the selected registered checkout.
    let root = checkout(repo, c).unwrap_or_else(|_| {
        c["checkout"]
            .as_str()
            .unwrap_or_else(|| repo.to_str().unwrap_or(""))
            .to_owned()
    });
    Ok(Binding {
        conversation_id: name.into(),
        checkout: root,
        plan_slug: None,
        version: 0,
        selections: BTreeMap::new(),
        origin: None,
    })
}
fn put_binding(db: &Connection, b: &mut Binding) -> Result<()> {
    b.version += 1;
    db.execute(
        "INSERT INTO chat_plan_bindings(conversation_id,document) VALUES (?1,?2)
        ON CONFLICT(conversation_id) DO UPDATE SET document=excluded.document",
        params![b.conversation_id, serde_json::to_string(b)?],
    )?;
    Ok(())
}
fn busy(db: &Connection, name: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_plan_turns WHERE conversation_id=?1)",
        [name],
        |r| r.get(0),
    )?)
}
fn not_busy(db: &Connection, name: &str) -> Result<()> {
    if busy(db, name)? {
        return reject(
            409,
            "Wait for this conversation's running replies before changing its context",
        );
    }
    Ok(())
}
pub fn embed(db: &Connection, repo: &Path, state: &mut Value) -> Result<()> {
    if let Some(cs) = state["convs"].as_array_mut() {
        for c in cs {
            let Some(name) = c["id"].as_str() else {
                continue;
            };
            let b = binding(db, repo, name)?;
            c["plan_binding"] = serde_json::to_value(b)?;
        }
    }
    Ok(())
}
pub fn save_snapshot(
    db: &mut Connection,
    repo: &Path,
    mut state: Value,
    drafts: Value,
    expected: Option<u64>,
) -> Result<u64> {
    if !state.is_object() || !drafts.is_object() || !state["convs"].is_array() {
        return reject(400, "State, conversations and drafts are required");
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (previous, _, revision) = snapshot(&tx)?;
    let managed: bool =
        tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_plan_bindings)", [], |r| {
            r.get(0)
        })?;
    if let Some(expected) = expected {
        check_version(revision, expected)?;
    } else if managed {
        return reject(
            409,
            "Include the current chat-state revision; preserve drafts and refresh",
        );
    }
    let mut names = BTreeSet::new();
    for c in state["convs"].as_array_mut().unwrap() {
        let name = text(c, "id")?.to_owned();
        if name.len() > 200
            || !c["participants"].is_array()
            || !c["log"].is_array()
            || c["participants"].as_array().unwrap().iter().any(|p| {
                !p.is_object()
                    || p["name"]
                        .as_str()
                        .is_none_or(|s| s.is_empty() || s.len() > 100 || s.contains(['\n', '\r']))
                    || !p["agent"].is_string()
            })
            || c["log"].as_array().unwrap().iter().any(|e| !e.is_object())
        {
            return reject(400, "Invalid conversation, participants or history");
        }
        if !names.insert(name.clone()) {
            return reject(400, "Duplicate conversation IDs");
        }
        if let Ok(old) = conversation(&previous, &name) {
            let b = binding(&tx, repo, &name)?;
            if b.plan_slug.is_some()
                && checkout(repo, c).ok().as_deref() != Some(b.checkout.as_str())
            {
                return reject(
                    409,
                    "Resolve the plan binding through the checkout endpoint first",
                );
            }
            if busy(&tx, &name)?
                && (c["participants"] != old["participants"] || c["checkout"] != old["checkout"])
            {
                // Session/seen cursors update while streaming. Only protect the
                // execution identity/permission, not those client-side cursors.
                let identities = |v: &Value| -> Vec<Value> {
                    v["participants"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| json!([p["name"], p["agent"], p["write"]]))
                        .collect()
                };
                if identities(c) != identities(old) || c["checkout"] != old["checkout"] {
                    return reject(
                        409,
                        "Cannot change execution participants or checkout while replies run",
                    );
                }
            }
        }
        // Never accept a binding supplied by a browser snapshot.
        c.as_object_mut()
            .context("Conversation must be an object")?
            .remove("plan_binding");
    }
    for old in previous["convs"].as_array().into_iter().flatten() {
        let Some(name) = old["id"].as_str() else {
            continue;
        };
        if !names.contains(name) {
            not_busy(&tx, name)?;
            tx.execute(
                "DELETE FROM chat_plan_bindings WHERE conversation_id=?1",
                [name],
            )?;
        }
    }
    // Persist before hydration so new free conversations can be resolved.
    put_snapshot(&tx, &state, &drafts, revision + 1)?;
    embed(&tx, repo, &mut state)?;
    put_snapshot(&tx, &state, &drafts, revision + 1)?;
    tx.commit()?;
    Ok(revision + 1)
}

fn reset_sessions(c: &mut Value) {
    let seen = c["log"].as_array().map_or(0, Vec::len);
    for p in c["participants"].as_array_mut().into_iter().flatten() {
        p["session"] = json!("");
        p["seen"] = json!(seen);
        p.as_object_mut().unwrap().remove("task_id");
    }
    for field in [
        "active_task",
        "planTasks",
        "planBinding",
        "planOrigin",
        "planMode",
        "gitCurrent",
    ] {
        c.as_object_mut().unwrap().remove(field);
    }
}
fn change_binding(
    tx: &Transaction<'_>,
    repo: &Path,
    name: &str,
    req: &Value,
    selected_checkout: Option<String>,
    force_copy: bool,
) -> Result<Value> {
    not_busy(tx, name)?;
    let (mut state, drafts, rev) = snapshot(tx)?;
    check_version(rev, version(req, "state_revision")?)?;
    let source = conversation(&state, name)?.clone();
    let mut b = binding(tx, repo, name)?;
    if !force_copy {
        check_version(b.version, version(req, "version")?)?;
        if req.get("plan_slug").is_none() {
            return reject(
                400,
                "Choose an explicit plan slug or null for a free conversation",
            );
        }
    }
    let plan = if force_copy {
        b.plan_slug.clone()
    } else {
        match &req["plan_slug"] {
            Value::Null => None,
            Value::String(s) if slug_ok(s) => Some(s.clone()),
            _ => return reject(400, "Invalid plan slug"),
        }
    };
    let root = selected_checkout.unwrap_or_else(|| b.checkout.clone());
    if let Some(slug) = &plan {
        load_plan(tx, repo, Path::new(&root), slug)?;
    }
    let changed = b.plan_slug != plan || b.checkout != root;
    if !changed && !force_copy {
        return Ok(json!({"binding":b,"conversation":source,"forked":false,"revision":rev}));
    }
    let with_history = req["with_history"].as_bool().unwrap_or(true);
    let history = source["log"].as_array().is_some_and(|l| !l.is_empty());
    let fork = force_copy || history;
    if fork && !force_copy && req["confirm_fork"] != true {
        return reject(
            409,
            "Changing a plan with history requires confirm_fork: true",
        );
    }
    let mut c = source.clone();
    if fork {
        let new_id = id(tx)?;
        c["id"] = json!(new_id);
        c["title"] = json!(format!(
            "Fork: {}",
            source["title"].as_str().unwrap_or("conversation")
        ));
        if !with_history && force_copy {
            c["log"] = json!([]);
            c["round"] = json!(0);
        }
        b.origin = Some(Origin {
            conversation_id: name.into(),
            previous_plan: b.plan_slug.clone(),
            current_plan: plan.clone(),
            message_count: source["log"].as_array().map_or(0, Vec::len),
            checkout: b.checkout.clone(),
        });
        b.conversation_id = new_id;
        b.version = 0;
    }
    b.checkout = root.clone();
    b.plan_slug = plan;
    b.selections.clear();
    put_binding(tx, &mut b)?;
    reset_sessions(&mut c);
    c["checkout"] = json!(root);
    c["keepEmpty"] = json!(true);
    c["updated"] = json!(chrono::Utc::now().timestamp_millis());
    if let Ok(git) = crate::diagnostics::git_context(Path::new(&root)) {
        c["gitOrigin"] = serde_json::to_value(git)?;
    }
    c.as_object_mut().unwrap().remove("gitAcknowledged");
    c["plan_binding"] = serde_json::to_value(&b)?;
    if fork {
        state["convs"].as_array_mut().unwrap().insert(0, c.clone());
    } else {
        *state["convs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|c| c["id"] == name)
            .unwrap() = c.clone();
    }
    state["current"] = json!(b.conversation_id);
    put_snapshot(tx, &state, &drafts, rev + 1)?;
    Ok(json!({"binding":b,"conversation":c,"forked":fork,"revision":rev+1}))
}

/// All routes run under the same database lock as chat snapshots.
pub fn route(
    db: &mut Connection,
    repo: &Path,
    method: &str,
    path: &str,
    selected: Option<&str>,
    req: &Value,
) -> Result<Value> {
    project_pending(db, repo);
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let root = crate::common::workspaces::resolve(repo, req["checkout"].as_str().or(selected))
        .map_err(|_| ApiError(400, "Select a registered checkout"))?
        .canonicalize()?;
    let decoded: Vec<_> = path
        .trim_matches('/')
        .split('/')
        .map(decode_segment)
        .collect::<Result<_>>()?;
    let parts: Vec<_> = decoded.iter().map(String::as_str).collect();
    let result = if parts.get(1) == Some(&"plans") {
        if parts.len() == 2 && method == "GET" {
            let parent = root.join(".aic/plans");
            let mut plans = Vec::new();
            if parent.exists() {
                // Validate ancestors as well before enumerating any plan files.
                if std::fs::symlink_metadata(root.join(".aic"))?
                    .file_type()
                    .is_symlink()
                    || std::fs::symlink_metadata(&parent)?.file_type().is_symlink()
                {
                    return reject(400, "Plan directories cannot be symlinks");
                }
                for entry in std::fs::read_dir(parent)? {
                    let entry = entry?;
                    let slug = entry.file_name().to_string_lossy().into_owned();
                    if entry.file_type()?.is_dir()
                        && slug_ok(&slug)
                        && entry.path().join("PLAN.md").is_file()
                    {
                        plans.push(load_plan(&tx, repo, &root, &slug)?);
                    }
                }
            }
            plans.sort_by(|a, b| a.slug.cmp(&b.slug));
            json!({"plans":plans})
        } else if parts.len() == 2 && method == "POST" {
            let slug = text(req, "slug")?;
            let title = short_text(text(req, "title")?)?;
            destination(&tx, repo, &root, slug)?;
            let content = req["content"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("# {title}\n\n## Objective\n\n{title}\n"));
            if content.len() > 4 * 1024 * 1024 {
                return reject(413, "Plan exceeds 4 MiB");
            }
            let mut p = Plan {
                slug: slug.into(),
                title,
                checkout: root.to_string_lossy().into_owned(),
                version: 0,
                content,
                tasks: vec![],
                executions: vec![],
                tasks_markdown: "# Tasks\n".into(),
                execution_markdown: "# Execution\n".into(),
                projection_pending: true,
            };
            save_plan(&tx, &mut p)?;
            serde_json::to_value(p)?
        } else {
            let slug = *parts.get(2).ok_or(ApiError(404, "Unknown plan route"))?;
            let mut p = load_plan(&tx, repo, &root, slug)?;
            if method == "GET" && parts.len() == 3 {
                serde_json::to_value(&p)?
            } else if method == "POST" {
                check_version(p.version, version(req, "version")?)?;
                if parts.get(3) == Some(&"reload") && parts.len() == 4 {
                    if p.projection_pending {
                        return reject(
                            503,
                            "Plan documents are pending projection; correct file access and retry before reloading",
                        );
                    }
                    if p.executions.iter().any(|e| e.state == "running") {
                        return reject(
                            409,
                            "Wait for running executions before reloading documents",
                        );
                    }
                    let dir = safe_dir(repo, &root, slug, false)?;
                    p.content = read_file(&dir.join("PLAN.md"), "")?;
                    p.tasks_markdown = read_file(&dir.join("TASKS.md"), "# Tasks\n")?;
                    let mut tasks = parse_tasks(&p.tasks_markdown)?;
                    for t in &mut tasks {
                        if let Some(old) = p.tasks.iter().find(|old| old.id == t.id) {
                            t.latest_execution = old.latest_execution.clone();
                        }
                    }
                    p.tasks = tasks;
                    p.execution_markdown = read_file(&dir.join("EXECUTION.md"), "# Execution\n")?;
                    save_plan(&tx, &mut p)?;
                    serde_json::to_value(&p)?
                } else {
                    ensure_unedited(repo, &p)?;
                    match parts.as_slice() {
                        [_, _, _, "tasks"] => {
                            let title = short_text(text(req, "title")?)?;
                            let mut n = 1;
                            while p.tasks.iter().any(|t| t.id == format!("T{n:03}")) {
                                n += 1;
                            }
                            let task_id = format!("T{n:03}");
                            p.tasks_markdown
                                .push_str(&format!("\n- [ ] {task_id} {title}\n"));
                            p.tasks.push(Task {
                                id: task_id,
                                title,
                                checked: false,
                                cancelled: false,
                                status: "pending".into(),
                                latest_execution: None,
                            });
                        }
                        [_, _, _, "tasks", task_id, "complete"] => {
                            if req["human_confirmed"] != true {
                                return reject(
                                    400,
                                    "Completion requires explicit human confirmation; an agent reply is not validation",
                                );
                            }
                            if p.executions
                                .iter()
                                .any(|e| e.task_id == *task_id && e.state == "running")
                            {
                                return reject(409, "Task still has a running execution");
                            }
                            let task = p
                                .tasks
                                .iter_mut()
                                .find(|t| t.id == *task_id)
                                .ok_or(ApiError(404, "Task not found"))?;
                            if task.cancelled {
                                return reject(409, "Cannot complete a cancelled task");
                            }
                            let evidence = req["validation"]
                                .as_str()
                                .filter(|s| !s.trim().is_empty())
                                .unwrap_or("Confirmed by user")
                                .to_owned();
                            if let Some(execution_id) = req["execution_id"]
                                .as_str()
                                .or(task.latest_execution.as_deref())
                            {
                                let e = p
                                    .executions
                                    .iter_mut()
                                    .find(|e| e.id == execution_id && e.task_id == *task_id)
                                    .ok_or(ApiError(
                                        404,
                                        "Execution does not belong to this task",
                                    ))?;
                                e.state = "completed".into();
                                e.validation = Some(evidence.clone());
                            }
                            task.checked = true;
                            task.status = "completed".into();
                            let mut fenced = false;
                            p.tasks_markdown = p
                                .tasks_markdown
                                .lines()
                                .map(|line| {
                                    if line.trim_start().starts_with("```") {
                                        fenced = !fenced;
                                    }
                                    if !fenced
                                        && task_line(line)
                                            .is_some_and(|(id, _, _, _)| id == *task_id)
                                    {
                                        line.replacen("[ ]", "[x]", 1)
                                    } else {
                                        line.to_owned()
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                                + "\n";
                            p.execution_markdown.push_str(&format!(
                                "\n## {task_id} · Human validation · {}\n\n{}\n",
                                now(),
                                evidence
                            ));
                            task_note(&mut p, task_id, "User", "completed · human confirmed");
                        }
                        _ => return reject(404, "Unknown plan operation"),
                    }
                    save_plan(&tx, &mut p)?;
                    serde_json::to_value(&p)?
                }
            } else {
                return reject(404, "Unknown plan operation");
            }
        }
    } else if parts.get(1) == Some(&"conversations") {
        let name = *parts.get(2).ok_or(ApiError(404, "Conversation required"))?;
        if method == "POST"
            && parts.get(3) != Some(&"checkout")
            && (req["checkout"].is_string() || selected.is_some())
        {
            if binding(&tx, repo, name)?.checkout != root.to_string_lossy() {
                return reject(
                    409,
                    "The conversation checkout changed; refresh its binding",
                );
            }
        }
        match (method, parts.as_slice()) {
            ("GET", [_, _, _, "plan"]) => serde_json::to_value(binding(&tx, repo, name)?)?,
            ("POST", [_, _, _, "plan"]) => change_binding(&tx, repo, name, req, None, false)?,
            ("POST", [_, _, _, "fork"]) => change_binding(&tx, repo, name, req, None, true)?,
            ("POST", [_, _, _, "tasks"]) => {
                not_busy(&tx, name)?;
                let mut b = binding(&tx, repo, name)?;
                check_version(b.version, version(req, "version")?)?;
                if req.get("task_id").is_none() {
                    return reject(
                        400,
                        "Choose an explicit task ID or null to clear the selection",
                    );
                }
                let participant = text(req, "participant")?;
                let (state, _, _) = snapshot(&tx)?;
                if !conversation(&state, name)?["participants"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|p| p["name"] == participant)
                {
                    return reject(404, "Participant not found");
                }
                if let Some(task) = req["task_id"].as_str() {
                    let slug = b
                        .plan_slug
                        .as_deref()
                        .ok_or(ApiError(409, "Free conversations have no task selections"))?;
                    let p = load_plan(&tx, repo, Path::new(&b.checkout), slug)?;
                    if !p
                        .tasks
                        .iter()
                        .any(|t| t.id == task && !t.checked && !t.cancelled)
                    {
                        return reject(409, "Select a pending task");
                    }
                    b.selections.insert(participant.into(), task.into());
                } else {
                    b.selections.remove(participant);
                }
                put_binding(&tx, &mut b)?;
                serde_json::to_value(b)?
            }
            ("POST", [_, _, _, "checkout"]) => {
                let b = binding(&tx, repo, name)?;
                let mut request = req.clone();
                let action = req["plan_action"].as_str().unwrap_or("unlink");
                if b.plan_slug.is_some() && !req["plan_action"].is_string() {
                    return reject(
                        409,
                        "Choose migrate, unlink or select when changing a planned checkout",
                    );
                }
                let plan = match action {
                    "unlink" => None,
                    "select" => Some(text(req, "plan_slug")?.to_owned()),
                    "migrate" => {
                        let slug = b
                            .plan_slug
                            .as_deref()
                            .ok_or(ApiError(409, "No plan to migrate"))?;
                        let mut p = load_plan(&tx, repo, Path::new(&b.checkout), slug)?;
                        ensure_unedited(repo, &p)?;
                        let target = req["plan_slug"].as_str().unwrap_or(slug);
                        destination(&tx, repo, &root, target)?;
                        p.execution_markdown.push_str(&format!("\nMigrated copy from {} / {} at {}. Original execution records remain in their source checkout.\n",p.checkout,p.slug,now()));
                        p.checkout = root.to_string_lossy().into_owned();
                        p.slug = target.into();
                        p.version = 0;
                        p.executions.clear();
                        for t in &mut p.tasks {
                            t.latest_execution = None;
                            t.status = if t.checked {
                                "completed"
                            } else if t.cancelled {
                                "cancelled"
                            } else {
                                "pending"
                            }
                            .into();
                        }
                        save_plan(&tx, &mut p)?;
                        Some(target.to_owned())
                    }
                    _ => return reject(400, "Choose migrate, unlink or select"),
                };
                request["plan_slug"] = json!(plan);
                change_binding(
                    &tx,
                    repo,
                    name,
                    &request,
                    Some(root.to_string_lossy().into_owned()),
                    false,
                )?
            }
            ("POST", [_, _, _, "plan", "undo"]) => {
                not_busy(&tx, name)?;
                let b = binding(&tx, repo, name)?;
                check_version(b.version, version(req, "version")?)?;
                let source = b
                    .origin
                    .as_ref()
                    .ok_or(ApiError(409, "This conversation is not a managed fork"))?;
                let (mut state, mut drafts, rev) = snapshot(&tx)?;
                check_version(rev, version(req, "state_revision")?)?;
                conversation(&state, &source.conversation_id)?;
                let used: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM chat_plan_turn_history WHERE conversation_id=?1)",
                    [name],
                    |r| r.get(0),
                )?;
                if !used {
                    state["convs"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|c| c["id"] != name);
                    drafts.as_object_mut().unwrap().remove(name);
                    tx.execute(
                        "DELETE FROM chat_plan_bindings WHERE conversation_id=?1",
                        [name],
                    )?;
                }
                state["current"] = json!(source.conversation_id);
                put_snapshot(&tx, &state, &drafts, rev + 1)?;
                json!({"undone":!used,"conversation_id":source.conversation_id,"revision":rev+1})
            }
            _ => return reject(404, "Unknown conversation plan operation"),
        }
    } else {
        return reject(404, "Unknown plan API route");
    };
    tx.commit()?;
    project_pending(db, repo);
    // Return the persisted projection flag, including successful projection.
    if result.get("tasks").is_some() && result["slug"].is_string() {
        return serde_json::to_value(load_plan(
            db,
            repo,
            &root,
            result["slug"].as_str().unwrap(),
        )?)
        .map_err(Into::into);
    }
    Ok(result)
}

pub struct Turn {
    pub token: String,
    pub execution_id: Option<String>,
    pub checkout: String,
    pub plan_slug: Option<String>,
    pub system: String,
    pub write: bool,
}
fn file_state(root: &Path) -> BTreeMap<String, String> {
    use std::hash::{Hash, Hasher};
    let mut names = BTreeSet::new();
    for args in [
        vec!["diff", "--name-only", "-z"],
        vec!["diff", "--cached", "--name-only", "-z"],
        vec!["ls-files", "--others", "--exclude-standard", "-z"],
    ] {
        if let Ok(out) = Command::new("git").args(args).current_dir(root).output() {
            if out.status.success() {
                for name in String::from_utf8_lossy(&out.stdout)
                    .split('\0')
                    .filter(|s| !s.is_empty() && !s.starts_with(".aic/") && *s != ".aic")
                {
                    names.insert(name.to_owned());
                }
            }
        }
    }
    names
        .into_iter()
        .map(|name| {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            let path = root.join(&name);
            if let Ok(m) = std::fs::symlink_metadata(&path) {
                m.len().hash(&mut hash);
                m.modified().ok().hash(&mut hash);
                if m.is_file() && m.len() <= 8 * 1024 * 1024 {
                    if let Ok(bytes) = std::fs::read(&path) {
                        bytes.hash(&mut hash);
                    }
                }
            }
            (name, format!("{:x}", hash.finish()))
        })
        .collect()
}
/// Reserve a turn before provider launch. Planned edit attempts acquire a task;
/// discussion is forced read-only even for participants with edit permission.
pub fn begin_turn(db: &mut Connection, repo: &Path, root: &Path, req: &Value) -> Result<Turn> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let conversation_id = req["conversation_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    let agent = text(req, "agent")?;
    let participant = req["participant"].as_str().unwrap_or(agent);
    let mut write = req["write"].as_bool().unwrap_or(false);
    let mut system = String::new();
    let mut execution_id = None;
    let mut plan_slug = None;
    let (state, _, _) = snapshot(&tx)?;
    let c = conversation(&state, conversation_id).ok();
    if let Some(c) = c {
        let b = binding(&tx, repo, conversation_id)?;
        plan_slug = b.plan_slug.clone();
        if let Some(session) = req["session_id"].as_str().filter(|s| !s.is_empty()) {
            let prior:Option<(String,Option<String>)>=tx.query_row("SELECT conversation_id,plan_slug FROM native_session_plan WHERE agent=?1 AND session_id=?2",params![agent,session],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if prior
                .as_ref()
                .is_some_and(|(id, slug)| id != conversation_id || slug != &b.plan_slug)
                || (prior.is_none() && (b.plan_slug.is_some() || b.origin.is_some()))
            {
                return reject(
                    409,
                    "Start a fresh session after changing the conversation or plan",
                );
            }
        }
        if let Some(slug) = b.plan_slug.as_deref() {
            check_version(b.version, version(req, "binding_version")?)?;
            if root.canonicalize()?.to_string_lossy() != b.checkout {
                return reject(
                    409,
                    "Resolve the conversation's plan before changing checkout",
                );
            }
            let p = c["participants"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["name"] == participant && p["agent"] == agent)
                .ok_or(ApiError(409, "Select this conversation's participant"))?;
            if write && p["write"] != true {
                return reject(409, "Participant has no edit permission");
            }
            if req["intent"] == "discussion" {
                write = false;
            } else if req["intent"].as_str().is_some_and(|v| v != "execute") {
                return reject(400, "Intent must be discussion or execute");
            }
            let mut plan = load_plan(&tx, repo, root, slug)?;
            ensure_unedited(repo, &plan)?;
            system = format!(
                "Shared plan: {} ({})\n{}\n\nPLAN.md, TASKS.md and EXECUTION.md are server-owned records. Do not edit them directly. Report changed files and validation evidence in your response; writing files or finishing a response does not complete a task.",
                plan.title, slug, plan.content
            );
            if let Some(origin) = &b.origin {
                system.push_str(&format!("\nHistory before message {} was copied from conversation {} (previous plan: {}). It is historical context, not work performed for the current plan. The current plan governs new executions.",origin.message_count,origin.conversation_id,origin.previous_plan.as_deref().unwrap_or("free conversation")));
            }
            if write {
                let task_id = req["task_id"]
                    .as_str()
                    .or_else(|| b.selections.get(participant).map(String::as_str))
                    .ok_or(ApiError(
                        409,
                        "Select or create a task before writing in a planned conversation",
                    ))?;
                if plan
                    .executions
                    .iter()
                    .any(|e| e.task_id == task_id && e.state == "running")
                {
                    return reject(409, "This task already has a running execution");
                }
                let task = plan
                    .tasks
                    .iter_mut()
                    .find(|t| t.id == task_id && !t.checked && !t.cancelled)
                    .ok_or(ApiError(409, "Select a pending task before writing"))?;
                let execution = id(&tx)?;
                task.status = "running".into();
                task.latest_execution = Some(execution.clone());
                system.push_str(&format!(
                    "\nAssigned task: {} — {}. Work within this task's scope.",
                    task.id, task.title
                ));
                let git = crate::diagnostics::git_context(root)?;
                let record = Execution {
                    id: execution.clone(),
                    task_id: task_id.into(),
                    conversation_id: conversation_id.into(),
                    participant: participant.into(),
                    agent: agent.into(),
                    checkout: git.checkout,
                    branch: git.branch,
                    sha: Some(git.sha),
                    started_at: now(),
                    finished_at: None,
                    state: "running".into(),
                    files: vec![],
                    validation: None,
                    baseline: file_state(root),
                };
                plan.execution_markdown.push_str(&format!("\n## {} · Execution {}\n\n- Participant: {} ({})\n- Conversation: {}\n- Started: {}\n- Branch: {}\n- Checkout: {}\n- State: running\n",task_id,execution,participant,agent,conversation_id,record.started_at,record.branch.as_deref().unwrap_or("detached HEAD"),record.checkout));
                plan.executions.push(record);
                task_note(&mut plan, task_id, participant, "running");
                save_plan(&tx, &mut plan)?;
                execution_id = Some(execution);
            }
        } else if let Some(origin) = &b.origin {
            system = format!(
                "History through message {} comes from conversation {} under plan {}. This is now a free conversation; prior plan work is historical context.",
                origin.message_count,
                origin.conversation_id,
                origin.previous_plan.as_deref().unwrap_or("none")
            );
        }
    } else {
        let managed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_plan_bindings WHERE conversation_id=?1)",
            [conversation_id],
            |r| r.get(0),
        )?;
        if managed {
            return reject(409, "Save and refresh this conversation before executing");
        }
    }
    let token = id(&tx)?;
    if !conversation_id.is_empty() {
        let running:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_plan_turns WHERE conversation_id=?1 AND participant=?2)",params![conversation_id,participant],|r|r.get(0))?;
        if running {
            return reject(409, "Participant already has a running reply");
        }
        tx.execute("INSERT INTO chat_plan_turns(token,conversation_id,participant,owner,execution_id,checkout,plan_slug) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![token,conversation_id,participant,owner(),execution_id,root.to_string_lossy(),plan_slug])?;
        tx.execute(
            "INSERT OR IGNORE INTO chat_plan_turn_history(conversation_id) VALUES (?1)",
            [conversation_id],
        )?;
    }
    tx.commit()?;
    project_pending(db, repo);
    Ok(Turn {
        token,
        execution_id,
        checkout: root.to_string_lossy().into_owned(),
        plan_slug,
        system,
        write,
    })
}
pub fn finish_turn(
    db: &mut Connection,
    repo: &Path,
    token: &str,
    execution: Option<&str>,
    root: &str,
    slug: Option<&str>,
    success: bool,
) -> Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let (Some(execution), Some(slug)) = (execution, slug) {
        // Finalization must release the lease even if the checkout's documents
        // disappeared or became unwritable. Persist the result first; the outbox
        // validates paths and retries projection separately.
        let document: String = tx.query_row(
            "SELECT document FROM chat_plans WHERE checkout=?1 AND slug=?2",
            params![root, slug],
            |r| r.get(0),
        )?;
        let mut p: Plan = serde_json::from_str(&document)?;
        if let Some(e) = p
            .executions
            .iter_mut()
            .find(|e| e.id == execution && e.state == "running")
        {
            let after = file_state(Path::new(root));
            e.files = after
                .keys()
                .chain(e.baseline.keys())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter(|name| after.get(*name) != e.baseline.get(*name))
                .cloned()
                .collect();
            e.state = if success {
                "validation_pending"
            } else {
                "partial"
            }
            .into();
            e.finished_at = Some(now());
            e.baseline.clear();
            if let Some(t) = p.tasks.iter_mut().find(|t| t.id == e.task_id) {
                t.status = e.state.clone();
            }
            p.execution_markdown.push_str(&format!("\n### {} · Result\n\n- Finished: {}\n- State: {}\n- Files observed: {}\n- Validation: pending human confirmation\n",execution,e.finished_at.as_deref().unwrap_or(""),e.state,if e.files.is_empty(){"none".into()}else{e.files.join(", ")}));
            let task_id = e.task_id.clone();
            let participant = e.participant.clone();
            let state = e.state.clone();
            task_note(&mut p, &task_id, &participant, &state);
            save_plan(&tx, &mut p)?;
        }
    }
    tx.execute("DELETE FROM chat_plan_turns WHERE token=?1", [token])?;
    tx.commit()?;
    project_pending(db, repo);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE chat_state(id INTEGER PRIMARY KEY,state_json TEXT,drafts_json TEXT,updated_at INTEGER);").unwrap();
        init(&db).unwrap();
        (dir, db)
    }
    fn save(db: &mut Connection, root: &Path, log: Value) -> u64 {
        save_snapshot(db,root,json!({"convs":[{"id":"c1","title":"Chat","participants":[{"name":"Dev","agent":"codex","write":true,"session":"","seen":0},{"name":"Reviewer","agent":"codex","write":true,"session":"","seen":0}],"log":log}],"current":"c1"}),json!({"c1":"Original draft"}),None).unwrap()
    }
    fn create(db: &mut Connection, root: &Path, slug: &str) -> Value {
        route(
            db,
            root,
            "POST",
            "/api/plans",
            None,
            &json!({"slug":slug,"title":"Shared objective"}),
        )
        .unwrap()
    }
    fn add(db: &mut Connection, root: &Path, slug: &str, v: u64) -> Value {
        route(
            db,
            root,
            "POST",
            &format!("/api/plans/{slug}/tasks"),
            None,
            &json!({"version":v,"title":"Implement feature"}),
        )
        .unwrap()
    }
    fn link(
        db: &mut Connection,
        root: &Path,
        slug: Option<&str>,
        v: u64,
        rev: u64,
        fork: bool,
    ) -> Value {
        route(
            db,
            root,
            "POST",
            "/api/conversations/c1/plan",
            None,
            &json!({"plan_slug":slug,"version":v,"state_revision":rev,"confirm_fork":fork}),
        )
        .unwrap()
    }
    fn turn_req(version: u64, task: Option<&str>) -> Value {
        json!({"conversation_id":"c1","participant":"Dev","agent":"codex","write":true,"binding_version":version,"task_id":task})
    }
    fn status(e: anyhow::Error) -> u16 {
        e.downcast::<ApiError>().unwrap().0
    }

    #[test]
    fn plans_are_shared_and_quick_tasks_preserve_cancelled_markers() {
        let (dir, mut db) = fixture();
        let root = dir.path();
        save(&mut db, root, json!([]));
        let p = create(&mut db, root, "shared");
        assert_eq!(p["version"], 1);
        assert_eq!(p["projection_pending"], false);
        for file in ["PLAN.md", "TASKS.md", "EXECUTION.md"] {
            assert!(root.join(".aic/plans/shared").join(file).is_file());
        }
        let p = add(&mut db, root, "shared", 1);
        assert_eq!(p["tasks"][0]["id"], "T001");
        let b = link(&mut db, root, Some("shared"), 0, 1, false);
        assert_eq!(b["forked"], false);
        let copied = route(
            &mut db,
            root,
            "POST",
            "/api/conversations/c1/fork",
            None,
            &json!({"state_revision":2,"with_history":false}),
        )
        .unwrap();
        assert_eq!(copied["binding"]["plan_slug"], "shared");
        assert_eq!(copied["binding"]["selections"], json!({}));
        let p = add(&mut db, root, "shared", 2);
        assert_eq!(p["tasks"].as_array().unwrap().len(), 2);
        assert_eq!(
            std::fs::read_dir(root.join(".aic/plans")).unwrap().count(),
            1
        );
        let external = root.join(".aic/plans/existing");
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("PLAN.md"), "# Existing\n").unwrap();
        std::fs::write(
            external.join("TASKS.md"),
            "# Tasks\n- [*] **T009 — Replaced**\n- [ ] **B500 — Backend:** detail\n",
        )
        .unwrap();
        let p = route(
            &mut db,
            root,
            "GET",
            "/api/plans/existing",
            None,
            &json!({}),
        )
        .unwrap();
        assert_eq!(p["tasks"][0]["cancelled"], true);
        assert_eq!(p["tasks"][1]["id"], "B500");
        let p = add(&mut db, root, "existing", 1);
        assert!(p["tasks_markdown"].as_str().unwrap().contains("[*] **T009"));
    }

    #[test]
    fn plan_change_forks_history_keeps_original_draft_and_requires_confirmation() {
        let (dir, mut db) = fixture();
        let root = dir.path();
        save(
            &mut db,
            root,
            json!([{ "from":"you","text":"old objective" }]),
        );
        create(&mut db, root, "new");
        assert_eq!(
            status(
                route(
                    &mut db,
                    root,
                    "POST",
                    "/api/conversations/c1/plan",
                    None,
                    &json!({"version":0,"state_revision":1,"plan_slug":"new"})
                )
                .unwrap_err()
            ),
            409
        );
        let changed = link(&mut db, root, Some("new"), 0, 1, true);
        assert_eq!(changed["forked"], true);
        assert_eq!(changed["binding"]["origin"]["previous_plan"], Value::Null);
        assert_eq!(changed["binding"]["origin"]["message_count"], 1);
        assert_eq!(changed["conversation"]["participants"][0]["session"], "");
        let (state, drafts, _) = snapshot(&db).unwrap();
        assert_eq!(drafts["c1"], "Original draft");
        assert_eq!(
            conversation(&state, "c1").unwrap()["log"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(binding(&db, root, "c1").unwrap().plan_slug, None);
        let fork = changed["conversation"]["id"].as_str().unwrap();
        let undo = route(
            &mut db,
            root,
            "POST",
            &format!("/api/conversations/{fork}/plan/undo"),
            None,
            &json!({"version":1,"state_revision":2}),
        )
        .unwrap();
        assert_eq!(undo["undone"], true);
        assert_eq!(
            snapshot(&db).unwrap().0["convs"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn planned_writes_require_a_task_and_discussion_is_read_only() {
        let (dir, mut db) = fixture();
        let root = dir.path();
        save(&mut db, root, json!([]));
        create(&mut db, root, "feature");
        add(&mut db, root, "feature", 1);
        link(&mut db, root, Some("feature"), 0, 1, false);
        assert_eq!(
            status(
                begin_turn(&mut db, root, root, &turn_req(1, None))
                    .err()
                    .unwrap()
            ),
            409
        );
        let mut discussion = turn_req(1, None);
        discussion["intent"] = json!("discussion");
        let t = begin_turn(&mut db, root, root, &discussion).unwrap();
        assert!(!t.write);
        assert!(t.execution_id.is_none());
        assert_eq!(
            status(
                route(
                    &mut db,
                    root,
                    "POST",
                    "/api/conversations/c1/plan",
                    None,
                    &json!({"version":1,"state_revision":2,"plan_slug":null})
                )
                .unwrap_err()
            ),
            409
        );
        finish_turn(
            &mut db,
            root,
            &t.token,
            None,
            &t.checkout,
            t.plan_slug.as_deref(),
            true,
        )
        .unwrap();
        let t = begin_turn(&mut db, root, root, &turn_req(1, Some("T001"))).unwrap();
        assert!(t.write);
        assert!(t.system.contains("Assigned task: T001"));
        let mut other = turn_req(1, Some("T001"));
        other["participant"] = json!("Reviewer");
        assert_eq!(
            status(begin_turn(&mut db, root, root, &other).err().unwrap()),
            409
        );
        std::fs::write(root.join("new.rs"), "new code").unwrap();
        finish_turn(
            &mut db,
            root,
            &t.token,
            t.execution_id.as_deref(),
            &t.checkout,
            t.plan_slug.as_deref(),
            true,
        )
        .unwrap();
        let p = load_plan(&db, root, root, "feature").unwrap();
        assert!(!p.tasks[0].checked);
        assert_eq!(p.tasks[0].status, "validation_pending");
        assert_eq!(p.executions[0].files, vec!["new.rs"]);
        assert!(p.tasks_markdown.contains("- [ ] T001"));
        assert!(p.tasks_markdown.contains("Dev · validation_pending"));
        assert_eq!(
            status(
                route(
                    &mut db,
                    root,
                    "POST",
                    "/api/plans/feature/tasks/T001/complete",
                    None,
                    &json!({"version":p.version,"validation":"Agent claims tests passed"})
                )
                .unwrap_err()
            ),
            400
        );
        let p=route(&mut db,root,"POST","/api/plans/feature/tasks/T001/complete",None,&json!({"version":p.version,"human_confirmed":true,"validation":"Reviewed test output: 8 passed"})).unwrap();
        assert_eq!(p["tasks"][0]["checked"], true);
        assert_eq!(p["executions"][0]["state"], "completed");
        assert!(p["tasks_markdown"].as_str().unwrap().contains("- [x] T001"));
        assert!(
            p["execution_markdown"]
                .as_str()
                .unwrap()
                .contains("8 passed")
        );
    }

    #[test]
    fn revisions_prevent_stale_snapshots_and_binding_injection() {
        let (dir, mut db) = fixture();
        let root = dir.path();
        save(&mut db, root, json!([]));
        create(&mut db, root, "feature");
        link(&mut db, root, Some("feature"), 0, 1, false);
        let (mut s, d, rev) = snapshot(&db).unwrap();
        assert_eq!(
            status(save_snapshot(&mut db, root, s.clone(), d.clone(), Some(rev - 1)).unwrap_err()),
            409
        );
        assert_eq!(
            status(save_snapshot(&mut db, root, s.clone(), d.clone(), None).unwrap_err()),
            409
        );
        s["convs"][0]["plan_binding"] = json!({"plan_slug":null});
        save_snapshot(&mut db, root, s, d, Some(rev)).unwrap();
        assert_eq!(
            snapshot(&db).unwrap().0["convs"][0]["plan_binding"]["plan_slug"],
            "feature"
        );
        assert_eq!(
            status(add_result(&mut db, root, "feature", 0).unwrap_err()),
            409
        );
    }
    fn add_result(db: &mut Connection, root: &Path, slug: &str, v: u64) -> Result<Value> {
        route(
            db,
            root,
            "POST",
            &format!("/api/plans/{slug}/tasks"),
            None,
            &json!({"version":v,"title":"Task"}),
        )
    }

    #[test]
    fn failed_turns_and_dead_process_recovery_never_check_tasks() {
        let (dir, mut db) = fixture();
        let root = dir.path();
        save(&mut db, root, json!([]));
        create(&mut db, root, "feature");
        add(&mut db, root, "feature", 1);
        link(&mut db, root, Some("feature"), 0, 1, false);
        let t = begin_turn(&mut db, root, root, &turn_req(1, Some("T001"))).unwrap();
        db.execute(
            "UPDATE chat_plan_turns SET owner='99999999:missing' WHERE token=?1",
            [&t.token],
        )
        .unwrap();
        recover(&mut db, root).unwrap();
        let p = load_plan(&db, root, root, "feature").unwrap();
        assert!(!p.tasks[0].checked);
        assert_eq!(p.tasks[0].status, "partial");
        assert!(!busy(&db, "c1").unwrap());
        let next = begin_turn(&mut db, root, root, &turn_req(1, Some("T001"))).unwrap();
        finish_turn(
            &mut db,
            root,
            &next.token,
            next.execution_id.as_deref(),
            &next.checkout,
            next.plan_slug.as_deref(),
            false,
        )
        .unwrap();
        assert_eq!(
            load_plan(&db, root, root, "feature")
                .unwrap()
                .executions
                .len(),
            2
        );
    }

    #[test]
    fn checkout_changes_require_resolution_and_failed_confirmation_has_no_files() {
        let (dir, mut db) = fixture();
        let root = dir.path();
        Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ])
            .current_dir(root)
            .status()
            .unwrap();
        let other = root.join("linked");
        Command::new("git")
            .args(["worktree", "add", "-qb", "linked"])
            .arg(&other)
            .current_dir(root)
            .status()
            .unwrap();
        save(&mut db, root, json!([]));
        create(&mut db, root, "feature");
        add(&mut db, root, "feature", 1);
        link(&mut db, root, Some("feature"), 0, 1, false);
        let (mut s, d, rev) = snapshot(&db).unwrap();
        s["convs"][0]["checkout"] = json!(other);
        assert_eq!(
            status(save_snapshot(&mut db, root, s, d, Some(rev)).unwrap_err()),
            409
        );
        let request =
            json!({"version":1,"state_revision":2,"checkout":other,"plan_action":"migrate"});
        let changed = route(
            &mut db,
            root,
            "POST",
            "/api/conversations/c1/checkout",
            None,
            &request,
        )
        .unwrap();
        assert_eq!(changed["forked"], false);
        assert!(root.join(".aic/plans/feature/PLAN.md").exists());
        assert!(other.join(".aic/plans/feature/TASKS.md").exists());
        let p = load_plan(&db, root, &other, "feature").unwrap();
        assert_eq!(p.tasks.len(), 1);
        assert!(p.execution_markdown.contains("Migrated copy"));
    }

    #[test]
    fn external_edits_require_reload_and_symlink_paths_are_rejected() {
        use std::os::unix::fs::symlink;
        let (dir, mut db) = fixture();
        let root = dir.path();
        create(&mut db, root, "feature");
        let tasks = root.join(".aic/plans/feature/TASKS.md");
        std::fs::write(&tasks, "# Tasks\n- [*] T090 Cancelled\n").unwrap();
        assert_eq!(
            status(add_result(&mut db, root, "feature", 1).unwrap_err()),
            409
        );
        let p = route(
            &mut db,
            root,
            "POST",
            "/api/plans/feature/reload",
            None,
            &json!({"version":1}),
        )
        .unwrap();
        assert_eq!(p["tasks"][0]["cancelled"], true);
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.join(".aic/plans/escape")).unwrap();
        assert!(route(&mut db, root, "GET", "/api/plans/escape", None, &json!({})).is_err());
        assert!(
            route(
                &mut db,
                root,
                "POST",
                "/api/plans",
                None,
                &json!({"slug":"../escape","title":"bad"})
            )
            .is_err()
        );
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[test]
    fn outbox_retries_without_writing_through_symlinks() {
        use std::os::unix::fs::symlink;
        let (dir, mut db) = fixture();
        let root = dir.path();
        save(&mut db, root, json!([]));
        create(&mut db, root, "feature");
        add(&mut db, root, "feature", 1);
        link(&mut db, root, Some("feature"), 0, 1, false);
        let t = begin_turn(&mut db, root, root, &turn_req(1, Some("T001"))).unwrap();
        let mut p = load_plan(&db, root, root, "feature").unwrap();
        p.content.push_str("\nUpdated objective\n");
        save_plan(&db, &mut p).unwrap();
        let path = root.join(".aic/plans/feature");
        let old = root.join(".aic/plans/held");
        std::fs::rename(&path, &old).unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), &path).unwrap();
        finish_turn(
            &mut db,
            root,
            &t.token,
            t.execution_id.as_deref(),
            &t.checkout,
            t.plan_slug.as_deref(),
            false,
        )
        .unwrap();
        assert!(
            !busy(&db, "c1").unwrap(),
            "projection failure must not leave a running turn"
        );
        project_pending(&mut db, root);
        let dirty: bool = db
            .query_row(
                "SELECT dirty FROM chat_plans WHERE slug='feature'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(dirty);
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(old, &path).unwrap();
        project_pending(&mut db, root);
        let p = load_plan(&db, root, root, "feature").unwrap();
        assert!(!p.projection_pending);
        assert_eq!(p.executions[0].state, "partial");
        assert!(!p.tasks[0].checked);
        assert!(
            std::fs::read_to_string(path.join("PLAN.md"))
                .unwrap()
                .contains("Updated objective")
        );
    }

    #[test]
    fn failed_checkout_fork_confirmation_rolls_back_migration() {
        let (dir, mut db) = fixture();
        let root = dir.path();
        Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ])
            .current_dir(root)
            .status()
            .unwrap();
        let other = root.join("linked");
        Command::new("git")
            .args(["worktree", "add", "-qb", "linked"])
            .arg(&other)
            .current_dir(root)
            .status()
            .unwrap();
        save(&mut db, root, json!([]));
        create(&mut db, root, "feature");
        link(&mut db, root, Some("feature"), 0, 1, false);
        let (mut s, d, rev) = snapshot(&db).unwrap();
        s["convs"][0]["log"] = json!([{ "from":"you","text":"history" }]);
        save_snapshot(&mut db, root, s, d, Some(rev)).unwrap();
        let request =
            json!({"version":1,"state_revision":3,"checkout":other,"plan_action":"migrate"});
        assert_eq!(
            status(
                route(
                    &mut db,
                    root,
                    "POST",
                    "/api/conversations/c1/checkout",
                    None,
                    &request
                )
                .unwrap_err()
            ),
            409
        );
        assert!(!other.join(".aic/plans/feature").exists());
        assert_eq!(
            binding(&db, root, "c1").unwrap().checkout,
            root.to_string_lossy()
        );
    }

    #[test]
    fn snapshot_free_writes_remain_compatible_and_undo_retains_executed_forks() {
        let (dir, mut db) = fixture();
        let root = dir.path();
        save(&mut db, root, json!([]));
        let t = begin_turn(&mut db, root, root, &turn_req(0, None)).unwrap();
        assert!(t.write);
        assert!(t.execution_id.is_none());
        finish_turn(&mut db, root, &t.token, None, &t.checkout, None, true).unwrap();
        let copy = route(
            &mut db,
            root,
            "POST",
            "/api/conversations/c1/fork",
            None,
            &json!({"state_revision":1}),
        )
        .unwrap();
        let fork = copy["conversation"]["id"].as_str().unwrap();
        let mut req = turn_req(1, None);
        req["conversation_id"] = json!(fork);
        let t = begin_turn(&mut db, root, root, &req).unwrap();
        finish_turn(&mut db, root, &t.token, None, &t.checkout, None, true).unwrap();
        let undo = route(
            &mut db,
            root,
            "POST",
            &format!("/api/conversations/{fork}/plan/undo"),
            None,
            &json!({"version":1,"state_revision":2}),
        )
        .unwrap();
        assert_eq!(undo["undone"], false);
        assert_eq!(
            snapshot(&db).unwrap().0["convs"].as_array().unwrap().len(),
            2
        );
    }
}
