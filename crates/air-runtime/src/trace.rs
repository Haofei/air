use crate::{truncate_middle_context_string, RuntimeError, State};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub agent: String,
    pub step: u32,
    pub rule: String,
    pub action: String,

    #[serde(default)]
    pub input: Option<Value>,

    #[serde(default)]
    pub output: Option<Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,

    pub status: TraceStatus,

    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceWriteOptions {
    pub redact_sensitive: bool,
    pub max_string_chars: Option<usize>,
    pub max_event_bytes: Option<usize>,
}

impl Default for TraceWriteOptions {
    fn default() -> Self {
        Self::redacted()
    }
}

impl TraceWriteOptions {
    pub fn raw() -> Self {
        Self {
            redact_sensitive: false,
            max_string_chars: None,
            max_event_bytes: None,
        }
    }

    pub fn redacted() -> Self {
        Self {
            redact_sensitive: true,
            max_string_chars: Some(4096),
            max_event_bytes: Some(64 * 1024),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    Ok,
    Error,
}

pub fn system_return_event(outputs: State) -> TraceEvent {
    TraceEvent {
        agent: "$system".to_string(),
        step: 0,
        rule: "system".to_string(),
        action: "return".to_string(),
        input: None,
        output: Some(Value::Object(outputs)),
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    }
}

pub fn write_trace_jsonl(
    path: impl AsRef<Path>,
    events: &[TraceEvent],
) -> Result<(), RuntimeError> {
    write_trace_jsonl_with_options(path, events, &TraceWriteOptions::default())
}

pub fn write_trace_jsonl_with_options(
    path: impl AsRef<Path>,
    events: &[TraceEvent],
    options: &TraceWriteOptions,
) -> Result<(), RuntimeError> {
    let mut file = File::create(path).map_err(|error| RuntimeError::TraceIo(error.to_string()))?;
    for event in events {
        let event = sanitize_trace_event(event, options);
        let mut line = serde_json::to_string(&event)
            .map_err(|error| RuntimeError::TraceJson(error.to_string()))?;
        if let Some(max_bytes) = options.max_event_bytes {
            if line.len() > max_bytes {
                line = serde_json::to_string(&compact_trace_event(&event))
                    .map_err(|error| RuntimeError::TraceJson(error.to_string()))?;
            }
        }
        writeln!(file, "{line}").map_err(|error| RuntimeError::TraceIo(error.to_string()))?;
    }
    Ok(())
}

pub fn read_trace_jsonl(path: impl AsRef<Path>) -> Result<Vec<TraceEvent>, RuntimeError> {
    let file = File::open(path).map_err(|error| RuntimeError::TraceIo(error.to_string()))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for line in reader.lines() {
        let line = line.map_err(|error| RuntimeError::TraceIo(error.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str(&line)
            .map_err(|error| RuntimeError::TraceJson(error.to_string()))?;
        events.push(event);
    }

    Ok(events)
}

pub fn replay_outputs(events: &[TraceEvent]) -> Result<Value, RuntimeError> {
    events
        .iter()
        .rev()
        .find(|event| {
            event.status == TraceStatus::Ok
                && event.action == "return"
                && (event.agent == "$system" || event.output.is_some())
        })
        .and_then(|event| event.output.clone())
        .ok_or(RuntimeError::TraceMissingOutput)
}

pub fn sanitize_trace_event(event: &TraceEvent, options: &TraceWriteOptions) -> TraceEvent {
    let mut sanitized = event.clone();
    sanitized.input = sanitized
        .input
        .as_ref()
        .map(|value| sanitize_trace_value(value, options));
    sanitized.output = sanitized
        .output
        .as_ref()
        .map(|value| sanitize_trace_value(value, options));
    sanitized.meta = sanitized
        .meta
        .as_ref()
        .map(|value| sanitize_trace_value(value, options));
    if let Some(error) = &sanitized.error {
        sanitized.error = Some(sanitize_trace_text(error, options));
    }
    sanitized
}

fn sanitize_trace_value(value: &Value, options: &TraceWriteOptions) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let value = if options.redact_sensitive && is_sensitive_key(key) {
                        Value::String("[AIR_REDACTED]".to_string())
                    } else {
                        sanitize_trace_value(value, options)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| sanitize_trace_value(value, options))
                .collect(),
        ),
        Value::String(text) => Value::String(sanitize_trace_text(text, options)),
        _ => value.clone(),
    }
}

pub fn sanitize_trace_text(text: &str, options: &TraceWriteOptions) -> String {
    let mut text = if options.redact_sensitive {
        mask_sensitive_text(text)
    } else {
        text.to_string()
    };
    if let Some(max_chars) = options.max_string_chars {
        if text.chars().count() > max_chars {
            let truncated = truncate_middle_context_string(&text, max_chars, "AIR_TRUNCATED");
            text = if truncated.contains("[AIR_TRUNCATED]") {
                truncated
            } else {
                "[AIR_TRUNCATED]".to_string()
            };
        }
    }
    text
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "apikey"
            | "apiaccesskey"
            | "authorization"
            | "cookie"
            | "setcookie"
            | "password"
            | "passwd"
            | "pwd"
            | "secret"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "clientsecret"
    )
}

fn mask_sensitive_text(text: &str) -> String {
    let mut output = text.to_string();
    for marker in [
        "Bearer ",
        "authorization: ",
        "Authorization: ",
        "api_key=",
        "api-key=",
        "apikey=",
        "password=",
        "secret=",
        "token=",
    ] {
        output = mask_after_marker(&output, marker);
    }
    output
}

fn mask_after_marker(text: &str, marker: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        output.push_str(&rest[..index + marker.len()]);
        output.push_str("[AIR_REDACTED]");
        let value_start = index + marker.len();
        let value = &rest[value_start..];
        let value_end = value
            .find(|ch: char| ch.is_whitespace() || ch == '&' || ch == '"' || ch == '\'')
            .unwrap_or(value.len());
        rest = &value[value_end..];
    }
    output.push_str(rest);
    output
}

pub(crate) fn compact_trace_event(event: &TraceEvent) -> TraceEvent {
    let mut compact = event.clone();
    compact.input = compact.input.as_ref().map(compact_trace_payload);
    compact.output = compact.output.as_ref().map(compact_trace_payload);
    compact.meta = compact.meta.as_ref().map(compact_trace_meta);
    compact.error = compact
        .error
        .as_ref()
        .map(|_| "[AIR_TRUNCATED]".to_string());
    compact
}

fn compact_trace_payload(payload: &Value) -> Value {
    let Value::Object(object) = payload else {
        return json!({"_air_truncated": true});
    };
    let mut compact = Map::new();
    compact.insert("_air_truncated".to_string(), Value::Bool(true));
    for (key, value) in object {
        let Some(value) = compact_trace_meta_value(value) else {
            compact.insert(key.clone(), json!({"_air_truncated": true}));
            continue;
        };
        compact.insert(key.clone(), value);
    }
    Value::Object(compact)
}

fn compact_trace_meta(meta: &Value) -> Value {
    let Value::Object(object) = meta else {
        return json!({"_air_truncated": true});
    };
    let mut compact = Map::new();
    compact.insert("_air_truncated".to_string(), Value::Bool(true));
    for (key, value) in object {
        let Some(value) = compact_trace_meta_value(value) else {
            continue;
        };
        compact.insert(key.clone(), value);
    }
    Value::Object(compact)
}

fn compact_trace_meta_value(value: &Value) -> Option<Value> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Some(value.clone()),
        Value::Array(values) => {
            let mut compact = Vec::new();
            for value in values.iter().take(32) {
                let Some(value) = compact_trace_meta_value(value) else {
                    return Some(json!({"_air_truncated": true}));
                };
                compact.push(value);
            }
            if values.len() > compact.len() {
                compact.push(json!({"_air_truncated": true}));
            }
            Some(Value::Array(compact))
        }
        Value::Object(_) => None,
    }
}
