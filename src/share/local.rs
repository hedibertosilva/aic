use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct ConfigureRequest {
    pub relay_url: String,
    pub publisher_token: String,
}

#[derive(Deserialize)]
pub struct ShareRequest {
    #[serde(default)]
    pub name: String,
    pub conversation_ids: Vec<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

struct RelayConfig {
    url: String,
    token: String,
}

pub fn init(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS share_settings (
           id INTEGER PRIMARY KEY CHECK(id=1), relay_url TEXT NOT NULL,
           publisher_token TEXT NOT NULL, updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS local_shares (
           id TEXT PRIMARY KEY, name TEXT NOT NULL, public_url TEXT NOT NULL,
           conversation_ids_json TEXT NOT NULL, expires_at INTEGER,
           status TEXT NOT NULL DEFAULT 'active', last_synced_at INTEGER,
           last_error TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );",
    )?;
    Ok(())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn config(db: &Connection) -> Result<RelayConfig> {
    db.query_row(
        "SELECT relay_url,publisher_token FROM share_settings WHERE id=1",
        [],
        |row| {
            Ok(RelayConfig {
                url: row.get(0)?,
                token: row.get(1)?,
            })
        },
    )
    .optional()?
    .context("Configure a sharing server first")
}

fn normalize_url(value: &str) -> Result<String> {
    let value = value.trim().trim_end_matches('/');
    if !value.starts_with("https://")
        && !value.starts_with("http://localhost")
        && !value.starts_with("http://127.0.0.1")
    {
        bail!("Sharing server URL must use HTTPS");
    }
    if value.contains(['\n', '\r']) {
        bail!("Invalid sharing server URL");
    }
    Ok(value.to_string())
}

fn agent() -> Result<ureq::Agent> {
    Ok(ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .build()
        .into())
}

fn send_json(
    method: &str,
    url: &str,
    token: &str,
    value: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>> {
    let client = agent()?;
    let authorization = format!("Bearer {token}");
    let mut response = match (method, value) {
        ("POST", Some(body)) => client
            .post(url)
            .header("Authorization", &authorization)
            .send_json(body),
        ("POST", None) => client
            .post(url)
            .header("Authorization", &authorization)
            .send_empty(),
        ("PUT", Some(body)) => client
            .put(url)
            .header("Authorization", &authorization)
            .send_json(body),
        ("DELETE", None) => client
            .delete(url)
            .header("Authorization", &authorization)
            .call(),
        ("GET", None) => client
            .get(url)
            .header("Authorization", &authorization)
            .call(),
        _ => bail!("unsupported relay method/body combination"),
    }
    .with_context(|| format!("sharing relay request failed: {method} {url}"))?;
    if response.status() == 204 {
        return Ok(None);
    }
    Ok(Some(
        response
            .body_mut()
            .read_json()
            .context("reading sharing relay response")?,
    ))
}

pub fn configure(db: &Connection, request: ConfigureRequest) -> Result<serde_json::Value> {
    let relay_url = normalize_url(&request.relay_url)?;
    if request.publisher_token.trim().len() < 32 {
        bail!("Publisher token is invalid");
    }
    send_json(
        "GET",
        &format!("{relay_url}/api/publisher/shares"),
        request.publisher_token.trim(),
        None,
    )
    .context("Could not authenticate with the sharing server")?;
    db.execute(
        "INSERT INTO share_settings(id,relay_url,publisher_token,updated_at) VALUES(1,?1,?2,?3)
         ON CONFLICT(id) DO UPDATE SET relay_url=excluded.relay_url,publisher_token=excluded.publisher_token,updated_at=excluded.updated_at",
        params![relay_url,request.publisher_token.trim(),now()])?;
    Ok(serde_json::json!({"configured":true,"relay_url":relay_url}))
}

pub fn list(db: &Connection) -> Result<serde_json::Value> {
    let configured: Option<String> = db
        .query_row(
            "SELECT relay_url FROM share_settings WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let mut statement = db.prepare("SELECT id,name,public_url,conversation_ids_json,expires_at,status,last_synced_at,last_error,created_at,updated_at FROM local_shares WHERE status!='revoked' ORDER BY created_at DESC")?;
    let shares = statement.query_map([], |row| {
        let ids: String = row.get(3)?;
        Ok(serde_json::json!({
            "shared_id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,"url":row.get::<_,String>(2)?,
            "conversation_ids":serde_json::from_str::<serde_json::Value>(&ids).unwrap_or_else(|_|serde_json::json!([])),
            "expires_at":row.get::<_,Option<i64>>(4)?,"status":row.get::<_,String>(5)?,
            "last_synced_at":row.get::<_,Option<i64>>(6)?,"last_error":row.get::<_,Option<String>>(7)?,
            "created_at":row.get::<_,i64>(8)?,"updated_at":row.get::<_,i64>(9)?
        }))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(
        serde_json::json!({"configured":configured.is_some(),"relay_url":configured,"shares":shares}),
    )
}

fn state(db: &Connection) -> Result<serde_json::Value> {
    let raw: String = db
        .query_row("SELECT state_json FROM chat_state WHERE id=1", [], |row| {
            row.get(0)
        })
        .context("No conversations have been saved yet")?;
    Ok(serde_json::from_str(&raw)?)
}

fn public_conversation(conversation: &serde_json::Value) -> serde_json::Value {
    let participants = conversation.get("participants").and_then(|value| value.as_array()).into_iter().flatten().map(|participant| serde_json::json!({
        "name":participant.get("name").and_then(|value|value.as_str()).unwrap_or("Participant"),
        "role":participant.get("role").and_then(|value|value.as_str()).unwrap_or(""),
        "agent":participant.get("agent").and_then(|value|value.as_str()).unwrap_or(""),
        "inactive":participant.get("inactive").and_then(|value|value.as_bool()).unwrap_or(false)
    })).collect::<Vec<_>>();
    let allowed = [
        "from", "text", "round", "t0", "to", "edited", "error", "stopped",
    ];
    let log = conversation
        .get("log")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|entry| {
            !entry
                .get("pending")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        })
        .map(|entry| {
            let mut public = serde_json::Map::new();
            for key in allowed {
                if let Some(value) = entry.get(key) {
                    public.insert(key.into(), value.clone());
                }
            }
            serde_json::Value::Object(public)
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "id":conversation.get("id").and_then(|value|value.as_str()).unwrap_or(""),
        "title":conversation.get("title").and_then(|value|value.as_str()).unwrap_or("Conversation"),
        "updated":conversation.get("updated").and_then(|value|value.as_i64()),
        "participants":participants,"log":log
    })
}

pub fn project(source: &serde_json::Value, ids: &[String]) -> Result<serde_json::Value> {
    if ids.is_empty() {
        bail!("Select at least one conversation");
    }
    let conversations = source
        .get("convs")
        .and_then(|value| value.as_array())
        .context("Invalid chat state")?;
    let mut selected = Vec::with_capacity(ids.len());
    for id in ids {
        let conversation = conversations
            .iter()
            .find(|item| item.get("id").and_then(|value| value.as_str()) == Some(id.as_str()))
            .with_context(|| format!("Conversation {id} was not found"))?;
        selected.push(public_conversation(conversation));
    }
    Ok(serde_json::json!({"conversations":selected}))
}

pub fn create(db: &Connection, request: ShareRequest) -> Result<serde_json::Value> {
    let relay = config(db)?;
    let snapshot = project(&state(db)?, &request.conversation_ids)?;
    let body = serde_json::json!({"name":request.name,"expires_at":request.expires_at,"snapshot":snapshot});
    let result = send_json(
        "POST",
        &format!("{}/api/publisher/shares", relay.url),
        &relay.token,
        Some(&body),
    )?
    .context("Sharing server returned no data")?;
    let id = result
        .get("shared_id")
        .and_then(|value| value.as_str())
        .context("Sharing server returned no shared_id")?;
    let public_url = result
        .get("url")
        .and_then(|value| value.as_str())
        .context("Sharing server returned no URL")?;
    let timestamp = now();
    db.execute("INSERT INTO local_shares(id,name,public_url,conversation_ids_json,expires_at,last_synced_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6,?6)",
        params![id,request.name,public_url,serde_json::to_string(&request.conversation_ids)?,request.expires_at,timestamp])?;
    Ok(result)
}

pub fn update(db: &Connection, id: &str, request: ShareRequest) -> Result<serde_json::Value> {
    let relay = config(db)?;
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM local_shares WHERE id=?1 AND status='active')",
        [id],
        |row| row.get(0),
    )?;
    if !exists {
        bail!("Share not found");
    }
    let snapshot = project(&state(db)?, &request.conversation_ids)?;
    let body = serde_json::json!({"name":request.name,"expires_at":request.expires_at,"snapshot":snapshot});
    send_json(
        "PUT",
        &format!("{}/api/publisher/shares/{id}", relay.url),
        &relay.token,
        Some(&body),
    )?;
    let timestamp = now();
    db.execute("UPDATE local_shares SET name=?2,conversation_ids_json=?3,expires_at=?4,last_synced_at=?5,last_error=NULL,updated_at=?5 WHERE id=?1",
        params![id,request.name,serde_json::to_string(&request.conversation_ids)?,request.expires_at,timestamp])?;
    Ok(serde_json::json!({"updated":true,"last_synced_at":timestamp}))
}

pub fn rotate_password(db: &Connection, id: &str) -> Result<serde_json::Value> {
    let relay = config(db)?;
    send_json(
        "POST",
        &format!("{}/api/publisher/shares/{id}/rotate-password", relay.url),
        &relay.token,
        None,
    )?
    .context("Sharing server returned no password")
}

pub fn revoke(db: &Connection, id: &str) -> Result<()> {
    let relay = config(db)?;
    send_json(
        "DELETE",
        &format!("{}/api/publisher/shares/{id}", relay.url),
        &relay.token,
        None,
    )?;
    db.execute(
        "UPDATE local_shares SET status='revoked',updated_at=?2 WHERE id=?1",
        params![id, now()],
    )?;
    Ok(())
}

pub fn sync_all(db: &Connection, source: &serde_json::Value) -> Result<()> {
    let Ok(relay) = config(db) else { return Ok(()) };
    let mut statement = db.prepare(
        "SELECT id,name,conversation_ids_json,expires_at FROM local_shares WHERE status='active'",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (id, name, ids_json, expires_at) in rows {
        let result = (|| -> Result<()> {
            let ids: Vec<String> = serde_json::from_str(&ids_json)?;
            let snapshot = project(source, &ids)?;
            let body = serde_json::json!({"name":name,"expires_at":expires_at,"snapshot":snapshot});
            send_json(
                "PUT",
                &format!("{}/api/publisher/shares/{id}", relay.url),
                &relay.token,
                Some(&body),
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                db.execute("UPDATE local_shares SET last_synced_at=?2,last_error=NULL,updated_at=?2 WHERE id=?1",params![id,now()])?;
            }
            Err(error) => {
                db.execute(
                    "UPDATE local_shares SET last_error=?2,updated_at=?3 WHERE id=?1",
                    params![id, error.to_string(), now()],
                )?;
            }
        }
    }
    Ok(())
}

static SYNC_PENDING: OnceLock<Mutex<Option<(PathBuf, serde_json::Value)>>> = OnceLock::new();
static SYNC_WORKER: AtomicBool = AtomicBool::new(false);

/// Coalesces rapid browser saves and keeps relay I/O off the local chat
/// request path. The last queued state always wins.
pub fn queue_sync(repo_root: &Path, source: serde_json::Value) {
    *SYNC_PENDING
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("share sync queue poisoned") = Some((repo_root.to_path_buf(), source));
    if SYNC_WORKER
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    std::thread::spawn(|| {
        loop {
            let pending = SYNC_PENDING
                .get()
                .expect("share sync queue initialized")
                .lock()
                .expect("share sync queue poisoned")
                .take();
            let Some((root, state)) = pending else {
                SYNC_WORKER.store(false, Ordering::Release);
                // Close the race where a producer queued after the take but just
                // before the worker flag changed.
                if SYNC_PENDING
                    .get()
                    .expect("share sync queue initialized")
                    .lock()
                    .expect("share sync queue poisoned")
                    .is_some()
                    && SYNC_WORKER
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    continue;
                }
                break;
            };
            let path = root.join(".aic").join("chat").join("chat.sqlite3");
            match Connection::open(&path).and_then(|db| {
                let _ = db.busy_timeout(Duration::from_secs(5));
                Ok(db)
            }) {
                Ok(db) => {
                    if let Err(error) = sync_all(&db, &state) {
                        eprintln!("syncing shared conversations failed: {error:#}");
                    }
                }
                Err(error) => eprintln!("opening chat database for share sync failed: {error}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_messages_but_removes_private_runtime_fields() {
        let source = serde_json::json!({"convs":[{"id":"one","title":"Hello","checkout":"/secret","participants":[{"name":"Backend","agent":"codex","role":"Dev","session":"secret","write":true}],"log":[{"from":"you","text":"hi","t0":1,"pasted":["secret"]},{"from":"Backend","text":"hello","actions":["Read /secret"],"session":"secret"}]}]});
        let projected = project(&source, &["one".into()]).unwrap();
        let text = projected.to_string();
        assert!(text.contains("hello"));
        assert!(!text.contains("/secret"));
        assert!(!text.contains("session"));
        assert!(!text.contains("actions"));
        assert!(!text.contains("write"));
        assert!(!text.contains("pasted"));
    }
}
