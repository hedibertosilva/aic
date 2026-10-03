//! Non-generating quota reads. Account cache lives only in the chat server.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use crate::agent::{AgentProcess, CliAgent};

#[derive(Debug, Clone, Serialize)]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    pub bucket: Option<String>,
    pub model: Option<String>,
    /// Models this window applies to, when the provider scopes quota by model group.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    pub used_pct: Option<f64>,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<String>,
    pub provider_status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaStatus {
    pub agent: String,
    pub state: String,
    pub windows: Vec<QuotaWindow>,
    pub source: Option<String>,
    pub fetched_at: Option<String>,
    pub attempted_at: Option<String>,
    pub reason: String,
    pub error: Option<String>,
    pub reading: bool,
    pub refreshable: bool,
}

impl QuotaStatus {
    pub fn initial(agent: &CliAgent) -> Self {
        let refreshable = Self::supported(agent);
        Self {
            agent: agent.name.clone(), state: "unavailable".into(), windows: Vec::new(),
            source: None, fetched_at: None, attempted_at: None, error: None, reading: false,
            refreshable,
            reason: if refreshable { "No quota reading yet".into() }
                else { format!("{} doesn't report quota to aic", agent.name) },
        }
    }

    /// Agents with a non-generating usage query: Codex app-server, `claude -p /usage`, `agy --print /usage`.
    pub fn supported(agent: &CliAgent) -> bool {
        agent.is_codex() || agent.is_claude() || agent.is_agy()
    }

    fn read_ok(agent: &CliAgent, windows: Vec<QuotaWindow>, source: &str, no_windows: &str) -> Self {
        let mut status = Self::initial(agent);
        status.state = if windows.iter().any(|w| w.used_pct.is_some()) { "available" } else { "unavailable" }.into();
        status.reason = if status.state == "available" { String::new() } else { no_windows.into() };
        status.windows = windows;
        status.source = Some(source.into());
        status.fetched_at = Some(Utc::now().to_rfc3339());
        status
    }
}

/// Normalizes a provider timestamp to UTC RFC 3339. Unparseable means unknown.
fn utc(raw: Option<&str>) -> Option<String> {
    raw.and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Utc).to_rfc3339())
}

pub fn codex_windows(result: &Value) -> Vec<QuotaWindow> {
    let buckets: Vec<(String, &Value)> = match result.get("rateLimitsByLimitId").and_then(Value::as_object) {
        Some(map) if !map.is_empty() => map.iter().map(|(id, value)| (id.clone(), value)).collect(),
        _ => result.get("rateLimits").filter(|v| v.is_object()).map(|v| {
            vec![(v.get("limitId").and_then(Value::as_str).unwrap_or("codex").to_string(), v)]
        }).unwrap_or_default(),
    };
    let mut windows = Vec::new();
    for (bucket_id, bucket) in buckets {
        for key in ["primary", "secondary"] {
            let Some(window) = bucket.get(key).filter(|v| v.is_object()) else { continue };
            let used_pct = window.get("usedPercent").and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v >= 0.0);
            // An absent percentage isn't a zero reading.
            let window_minutes = window.get("windowDurationMins").and_then(Value::as_u64);
            let resets_at = window.get("resetsAt").and_then(Value::as_i64)
                .and_then(|s| DateTime::from_timestamp(s, 0)).map(|t| t.to_rfc3339());
            windows.push(QuotaWindow {
                id: format!("{bucket_id}/{key}"),
                label: match window_minutes { Some(300) => "5-hour".into(), Some(10080) => "Weekly".into(),
                    Some(n) => format!("{n}-minute"), None => window.get("label").and_then(Value::as_str).unwrap_or(key).to_string() },
                bucket: bucket.get("limitName").and_then(Value::as_str).map(str::to_string)
                    .or_else(|| (bucket_id != "codex").then(|| bucket_id.clone())),
                model: bucket.get("model").and_then(Value::as_str).map(str::to_string),
                models: Vec::new(),
                used_pct, window_minutes, resets_at, provider_status: None,
            });
        }
    }
    windows
}

/// No prompts, threads, or token-file reads. CLI owns authentication/refresh.
pub fn read_codex(agent: &CliAgent, cwd: &Path) -> Result<QuotaStatus> {
    let AgentProcess { mut child, killer } = agent.spawn_query(&["app-server", "--listen", "stdio://"], cwd)?;
    let mut input = child.stdin.take().context("quota stdin missing")?;
    let output = child.stdout.take().context("quota stdout missing")?;
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut output = BufReader::new(output);
        loop {
            // Bound individual frames; reject oversized output instead of buffering forever.
            let mut frame = Vec::new();
            use std::io::Read;
            let result = output.by_ref().take(1 << 20).read_until(b'\n', &mut frame);
            match result {
                Ok(0) => break,
                Ok(_) if frame.len() >= 1 << 20 => { let _ = tx.send(Err("Quota response too large")); break; }
                Ok(_) => if let Ok(value) = serde_json::from_slice::<Value>(&frame) {
                    if tx.send(Ok(value)).is_err() { break; }
                },
                Err(_) => break,
            }
        }
    });
    let result = (|| -> Result<QuotaStatus> {
        let send = |input: &mut std::process::ChildStdin, value: Value| -> Result<()> {
            writeln!(input, "{value}")?; input.flush()?; Ok(())
        };
        send(&mut input, json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"aic-chat","version":env!("CARGO_PKG_VERSION")}}}))?;
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let message = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("Quota read timed out or the CLI closed its output")?
                .map_err(anyhow::Error::msg)?;
            let id = message.get("id").and_then(Value::as_u64);
            if !matches!(id, Some(1 | 2 | 3)) { continue; }
            if message.get("error").is_some() {
                // Raw provider errors can contain credentials/account information.
                bail!("Codex could not read quota (RPC error); check its login and retry");
            }
            match id {
                Some(1) => {
                    send(&mut input, json!({"method":"initialized","params":{}}))?;
                    send(&mut input, json!({"id":2,"method":"account/read","params":{"refreshToken":false}}))?;
                }
                Some(2) => {
                    if message.pointer("/result/account/type").and_then(Value::as_str) == Some("apiKey") {
                        let mut status = QuotaStatus::initial(agent);
                        status.reason = "This Codex account has no usage windows (API key)".into();
                        status.source = Some("Codex app-server account/read".into());
                        status.fetched_at = Some(Utc::now().to_rfc3339());
                        return Ok(status);
                    }
                    send(&mut input, json!({"id":3,"method":"account/rateLimits/read"}))?;
                }
                Some(3) => {
                    let result = message.get("result").context("Quota response missing result")?;
                    return Ok(QuotaStatus::read_ok(agent, codex_windows(result), "Codex app-server account/rateLimits/read",
                        "This Codex account has no usage windows"));
                }
                _ => {}
            }
        }
    })();
    drop(input);
    // Includes the sandbox container and children; timeout cannot orphan the RPC server.
    killer.kill();
    let _ = child.wait();
    let _ = reader.join();
    result
}

/// Claude's `/usage` report: `rate_limits.limits[]` with `percent` (0-100, used).
pub fn claude_windows(report: &Value) -> Vec<QuotaWindow> {
    let Some(limits) = report.pointer("/rate_limits/limits").and_then(Value::as_array) else { return Vec::new() };
    limits.iter().map(|limit| {
        let kind = limit.get("kind").and_then(Value::as_str).unwrap_or("limit");
        let (label, window_minutes) = match kind {
            "session" => ("5-hour".to_string(), Some(300)),
            k if k.starts_with("weekly") => ("Weekly".to_string(), Some(10080)),
            k => (k.replace('_', " "), None),
        };
        // `weekly_all` is the account-wide window; other suffixes (or a scope) name a model.
        let bucket = limit.get("scope").and_then(Value::as_str).map(str::to_string)
            .or_else(|| kind.strip_prefix("weekly_").filter(|s| *s != "all").map(str::to_string));
        QuotaWindow {
            id: kind.to_string(), label, bucket, model: None, models: Vec::new(),
            // An absent percentage isn't a zero reading.
            used_pct: limit.get("percent").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0),
            window_minutes,
            resets_at: utc(limit.get("resets_at").and_then(Value::as_str)),
            provider_status: limit.get("severity").and_then(Value::as_str).filter(|s| *s != "normal").map(str::to_string),
        }
    }).collect()
}

/// agy's `/usage` data: model groups sharing buckets with `remaining_fraction`.
pub fn agy_windows(data: &Value) -> Vec<QuotaWindow> {
    let mut windows = Vec::new();
    for group in data.get("groups").and_then(Value::as_array).into_iter().flatten() {
        let name = group.get("name").and_then(Value::as_str).map(str::to_string);
        let models: Vec<String> = group.get("description").and_then(Value::as_str)
            .and_then(|d| d.strip_prefix("Models within this group:"))
            .map(|list| list.split(',').map(|m| m.trim().trim_end_matches('.').to_string()).filter(|m| !m.is_empty()).collect())
            .unwrap_or_default();
        for bucket in group.get("buckets").and_then(Value::as_array).into_iter().flatten() {
            let window = bucket.get("window").and_then(Value::as_str);
            let (label, window_minutes) = match window {
                Some("weekly") => ("Weekly".to_string(), Some(10080)),
                Some("daily") => ("Daily".to_string(), Some(1440)),
                other => (other.or(bucket.get("name").and_then(Value::as_str)).unwrap_or("Limit").to_string(), None),
            };
            windows.push(QuotaWindow {
                id: bucket.get("id").and_then(Value::as_str).unwrap_or("bucket").to_string(),
                label, bucket: name.clone(), model: None, models: models.clone(),
                // `remaining_fraction` is what's left; the text `response` percentages are too.
                used_pct: bucket.get("remaining_fraction").and_then(Value::as_f64)
                    .filter(|r| r.is_finite()).map(|r| ((1.0 - r.clamp(0.0, 1.0)) * 1000.0).round() / 10.0),
                window_minutes,
                resets_at: utc(bucket.get("reset_time").and_then(Value::as_str)),
                provider_status: None,
            });
        }
    }
    windows
}

/// One-shot query. stdin closes at once, so a signed-out CLI fails instead of
/// waiting for a pasted login code. The process group (and container) is
/// always killed, so a timeout can't orphan it.
fn run_query(agent: &CliAgent, args: &[&str], cwd: &Path) -> Result<String> {
    const LIMIT: u64 = 1 << 20;
    let AgentProcess { mut child, killer } = agent.spawn_query(args, cwd)?;
    drop(child.stdin.take());
    let output = child.stdout.take().context("quota stdout missing")?;
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = output.take(LIMIT + 1).read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let result = rx.recv_timeout(Duration::from_secs(20)).context("Quota read timed out");
    killer.kill();
    let _ = child.wait();
    let _ = reader.join();
    let bytes = result?;
    if bytes.len() as u64 > LIMIT { bail!("Quota response too large"); }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn json_lines(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok())
}

/// `--no-session-persistence`: without it `/usage` would leave a session behind.
pub fn read_claude(agent: &CliAgent, cwd: &Path) -> Result<QuotaStatus> {
    let out = run_query(agent, &["-p", "/usage", "--no-session-persistence", "--output-format", "stream-json", "--verbose"], cwd)?;
    let report = json_lines(&out).find_map(|event| event.get("usage_report").cloned())
        .context("Claude returned no usage report")?;
    Ok(QuotaStatus::read_ok(agent, claude_windows(&report), "Claude /usage",
        "Claude reported no usage windows for this account"))
}

pub fn read_agy(agent: &CliAgent, cwd: &Path) -> Result<QuotaStatus> {
    let out = run_query(agent, &["--print", "/usage", "--output-format", "json"], cwd)?;
    // Signed out: the result carries status ERROR and no command data.
    let data = json_lines(&out).find_map(|v| v.pointer("/command/data").filter(|d| d.get("groups").is_some()).cloned())
        .context("agy returned no usage data")?;
    Ok(QuotaStatus::read_ok(agent, agy_windows(&data), "agy /usage",
        "agy reported no usage limits for this account"))
}

pub fn read(agent: &CliAgent, cwd: &Path) -> Result<QuotaStatus> {
    if agent.is_codex() { read_codex(agent, cwd) }
    else if agent.is_claude() { read_claude(agent, cwd) }
    else if agent.is_agy() { read_agy(agent, cwd) }
    else { bail!("{} has no quota source", agent.name) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_all_buckets_and_keeps_missing_values_unknown() {
        let result = json!({"rateLimitsByLimitId":{
            "a":{"primary":{"usedPercent":80,"windowDurationMins":300,"resetsAt":1791052371},"secondary":{"windowDurationMins":10080}},
            "b":{"limitName":"Code review","primary":{"usedPercent":100,"windowDurationMins":60}}
        }});
        let windows = codex_windows(&result);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].used_pct, Some(80.0));
        assert!(windows[0].resets_at.as_ref().unwrap().ends_with("+00:00"));
        assert_eq!(windows[1].used_pct, None);
        assert_eq!(windows[2].used_pct, Some(100.0));
        assert_eq!(windows[2].bucket.as_deref(), Some("Code review"));
        assert_eq!(windows[2].model, None);
        assert!(codex_windows(&json!({"rateLimits":null})).is_empty());
    }
    // Captured from `agy --print /usage --output-format json` (agy 1.2.16).
    const AGY: &str = r#"{"description":"x","groups":[
      {"name":"Gemini Models","description":"Models within this group: Gemini Flash, Gemini Pro","buckets":[{"id":"gemini-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":0,"reset_time":"2026-10-08T00:35:13Z"}]},
      {"name":"Claude and GPT models","description":"Models within this group: Claude Opus, Claude Sonnet, GPT-OSS","buckets":[{"id":"3p-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":1,"reset_time":"2026-10-10T18:37:32Z"}]}]}"#;
    #[test]
    fn agy_groups_become_used_percent_windows_scoped_to_their_models() {
        let windows = agy_windows(&serde_json::from_str(AGY).unwrap());
        assert_eq!(windows.len(), 2);
        // remaining_fraction 0 is an exhausted group, not an unused one.
        assert_eq!(windows[0].used_pct, Some(100.0));
        assert_eq!(windows[0].bucket.as_deref(), Some("Gemini Models"));
        assert_eq!(windows[0].models, ["Gemini Flash", "Gemini Pro"]);
        assert_eq!(windows[0].window_minutes, Some(10080));
        assert_eq!(windows[0].resets_at.as_deref(), Some("2026-10-08T00:35:13+00:00"));
        assert_eq!(windows[1].used_pct, Some(0.0));
        assert_eq!(windows[1].models, ["Claude Opus", "Claude Sonnet", "GPT-OSS"]);
    }
    #[test]
    fn agy_missing_or_odd_values_stay_unknown() {
        let data = json!({"groups":[{"name":"G","buckets":[{"id":"a","window":"weekly","reset_time":"soon"},
            {"id":"b","window":"daily","remaining_fraction":0.3,"reset_time":"2026-10-08T00:35:13Z"}]}]});
        let windows = agy_windows(&data);
        assert_eq!(windows[0].used_pct, None);
        assert_eq!(windows[0].resets_at, None);
        assert_eq!(windows[1].used_pct, Some(70.0));
        assert!(windows[0].models.is_empty());
        assert!(agy_windows(&json!({})).is_empty());
    }
    // Captured from the `usage_report` of `claude -p /usage --output-format stream-json --verbose` (2.1.287).
    #[test]
    fn claude_limits_become_windows_and_keep_absent_percent_unknown() {
        let report = json!({"rate_limits":{"limits":[
            {"kind":"session","group":"session","percent":0,"resets_at":"2026-10-03T23:30:00.130413+00:00","scope":null,"severity":"normal","is_active":false},
            {"kind":"weekly_all","group":"weekly","percent":8,"resets_at":"2026-10-09T03:00:00.130431+00:00","scope":null,"severity":"normal","is_active":true},
            {"kind":"weekly_opus","percent":null,"severity":"warning"}]}});
        let windows = claude_windows(&report);
        assert_eq!(windows.len(), 3);
        assert_eq!((windows[0].label.as_str(), windows[0].window_minutes, windows[0].used_pct), ("5-hour", Some(300), Some(0.0)));
        assert_eq!((windows[1].label.as_str(), windows[1].window_minutes, windows[1].used_pct), ("Weekly", Some(10080), Some(8.0)));
        assert_eq!(windows[1].bucket, None);
        assert_eq!(windows[1].resets_at.as_deref(), Some("2026-10-09T03:00:00.130431+00:00"));
        assert_eq!(windows[2].bucket.as_deref(), Some("opus"));
        assert_eq!(windows[2].used_pct, None);
        assert_eq!(windows[2].provider_status.as_deref(), Some("warning"));
        assert!(claude_windows(&json!({"rate_limits":{"limits":[],"extra_usage":{}}})).is_empty());
        assert!(claude_windows(&json!({})).is_empty());
    }
}
