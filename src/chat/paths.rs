//! Additional host directories granted to one chat conversation.
//!
//! These grants are server-owned because they expand the container's view of
//! the host. They intentionally do not live in the browser-authored chat JSON.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use serde_json::{json, Value};

use crate::chat::plans::{self, ApiError};
use crate::sandbox::{Access, AdditionalPath};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationPath {
    pub path: String,
    pub access: &'static str,
}

pub fn init(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversation_paths (
            conversation_id TEXT NOT NULL,
            path TEXT NOT NULL,
            access TEXT NOT NULL CHECK (access IN ('ro','rw')),
            created_at INTEGER NOT NULL,
            PRIMARY KEY (conversation_id, path)
        );
        CREATE INDEX IF NOT EXISTS conversation_paths_conversation
            ON conversation_paths(conversation_id);",
    )?;
    Ok(())
}

fn reject<T>(status: u16, message: &'static str) -> Result<T> {
    Err(ApiError(status, message).into())
}

fn conversation_exists(db: &Connection, repo: &Path, id: &str) -> Result<()> {
    plans::binding(db, repo, id).map(|_| ())
}

fn busy(db: &Connection, id: &str) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM chat_plan_turns WHERE conversation_id=?1
             UNION ALL
             SELECT 1 FROM chat_runs
              WHERE conversation_id=?1 AND state IN ('running','stopping')
         )",
        [id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn ensure_mutable(db: &Connection, repo: &Path, id: &str) -> Result<PathBuf> {
    let binding = plans::binding(db, repo, id)?;
    if busy(db, id)? {
        return reject(
            409,
            "Wait for this conversation's running replies before changing its paths",
        );
    }
    PathBuf::from(binding.checkout)
        .canonicalize()
        .map_err(|_| ApiError(409, "The conversation checkout is unavailable").into())
}

fn sensitive(path: &Path) -> bool {
    if path == Path::new("/")
        || ["/proc", "/sys", "/dev", "/run"]
            .iter()
            .any(|root| path.starts_with(root))
        || matches!(
            path.to_str(),
            Some("/tmp") | Some("/etc/aic") | Some("/usr/local/bin/aic")
        )
    {
        return true;
    }
    let Ok(home) = std::env::var("HOME") else { return false };
    let home = PathBuf::from(home);
    path == home
        || [".ssh", ".gnupg", ".aws", ".config"]
            .iter()
            .any(|name| path.starts_with(home.join(name)))
        || path.starts_with(home.join(".local/share/aic/sandbox"))
}

fn canonical_path(raw: &str, checkout: &Path) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains('\0') {
        return reject(400, "Choose a nonempty directory path");
    }
    let requested = Path::new(raw);
    let requested = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        checkout.join(requested)
    };
    let canonical = requested
        .canonicalize()
        .map_err(|_| ApiError(400, "The path does not exist or cannot be resolved"))?;
    if canonical.to_str().is_none_or(|path| path.contains(':')) {
        return reject(400, "The path must be valid UTF-8 and cannot contain a colon");
    }
    if !canonical.is_dir() {
        return reject(400, "Only directories can be added to a conversation");
    }
    if sensitive(&canonical) {
        return reject(403, "This path is too broad or contains protected credentials");
    }
    if canonical == checkout || canonical.starts_with(checkout) || checkout.starts_with(&canonical)
    {
        return reject(409, "The path overlaps the conversation's primary checkout");
    }
    Ok(canonical)
}

fn under_allowed_root(path: &Path, repo: &Path, configured: &[String]) -> bool {
    let default = repo
        .canonicalize()
        .ok()
        .and_then(|repo| repo.parent().map(Path::to_path_buf));
    default.as_ref().is_some_and(|root| path.starts_with(root))
        || configured.iter().any(|root| {
            let root = Path::new(root);
            root.is_absolute()
                && root
                    .canonicalize()
                .is_ok_and(|root| path.starts_with(root))
        })
}

pub fn list(db: &Connection, repo: &Path, id: &str) -> Result<Vec<ConversationPath>> {
    conversation_exists(db, repo, id)?;
    let mut statement = db.prepare(
        "SELECT path,access FROM conversation_paths
         WHERE conversation_id=?1 ORDER BY path",
    )?;
    let rows = statement.query_map([id], |row| {
        let path: String = row.get(0)?;
        let access: String = row.get(1)?;
        Ok(ConversationPath {
            path,
            access: if access == "rw" { "rw" } else { "ro" },
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn add(
    db: &Connection,
    repo: &Path,
    id: &str,
    req: &Value,
    configured_roots: &[String],
) -> Result<Value> {
    let checkout = ensure_mutable(db, repo, id)?;
    let raw = req["path"]
        .as_str()
        .ok_or(ApiError(400, "Missing path"))?;
    let access = req["access"].as_str().unwrap_or("ro");
    if !matches!(access, "ro" | "rw") {
        return reject(400, "Path access must be ro or rw");
    }
    if access == "rw" && req["confirmed"].as_bool() != Some(true) {
        return reject(409, "Writable path access requires explicit confirmation");
    }
    let path = canonical_path(raw, &checkout)?;
    if !under_allowed_root(&path, repo, configured_roots) {
        return reject(
            403,
            "Path is outside the allowed roots; configure [sandbox] path_roots",
        );
    }
    let path = path.to_string_lossy().into_owned();
    db.execute(
        "INSERT INTO conversation_paths(conversation_id,path,access,created_at)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(conversation_id,path) DO UPDATE SET access=excluded.access",
        params![id, path, access, chrono::Utc::now().timestamp_millis()],
    )?;
    Ok(json!({"path":path,"access":access}))
}

pub fn remove(db: &Connection, repo: &Path, id: &str, req: &Value) -> Result<Value> {
    let checkout = ensure_mutable(db, repo, id)?;
    let raw = req["path"]
        .as_str()
        .ok_or(ApiError(400, "Missing path"))?;
    let stored: Option<String> = db
        .query_row(
            "SELECT path FROM conversation_paths WHERE conversation_id=?1 AND path=?2",
            params![id, raw.trim()],
            |row| row.get(0),
        )
        .optional()?;
    let path = match stored {
        Some(path) => path,
        None => canonical_path(raw, &checkout)?.to_string_lossy().into_owned(),
    };
    let changed = db.execute(
        "DELETE FROM conversation_paths WHERE conversation_id=?1 AND path=?2",
        params![id, path],
    )?;
    if changed == 0 {
        return reject(404, "Path is not mounted in this conversation");
    }
    Ok(json!({"path":path,"removed":true}))
}

/// Effective grants for an invocation. Read-only participants never receive a
/// writable bind, regardless of the conversation-level request.
pub fn effective(
    db: &Connection,
    id: &str,
    participant_writes: bool,
    repo: &Path,
    configured_roots: &[String],
) -> Result<Vec<AdditionalPath>> {
    let mut statement = db.prepare(
        "SELECT path,access FROM conversation_paths
         WHERE conversation_id=?1 ORDER BY path",
    )?;
    let rows = statement.query_map([id], |row| {
        let path: String = row.get(0)?;
        let access: String = row.get(1)?;
        Ok(AdditionalPath {
            path: PathBuf::from(path),
            access: if participant_writes && access == "rw" {
                Access::ReadWrite
            } else {
                Access::ReadOnly
            },
        })
    })?;
    let mut paths = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    for grant in &mut paths {
        let canonical = grant
            .path
            .canonicalize()
            .map_err(|_| ApiError(409, "An additional conversation path is no longer available"))?;
        if canonical != grant.path
            || !canonical.is_dir()
            || sensitive(&canonical)
            || !under_allowed_root(&canonical, repo, configured_roots)
        {
            return reject(
                409,
                "An additional conversation path changed or is no longer allowed",
            );
        }
        grant.path = canonical;
    }

    // Linked worktrees keep their common Git directory outside the checkout.
    // Bind it with the same effective access so normal Git commands work.
    let visible = paths.clone();
    for grant in visible {
        let Ok(git_dir) = crate::git::git_common_dir(&grant.path) else {
            continue;
        };
        if git_dir.starts_with(&grant.path) {
            continue;
        }
        if let Some(existing) = paths.iter_mut().find(|path| path.path == git_dir) {
            if grant.access == Access::ReadWrite {
                existing.access = Access::ReadWrite;
            }
        } else {
            paths.push(AdditionalPath {
                path: git_dir,
                access: grant.access,
            });
        }
    }
    Ok(paths)
}

pub fn copy(tx: &Transaction<'_>, source: &str, target: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO conversation_paths(conversation_id,path,access,created_at)
         SELECT ?2,path,access,created_at FROM conversation_paths WHERE conversation_id=?1",
        params![source, target],
    )?;
    Ok(())
}

/// A checkout selected as the conversation's primary workspace must not also
/// be represented by a nested/parent bind. It is already mounted with the
/// participant's effective access.
pub fn remove_overlapping(tx: &Transaction<'_>, id: &str, checkout: &Path) -> Result<()> {
    let mut statement = tx.prepare(
        "SELECT path FROM conversation_paths WHERE conversation_id=?1",
    )?;
    let paths = statement
        .query_map([id], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    for value in paths {
        let path = Path::new(&value);
        if path.starts_with(checkout) || checkout.starts_with(path) {
            tx.execute(
                "DELETE FROM conversation_paths WHERE conversation_id=?1 AND path=?2",
                params![id, value],
            )?;
        }
    }
    Ok(())
}

pub fn delete(tx: &Transaction<'_>, id: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM conversation_paths WHERE conversation_id=?1",
        [id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        init(&db).unwrap();
        db.execute_batch(
            "CREATE TABLE chat_plan_turns (conversation_id TEXT);
             CREATE TABLE chat_runs (conversation_id TEXT, state TEXT);",
        )
        .unwrap();
        db
    }

    #[test]
    fn sensitive_host_paths_are_rejected() {
        assert!(sensitive(Path::new("/")));
        assert!(sensitive(Path::new("/proc")));
    }

    #[test]
    fn canonical_path_rejects_primary_checkout_and_accepts_sibling() {
        let temp = tempdir().unwrap();
        let elsewhere = tempdir().unwrap();
        let checkout = temp.path().join("main");
        let sibling = temp.path().join("other");
        let external = elsewhere.path().join("external");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        let checkout = checkout.canonicalize().unwrap();
        let sibling = sibling.canonicalize().unwrap();
        let external = external.canonicalize().unwrap();
        assert!(canonical_path(checkout.to_str().unwrap(), &checkout).is_err());
        assert_eq!(
            canonical_path("../other", &checkout).unwrap(),
            sibling
        );
        assert!(under_allowed_root(&sibling, &checkout, &[]));
        assert!(!under_allowed_root(&external, &checkout, &[]));
        assert!(under_allowed_root(
            &external,
            &checkout,
            &[elsewhere.path().to_string_lossy().into_owned()]
        ));
    }

    #[test]
    fn participant_permission_caps_write_access() {
        let db = database();
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let other = temp.path().join("other");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let other = other.to_string_lossy().into_owned();
        db.execute(
            "INSERT INTO conversation_paths(conversation_id,path,access,created_at)
             VALUES ('chat',?1,'rw',0)",
            [&other],
        )
        .unwrap();
        assert_eq!(
            effective(&db, "chat", false, &main, &[]).unwrap()[0].access,
            Access::ReadOnly
        );
        assert_eq!(
            effective(&db, "chat", true, &main, &[]).unwrap()[0].access,
            Access::ReadWrite
        );
    }

    #[test]
    fn fork_copy_and_delete_are_transactional() {
        let mut db = database();
        db.execute(
            "INSERT INTO conversation_paths(conversation_id,path,access,created_at)
             VALUES ('source','/work/other','ro',0)",
            [],
        )
        .unwrap();
        let tx = db.transaction().unwrap();
        copy(&tx, "source", "fork").unwrap();
        delete(&tx, "source").unwrap();
        tx.commit().unwrap();
        let source: i64 = db
            .query_row(
                "SELECT count(*) FROM conversation_paths WHERE conversation_id='source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let fork: i64 = db
            .query_row(
                "SELECT count(*) FROM conversation_paths WHERE conversation_id='fork'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((source, fork), (0, 1));
    }

    #[test]
    fn selecting_a_mounted_checkout_removes_the_redundant_bind() {
        let mut db = database();
        db.execute(
            "INSERT INTO conversation_paths(conversation_id,path,access,created_at)
             VALUES ('chat','/work/other','rw',0)",
            [],
        )
        .unwrap();
        let tx = db.transaction().unwrap();
        remove_overlapping(&tx, "chat", Path::new("/work/other")).unwrap();
        tx.commit().unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM conversation_paths", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
