//! Non-generating quota reads. Account cache lives only in the chat server.
use std::io::{BufRead, BufReader, Write};
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
        let codex = agent.is_codex();
        Self {
            agent: agent.name.clone(), state: "unavailable".into(), windows: Vec::new(),
            source: None, fetched_at: None, attempted_at: None, error: None, reading: false,
            refreshable: codex,
            reason: if codex { "No quota reading yet".into() }
                else if agent.is_agy() { format!("{} doesn't report quota to aic yet", agent.name) }
                else { "No quota reading yet".into() },
        }
    }
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
                    let mut status = QuotaStatus::initial(agent);
                    status.windows = codex_windows(result);
                    status.state = if status.windows.iter().any(|w| w.used_pct.is_some()) { "available" } else { "unavailable" }.into();
                    status.reason = if status.state == "available" { String::new() } else { "This Codex account has no usage windows".into() };
                    status.source = Some("Codex app-server account/rateLimits/read".into());
                    status.fetched_at = Some(Utc::now().to_rfc3339());
                    return Ok(status);
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
}
