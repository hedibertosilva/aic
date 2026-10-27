use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};
use base64::Engine;
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tiny_http::{Header, Method, Request, Response, StatusCode};

const HTML: &str = include_str!("public.html");
const CSS: &str = include_str!("public.css");
const JS: &str = include_str!("public.js");
const MAX_BODY: u64 = 64 << 20;
const SESSION_AGE_MS: i64 = 24 * 60 * 60 * 1000;
const ATTEMPT_WINDOW_MS: i64 = 10 * 60 * 1000;
const MAX_ATTEMPTS: u32 = 10;

struct App {
    db: Mutex<Connection>,
    public_url: String,
    admin_token: String,
    attempts: Mutex<HashMap<String, (i64, u32)>>,
}

#[derive(Deserialize)]
struct CreateShare {
    #[serde(default)]
    name: String,
    #[serde(default)]
    expires_at: Option<i64>,
    snapshot: serde_json::Value,
}

#[derive(Deserialize)]
struct UpdateShare {
    #[serde(default)]
    name: String,
    #[serde(default)]
    expires_at: Option<i64>,
    snapshot: serde_json::Value,
}

#[derive(Deserialize)]
struct Unlock {
    password: String,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid response header")
}

fn random_urlsafe(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut value);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value)
}

fn random_password() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
        .chunks(5)
        .map(|chunk| {
            chunk
                .iter()
                .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn password_hash(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|value| value.to_string())
        .map_err(|error| anyhow::anyhow!("hashing password: {error}"))
}

fn password_matches(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn open_db(data_dir: &Path) -> Result<Connection> {
    fs::create_dir_all(data_dir)?;
    let path = data_dir.join("shares.sqlite3");
    let db = Connection::open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(data_dir, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS shares (
           id TEXT PRIMARY KEY, name TEXT NOT NULL, password_hash TEXT NOT NULL,
           snapshot_json TEXT NOT NULL, version INTEGER NOT NULL DEFAULT 1,
           expires_at INTEGER, status TEXT NOT NULL DEFAULT 'active',
           auth_version INTEGER NOT NULL DEFAULT 1,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS share_sessions (
           token_hash TEXT PRIMARY KEY, share_id TEXT NOT NULL,
           auth_version INTEGER NOT NULL, expires_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS share_sessions_share ON share_sessions(share_id);",
    )?;
    Ok(db)
}

pub fn run(
    host: &str,
    port: u16,
    data_dir: &Path,
    public_url: &str,
    admin_token: &str,
) -> Result<()> {
    if admin_token.len() < 32 {
        anyhow::bail!("publisher token must contain at least 32 characters");
    }
    let server = tiny_http::Server::http((host, port))
        .map_err(|error| anyhow::anyhow!("binding {host}:{port}: {error}"))?;
    let app = Arc::new(App {
        db: Mutex::new(open_db(data_dir)?),
        public_url: public_url.trim_end_matches('/').to_string(),
        admin_token: admin_token.to_string(),
        attempts: Mutex::default(),
    });
    println!("aic sharing relay on http://{host}:{port} ({public_url})");
    for request in server.incoming_requests() {
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            if let Err(error) = handle(&app, request) {
                eprintln!("share request failed: {error:#}");
            }
        });
    }
    Ok(())
}

fn response(
    request: Request,
    status: u16,
    content_type: &str,
    body: impl Into<Vec<u8>>,
    extra: &[Header],
) -> Result<()> {
    let mut value = Response::from_data(body.into())
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Referrer-Policy", "no-referrer"))
        .with_header(header("X-Frame-Options", "DENY"));
    for item in extra {
        value.add_header(item.clone());
    }
    request.respond(value)?;
    Ok(())
}

fn json(request: Request, status: u16, value: serde_json::Value) -> Result<()> {
    response(request, status, "application/json", value.to_string(), &[])
}

fn read_json<T: for<'de> Deserialize<'de>>(request: &mut Request) -> Result<T> {
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY + 1)
        .read_to_string(&mut body)?;
    if body.len() as u64 > MAX_BODY {
        anyhow::bail!("request body is too large");
    }
    Ok(serde_json::from_str(&body)?)
}

fn bearer(request: &Request, expected: &str) -> bool {
    request
        .headers()
        .iter()
        .find(|item| {
            item.field
                .as_str()
                .as_str()
                .eq_ignore_ascii_case("Authorization")
        })
        .is_some_and(|item| item.value.as_str().strip_prefix("Bearer ") == Some(expected))
}

fn cookie(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|item| item.field.as_str().as_str().eq_ignore_ascii_case("Cookie"))
        .and_then(|item| {
            item.value
                .as_str()
                .split(';')
                .map(str::trim)
                .find_map(|part| part.strip_prefix(&format!("{name}=")).map(str::to_string))
        })
}

fn valid_id(id: &str) -> bool {
    id.starts_with("shr_")
        && id.len() <= 80
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn active_share(db: &Connection, id: &str) -> Result<Option<(String, i64, String, i64)>> {
    let row = db
        .query_row(
            "SELECT snapshot_json,updated_at,status,COALESCE(expires_at,0) FROM shares WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    Ok(
        row.filter(|(_, _, status, expires): &(String, i64, String, i64)| {
            status == "active" && (*expires == 0 || *expires > now())
        }),
    )
}

fn authorized_public(app: &App, request: &Request, id: &str) -> Result<bool> {
    let Some(token) = cookie(request, &format!("aic_share_{id}")) else {
        return Ok(false);
    };
    let db = app.db.lock().expect("share db lock poisoned");
    let valid: Option<i64> = db
        .query_row(
            "SELECT 1 FROM share_sessions s JOIN shares h ON h.id=s.share_id
         WHERE s.token_hash=?1 AND s.share_id=?2 AND s.expires_at>?3
           AND s.auth_version=h.auth_version AND h.status='active'
           AND (h.expires_at IS NULL OR h.expires_at>?3)",
            params![token_hash(&token), id, now()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(valid.is_some())
}

fn handle(app: &App, request: Request) -> Result<()> {
    let path = request.url().split('?').next().unwrap_or("/").to_string();
    if path == "/share.css" && request.method() == &Method::Get {
        return response(request, 200, "text/css; charset=utf-8", CSS, &[]);
    }
    if path == "/share.js" && request.method() == &Method::Get {
        return response(
            request,
            200,
            "text/javascript; charset=utf-8",
            JS,
            &[header(
                "Content-Security-Policy",
                "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            )],
        );
    }
    if let Some(_id) = path
        .strip_prefix("/s/")
        .filter(|id| !id.contains('/') && valid_id(id))
    {
        // Serving the same locked page for valid-looking unknown ids avoids
        // exposing which sharing links exist.
        if request.method() == &Method::Get {
            return response(
                request,
                200,
                "text/html; charset=utf-8",
                HTML,
                &[header(
                    "Content-Security-Policy",
                    "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
                )],
            );
        }
    }
    if let Some(rest) = path.strip_prefix("/api/publisher/shares") {
        if !bearer(&request, &app.admin_token) {
            return json(request, 401, serde_json::json!({"error":"unauthorized"}));
        }
        return publisher_route(app, request, rest);
    }
    if let Some(rest) = path.strip_prefix("/api/public/shares/") {
        let mut parts = rest.split('/');
        let id = parts.next().unwrap_or("");
        let action = parts.next().unwrap_or("");
        if !valid_id(id) || parts.next().is_some() {
            return json(request, 404, serde_json::json!({"error":"not found"}));
        }
        return public_route(app, request, id, action);
    }
    json(request, 404, serde_json::json!({"error":"not found"}))
}

fn publisher_route(app: &App, mut request: Request, rest: &str) -> Result<()> {
    if rest.is_empty() && request.method() == &Method::Get {
        return json(
            request,
            200,
            serde_json::json!({"ok":true,"public_url":app.public_url}),
        );
    }
    if rest.is_empty() && request.method() == &Method::Post {
        let payload: CreateShare = match read_json(&mut request) {
            Ok(value) => value,
            Err(_) => return json(request, 400, serde_json::json!({"error":"invalid request"})),
        };
        let id = format!("shr_{}", random_urlsafe(18));
        let password = random_password();
        let timestamp = now();
        let mut snapshot = payload.snapshot;
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("name".into(), payload.name.clone().into());
            object.insert("updated_at".into(), timestamp.into());
        }
        app.db.lock().expect("share db lock poisoned").execute(
            "INSERT INTO shares(id,name,password_hash,snapshot_json,expires_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6)",
            params![id,payload.name,password_hash(&password)?,snapshot.to_string(),payload.expires_at,timestamp])?;
        return json(
            request,
            201,
            serde_json::json!({"shared_id":id,"url":format!("{}/s/{}",app.public_url,id),"password":password,"updated_at":timestamp}),
        );
    }
    let Some(value) = rest.strip_prefix('/') else {
        return json(request, 404, serde_json::json!({"error":"not found"}));
    };
    let mut parts = value.split('/');
    let id = parts.next().unwrap_or("");
    let action = parts.next().unwrap_or("");
    if !valid_id(id) || parts.next().is_some() {
        return json(request, 404, serde_json::json!({"error":"not found"}));
    }
    if action.is_empty() && request.method() == &Method::Put {
        let payload: UpdateShare = match read_json(&mut request) {
            Ok(value) => value,
            Err(_) => return json(request, 400, serde_json::json!({"error":"invalid request"})),
        };
        let timestamp = now();
        let mut snapshot = payload.snapshot;
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("name".into(), payload.name.clone().into());
            object.insert("updated_at".into(), timestamp.into());
        }
        let changed = app.db.lock().expect("share db lock poisoned").execute(
            "UPDATE shares SET name=?2,snapshot_json=?3,expires_at=?4,updated_at=?5,version=version+1 WHERE id=?1 AND status='active'",
            params![id,payload.name,snapshot.to_string(),payload.expires_at,timestamp])?;
        return json(
            request,
            if changed == 1 { 200 } else { 404 },
            serde_json::json!({"updated_at":timestamp,"updated":changed==1}),
        );
    }
    if action == "rotate-password" && request.method() == &Method::Post {
        let password = random_password();
        let changed = app.db.lock().expect("share db lock poisoned").execute(
            "UPDATE shares SET password_hash=?2,auth_version=auth_version+1,updated_at=?3 WHERE id=?1 AND status='active'", params![id,password_hash(&password)?,now()])?;
        return if changed == 1 {
            json(request, 200, serde_json::json!({"password":password}))
        } else {
            json(request, 404, serde_json::json!({"error":"not found"}))
        };
    }
    if action.is_empty() && request.method() == &Method::Delete {
        let changed = app.db.lock().expect("share db lock poisoned").execute("UPDATE shares SET status='revoked',auth_version=auth_version+1,updated_at=?2 WHERE id=?1", params![id,now()])?;
        return json(
            request,
            if changed == 1 { 204 } else { 404 },
            serde_json::json!({}),
        );
    }
    json(request, 404, serde_json::json!({"error":"not found"}))
}

fn public_route(app: &App, mut request: Request, id: &str, action: &str) -> Result<()> {
    if action == "unlock" && request.method() == &Method::Post {
        let remote = request
            .remote_addr()
            .map(|value| value.ip().to_string())
            .unwrap_or_else(|| "unknown".into());
        let key = format!("{remote}:{id}");
        {
            let mut attempts = app.attempts.lock().expect("attempt lock poisoned");
            attempts.retain(|_, (started, _)| now() - *started < ATTEMPT_WINDOW_MS);
            if attempts
                .get(&key)
                .is_some_and(|(_, count)| *count >= MAX_ATTEMPTS)
            {
                return json(request, 429, serde_json::json!({"error":"try again later"}));
            }
        }
        let payload: Unlock = match read_json(&mut request) {
            Ok(value) => value,
            Err(_) => {
                return json(
                    request,
                    401,
                    serde_json::json!({"error":"invalid password"}),
                );
            }
        };
        let db = app.db.lock().expect("share db lock poisoned");
        let stored: Option<(String,i64)> = db.query_row(
            "SELECT password_hash,auth_version FROM shares WHERE id=?1 AND status='active' AND (expires_at IS NULL OR expires_at>?2)",
            params![id,now()], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        let Some((hash, auth_version)) =
            stored.filter(|(hash, _)| password_matches(hash, &payload.password))
        else {
            let mut attempts = app.attempts.lock().expect("attempt lock poisoned");
            let entry = attempts.entry(key).or_insert((now(), 0));
            entry.1 += 1;
            return json(
                request,
                401,
                serde_json::json!({"error":"invalid password"}),
            );
        };
        drop(hash);
        app.attempts
            .lock()
            .expect("attempt lock poisoned")
            .remove(&key);
        let session = random_urlsafe(32);
        db.execute("INSERT INTO share_sessions(token_hash,share_id,auth_version,expires_at) VALUES(?1,?2,?3,?4)", params![token_hash(&session),id,auth_version,now()+SESSION_AGE_MS])?;
        let secure = if app.public_url.starts_with("https://") {
            "; Secure"
        } else {
            ""
        };
        return response(
            request,
            204,
            "application/json",
            Vec::new(),
            &[header(
                "Set-Cookie",
                &format!(
                    "aic_share_{id}={session}; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400{secure}"
                ),
            )],
        );
    }
    if action == "logout" && request.method() == &Method::Post {
        if let Some(token) = cookie(&request, &format!("aic_share_{id}")) {
            let _ = app.db.lock().expect("share db lock poisoned").execute(
                "DELETE FROM share_sessions WHERE token_hash=?1",
                [token_hash(&token)],
            );
        }
        return response(
            request,
            204,
            "application/json",
            Vec::new(),
            &[header(
                "Set-Cookie",
                &format!("aic_share_{id}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
            )],
        );
    }
    if action.is_empty() && request.method() == &Method::Get {
        if !authorized_public(app, &request, id)? {
            return json(request, 401, serde_json::json!({"error":"unauthorized"}));
        }
        let db = app.db.lock().expect("share db lock poisoned");
        let Some((snapshot, _, _, _)) = active_share(&db, id)? else {
            return json(request, 404, serde_json::json!({"error":"not found"}));
        };
        return response(request, 200, "application/json", snapshot, &[]);
    }
    json(request, 404, serde_json::json!({"error":"not found"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passwords_verify_but_wrong_passwords_do_not() {
        let password = random_password();
        assert_eq!(password.split('-').count(), 4);
        let hash = password_hash(&password).unwrap();
        assert!(password_matches(&hash, &password));
        assert!(!password_matches(&hash, "wrong"));
    }

    #[test]
    fn ids_are_narrowly_validated() {
        assert!(valid_id("shr_abc-123"));
        assert!(!valid_id("abc"));
        assert!(!valid_id("shr_../etc"));
    }
}
