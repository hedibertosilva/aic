//! Turns the raw JSON event lines of Claude, Gemini and Codex into one small
//! event vocabulary the web chat understands.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    /// The id to pass back as `resume` on the next turn.
    Session { id: String },
    /// A chunk of the assistant's reply.
    Delta { text: String },
    /// What the agent is doing right now (reading a file, running a command).
    Activity { text: String },
    /// End of turn. `text` is the full reply as the agent reports it.
    Done {
        ok: bool,
        text: String,
        error: Option<String>,
        /// Tokens and cost, when the agent reports them.
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
}

/// What one turn cost. Every field is optional: each agent reports a
/// different subset (agy may report none at all).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Prompt tokens served from / written to the prompt cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u64>,
}

impl Usage {
    /// Reads the `usage` object plus the `total_cost_usd`/`num_turns` next
    /// to it in a result event. `None` when there's none of them.
    fn from_result(value: &Value) -> Option<Usage> {
        let tokens = value.get("usage");
        let num = |key: &str| tokens.and_then(|u| u.get(key)).and_then(Value::as_u64);
        let usage = Usage {
            input_tokens: num("input_tokens"),
            output_tokens: num("output_tokens"),
            cache_read_tokens: num("cache_read_input_tokens"),
            cache_write_tokens: num("cache_creation_input_tokens"),
            cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
            num_turns: value.get("num_turns").and_then(Value::as_u64),
        };
        (usage != Usage::default()).then_some(usage)
    }
}

/// Models and reasoning efforts an agent CLI says it can use.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelCatalog {
    pub models: Vec<String>,
    pub efforts: Vec<String>,
}

/// `codex debug models` prints one JSON object. Hidden models are left out;
/// efforts are the union across the rest, in the order the CLI lists them.
pub fn parse_codex_models(output: &str) -> ModelCatalog {
    let mut catalog = ModelCatalog::default();
    let Ok(value) = serde_json::from_str::<Value>(output.trim()) else { return catalog };
    let Some(models) = value.get("models").and_then(Value::as_array) else { return catalog };
    for model in models {
        if str_of(model, "visibility") != Some("list") {
            continue;
        }
        let Some(slug) = str_of(model, "slug") else { continue };
        catalog.models.push(slug.to_string());
        let levels = model.get("supported_reasoning_levels").and_then(Value::as_array);
        for effort in levels.into_iter().flatten().filter_map(|l| str_of(l, "effort")) {
            if !catalog.efforts.iter().any(|e| e == effort) {
                catalog.efforts.push(effort.to_string());
            }
        }
    }
    catalog
}

/// `agy models` prints a plain list; its exact layout isn't documented, so
/// take the first word of each line that looks like a model id and skip the
/// status lines ("Fetching ...", "Error: ...").
pub fn parse_agy_models(output: &str) -> ModelCatalog {
    let mut catalog = ModelCatalog::default();
    for line in output.lines() {
        let word = line.trim().trim_start_matches(['-', '*', '•']).split_whitespace().next().unwrap_or("");
        let id_like = word.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && word.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
            && word.chars().any(|c| c.is_ascii_digit() || c == '-');
        if id_like && !catalog.models.iter().any(|m| m == word) {
            catalog.models.push(word.to_string());
        }
    }
    catalog
}

/// Parses one stdout line. Lines that aren't JSON, or events the chat has no
/// use for (hooks, tool calls, ...), yield `None`.
#[cfg(test)]
pub fn parse_line(is_agy: bool, line: &str) -> Option<ChatEvent> {
    parse_line_for(is_agy, false, line)
}

pub fn parse_line_for(is_agy: bool, is_codex: bool, line: &str) -> Option<ChatEvent> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    if is_codex {
        parse_codex(&value)
    } else if is_agy {
        parse_agy(&value)
    } else {
        parse_claude(&value)
    }
}

fn parse_codex(value: &Value) -> Option<ChatEvent> {
    match str_of(value, "type")? {
        "thread.started" => Some(ChatEvent::Session { id: str_of(value, "thread_id")?.to_string() }),
        "item.started" => {
            let item = value.get("item")?;
            (str_of(item, "type") == Some("command_execution")).then(|| {
                let command = str_of(item, "command").unwrap_or("command");
                ChatEvent::Activity { text: format!("Bash: {}", command.lines().next().unwrap_or("")) }
            })
        }
        "item.completed" => {
            let item = value.get("item")?;
            (str_of(item, "type") == Some("agent_message")).then(|| ChatEvent::Delta {
                text: str_of(item, "text").unwrap_or_default().to_string(),
            })
        }
        "turn.completed" => Some(ChatEvent::Done { ok: true, text: String::new(), error: None, usage: None }),
        "turn.failed" => Some(ChatEvent::Done {
            ok: false,
            text: String::new(),
            error: value.get("error").and_then(Value::as_str).map(str::to_string).or_else(|| Some("Codex turn failed".into())),
            usage: None,
        }),
        _ => None,
    }
}

fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn parse_claude(value: &Value) -> Option<ChatEvent> {
    match str_of(value, "type")? {
        "system" if str_of(value, "subtype") == Some("init") => Some(ChatEvent::Session {
            id: str_of(value, "session_id")?.to_string(),
        }),
        "assistant" => {
            // Complete content blocks; tool calls carry their input here.
            let blocks = value.get("message")?.get("content")?.as_array()?;
            let text = blocks
                .iter()
                .rev()
                .find(|b| str_of(b, "type") == Some("tool_use"))
                .map(describe_tool)?;
            Some(ChatEvent::Activity { text })
        }
        "stream_event" => {
            let delta = value.get("event")?.get("delta")?;
            if str_of(delta, "type") != Some("text_delta") {
                return None;
            }
            Some(ChatEvent::Delta { text: str_of(delta, "text")?.to_string() })
        }
        "result" => {
            let is_error = value.get("is_error").and_then(Value::as_bool).unwrap_or(true);
            let ok = str_of(value, "subtype") == Some("success") && !is_error;
            let text = str_of(value, "result").unwrap_or_default().to_string();
            let error = (!ok).then(|| {
                if text.is_empty() {
                    format!("agent finished with {}", str_of(value, "subtype").unwrap_or("an error"))
                } else {
                    text.clone()
                }
            });
            Some(ChatEvent::Done { ok, text, error, usage: Usage::from_result(value) })
        }
        _ => None,
    }
}

/// "Read src/agent.rs", "Bash: cargo test", ... — one short line per tool call.
fn describe_tool(block: &Value) -> String {
    let name = str_of(block, "name").unwrap_or("tool");
    let input = block.get("input");
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input.and_then(|i| str_of(i, k)))
            .map(|s| s.lines().next().unwrap_or("").to_string())
    };
    let detail = match name {
        "Bash" => pick(&["description", "command"]),
        "Read" | "Edit" | "Write" | "NotebookEdit" => pick(&["file_path", "notebook_path"]),
        "Grep" | "Glob" => pick(&["pattern"]),
        "WebFetch" => pick(&["url"]),
        "WebSearch" => pick(&["query"]),
        _ => pick(&["description", "file_path", "path", "query"]),
    };
    let mut text = match detail {
        Some(d) if !d.is_empty() => format!("{name}: {d}"),
        _ => name.to_string(),
    };
    if text.chars().count() > 100 {
        text = text.chars().take(99).collect::<String>() + "…";
    }
    text
}

fn parse_agy(value: &Value) -> Option<ChatEvent> {
    match str_of(value, "event")? {
        "step_update" => {
            let step = value.get("step_update")?;
            let kind = str_of(step, "step_type")?;
            if matches!(kind, "user_input" | "error_message" | "planner_response") {
                return None;
            }
            Some(ChatEvent::Activity { text: kind.replace('_', " ") })
        }
        "init" => Some(ChatEvent::Session {
            id: str_of(value, "conversation_id")?.to_string(),
        }),
        "result" => {
            let result = value.get("result")?;
            let ok = str_of(result, "status") == Some("SUCCESS");
            let text = str_of(result, "response").unwrap_or_default().to_string();
            let error = (!ok).then(|| {
                str_of(result, "error")
                    .filter(|e| !e.is_empty())
                    .unwrap_or("agent returned an error")
                    .to_string()
            });
            // Same shape as claude's if agy reports any; nothing otherwise.
            Some(ChatEvent::Done { ok, text, error, usage: Usage::from_result(result) })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_text_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Oi"}}}"#;
        assert_eq!(parse_line(false, line), Some(ChatEvent::Delta { text: "Oi".into() }));
    }

    #[test]
    fn claude_tool_use_becomes_activity() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"Read","input":{"file_path":"src/agent.rs"}}]}}"#;
        assert_eq!(
            parse_line(false, line),
            Some(ChatEvent::Activity { text: "Read: src/agent.rs".into() })
        );
        let bash = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test\nls","description":"Run tests"}}]}}"#;
        assert_eq!(
            parse_line(false, bash),
            Some(ChatEvent::Activity { text: "Bash: Run tests".into() })
        );
        let plain = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(parse_line(false, plain), None);
    }

    #[test]
    fn agy_steps_become_activity_except_chatter() {
        let step = r#"{"event":"step_update","step_update":{"step_type":"run_command","state":"RUNNING"}}"#;
        assert_eq!(parse_line(true, step), Some(ChatEvent::Activity { text: "run command".into() }));
        let user = r#"{"event":"step_update","step_update":{"step_type":"user_input","state":"DONE"}}"#;
        assert_eq!(parse_line(true, user), None);
    }

    #[test]
    fn claude_ignores_non_text_deltas_and_hooks() {
        let thinking = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}}"#;
        assert_eq!(parse_line(false, thinking), None);
        let hook = r#"{"type":"system","subtype":"hook_started","session_id":"x"}"#;
        assert_eq!(parse_line(false, hook), None);
    }

    #[test]
    fn claude_init_and_result() {
        let init = r#"{"type":"system","subtype":"init","session_id":"s1"}"#;
        assert_eq!(parse_line(false, init), Some(ChatEvent::Session { id: "s1".into() }));
        let done = r#"{"type":"result","subtype":"success","is_error":false,"result":"hello","session_id":"s1"}"#;
        assert_eq!(
            parse_line(false, done),
            Some(ChatEvent::Done { ok: true, text: "hello".into(), error: None, usage: None })
        );
    }

    #[test]
    fn claude_error_result_carries_an_error() {
        let done = r#"{"type":"result","subtype":"success","is_error":true,"result":"rate limited"}"#;
        assert_eq!(
            parse_line(false, done),
            Some(ChatEvent::Done { ok: false, text: "rate limited".into(), error: Some("rate limited".into()), usage: None })
        );
    }

    #[test]
    fn agy_init_and_result() {
        let init = r#"{"event":"init","conversation_id":"c1","init":{"cwd":"/"}}"#;
        assert_eq!(parse_line(true, init), Some(ChatEvent::Session { id: "c1".into() }));
        let done = r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"hi"}}"#;
        assert_eq!(
            parse_line(true, done),
            Some(ChatEvent::Done { ok: true, text: "hi".into(), error: None, usage: None })
        );
    }

    #[test]
    fn codex_thread_message_and_completion() {
        let init = r#"{"type":"thread.started","thread_id":"t1"}"#;
        assert_eq!(parse_line_for(false, true, init), Some(ChatEvent::Session { id: "t1".into() }));
        let message = r#"{"type":"item.completed","item":{"type":"agent_message","text":"hi"}}"#;
        assert_eq!(parse_line_for(false, true, message), Some(ChatEvent::Delta { text: "hi".into() }));
        assert_eq!(parse_line_for(false, true, r#"{"type":"turn.completed"}"#), Some(ChatEvent::Done { ok: true, text: String::new(), error: None, usage: None }));
    }

    #[test]
    fn agy_error_result_and_noise() {
        let done = r#"{"event":"result","result":{"status":"ERROR","response":"","error":"Individual quota reached."}}"#;
        assert_eq!(
            parse_line(true, done),
            Some(ChatEvent::Done { ok: false, text: String::new(), error: Some("Individual quota reached.".into()), usage: None })
        );
        assert_eq!(parse_line(true, "error: Individual quota reached."), None);
        assert_eq!(parse_line(true, r#"{"event":"step_update","step_update":{}}"#), None);
    }

    #[test]
    fn claude_result_carries_usage_and_cost() {
        let done = r#"{"type":"result","subtype":"success","is_error":false,"result":"hi","num_turns":3,"total_cost_usd":0.0123,"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":300,"cache_creation_input_tokens":40}}"#;
        let Some(ChatEvent::Done { usage: Some(usage), .. }) = parse_line(false, done) else {
            panic!("expected a done event with usage");
        };
        assert_eq!(
            usage,
            Usage {
                input_tokens: Some(10),
                output_tokens: Some(20),
                cache_read_tokens: Some(300),
                cache_write_tokens: Some(40),
                cost_usd: Some(0.0123),
                num_turns: Some(3),
            }
        );
        let event = ChatEvent::Done { ok: true, text: String::new(), error: None, usage: Some(usage) };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["usage"]["output_tokens"], 20);
    }

    #[test]
    fn missing_usage_is_left_out() {
        let event = ChatEvent::Done { ok: true, text: "x".into(), error: None, usage: None };
        assert!(serde_json::to_value(&event).unwrap().get("usage").is_none());
        let partial = r#"{"event":"result","result":{"status":"SUCCESS","response":"hi","usage":{"output_tokens":5}}}"#;
        let Some(ChatEvent::Done { usage: Some(usage), .. }) = parse_line(true, partial) else {
            panic!("expected agy usage");
        };
        assert_eq!(usage, Usage { output_tokens: Some(5), ..Default::default() });
    }

    #[test]
    fn codex_models_skip_hidden_and_merge_efforts() {
        let out = r#"{"models":[
            {"slug":"gpt-a","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]},
            {"slug":"gpt-hidden","visibility":"hide","supported_reasoning_levels":[{"effort":"ultra"}]},
            {"slug":"gpt-b","visibility":"list","supported_reasoning_levels":[{"effort":"high"},{"effort":"max"}]}]}"#;
        let catalog = parse_codex_models(out);
        assert_eq!(catalog.models, ["gpt-a", "gpt-b"]);
        assert_eq!(catalog.efforts, ["low", "high", "max"]);
    }

    #[test]
    fn codex_models_tolerate_garbage() {
        assert_eq!(parse_codex_models("not json"), ModelCatalog::default());
        assert_eq!(parse_codex_models(r#"{"models":3}"#), ModelCatalog::default());
    }

    #[test]
    fn agy_models_take_id_like_words_only() {
        let out = "Fetching available models...\n- gemini-3-pro  (default)\n* gemini-3-flash\nError: nope\ngemini-3-pro\n";
        assert_eq!(parse_agy_models(out).models, ["gemini-3-pro", "gemini-3-flash"]);
        assert!(parse_agy_models("Error: Please sign in to view available models.\n").models.is_empty());
    }
}
