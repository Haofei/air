use air_runtime::{sanitize_trace_text, ModelProvider, RuntimeError, TraceWriteOptions};
use async_openai::{config::OpenAIConfig, Client as OpenAiClient};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::runtime::{Builder, Runtime};

const DEFAULT_OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const DEFAULT_OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const DEFAULT_OPENAI_MODEL_ENV: &str = "OPENAI_MODEL";

struct ChatCompletionBody {
    body: Value,
    tool_name_map: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiCompatibleConfig {
    pub models: BTreeMap<String, OpenAiModelConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiModelConfig {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub base_url_env: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub model_env: Option<String>,

    #[serde(default)]
    pub temperature: Option<f64>,

    #[serde(default)]
    pub request_timeout_seconds: Option<u64>,

    #[serde(default)]
    pub system_prompt: Option<String>,

    #[serde(default)]
    pub json_mode: Option<bool>,

    #[serde(default)]
    pub response_format: Option<Value>,

    #[serde(default)]
    pub extra_body: Option<Value>,

    #[serde(default)]
    pub native_tool_calls: Option<bool>,
}

#[derive(Debug, Error)]
pub enum OpenAiConfigError {
    #[error("failed to read model config {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse model config JSON {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid model config {path}: {message}")]
    Invalid { path: String, message: String },
}

pub fn parse_config_file(
    path: impl AsRef<Path>,
) -> Result<OpenAiCompatibleConfig, OpenAiConfigError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| OpenAiConfigError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let config = serde_json::from_str(&source).map_err(|source| OpenAiConfigError::Json {
        path: path.display().to_string(),
        source,
    })?;
    validate_config(path, &config)?;
    Ok(config)
}

fn validate_config(path: &Path, config: &OpenAiCompatibleConfig) -> Result<(), OpenAiConfigError> {
    if config.models.is_empty() {
        return Err(invalid_config(
            path,
            "models must contain at least one alias",
        ));
    }

    for (alias, model) in &config.models {
        if alias.trim().is_empty() {
            return Err(invalid_config(path, "models contains an empty alias"));
        }
        if let Some(base_url) = &model.base_url {
            validate_base_url(path, alias, base_url)?;
        }
        if let Some(base_url_env) = &model.base_url_env {
            if base_url_env.trim().is_empty() {
                return Err(invalid_config(
                    path,
                    format!("models.{alias}.base_url_env must not be empty"),
                ));
            }
        }
        if matches!(model.api_key_env.as_deref(), Some(value) if value.trim().is_empty()) {
            return Err(invalid_config(
                path,
                format!("models.{alias}.api_key_env must not be empty"),
            ));
        }
        if let Some(model_env) = &model.model_env {
            if model_env.trim().is_empty() {
                return Err(invalid_config(
                    path,
                    format!("models.{alias}.model_env must not be empty"),
                ));
            }
        }
        if let Some(temperature) = model.temperature {
            if !(0.0..=2.0).contains(&temperature) {
                return Err(invalid_config(
                    path,
                    format!("models.{alias}.temperature must be between 0 and 2"),
                ));
            }
        }
        if matches!(model.request_timeout_seconds, Some(0)) {
            return Err(invalid_config(
                path,
                format!("models.{alias}.request_timeout_seconds must be at least 1"),
            ));
        }
        if model.json_mode == Some(true) && model.response_format.is_some() {
            return Err(invalid_config(
                path,
                format!("models.{alias} must not declare both json_mode and response_format"),
            ));
        }
        if let Some(response_format) = &model.response_format {
            if !response_format.is_object() {
                return Err(invalid_config(
                    path,
                    format!("models.{alias}.response_format must be a JSON object"),
                ));
            }
        }
        if let Some(extra_body) = &model.extra_body {
            if !extra_body.is_object() {
                return Err(invalid_config(
                    path,
                    format!("models.{alias}.extra_body must be a JSON object"),
                ));
            }
        }
    }

    Ok(())
}

fn validate_base_url(path: &Path, alias: &str, base_url: &str) -> Result<(), OpenAiConfigError> {
    if base_url.trim().is_empty() {
        return Err(invalid_config(
            path,
            format!("models.{alias}.base_url must not be empty"),
        ));
    }
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(invalid_config(
            path,
            format!("models.{alias}.base_url must start with http:// or https://"),
        ));
    }
    Ok(())
}

fn invalid_config(path: &Path, message: impl Into<String>) -> OpenAiConfigError {
    OpenAiConfigError::Invalid {
        path: path.display().to_string(),
        message: message.into(),
    }
}

#[derive(Clone)]
pub struct OpenAiCompatibleModelProvider {
    config: OpenAiCompatibleConfig,
    runtime: Arc<Runtime>,
}

impl OpenAiCompatibleModelProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Result<Self, RuntimeError> {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(provider_error)?;

        Ok(Self {
            config,
            runtime: Arc::new(runtime),
        })
    }

    fn call_model_with_request_timeout(
        &mut self,
        name: &str,
        input: &Value,
        action_timeout: Option<Duration>,
    ) -> Result<Value, RuntimeError> {
        let model_config = self
            .config
            .models
            .get(name)
            .ok_or_else(|| RuntimeError::Provider(format!("unknown model alias {name}")))?;
        let api_key_env = model_config
            .api_key_env
            .as_deref()
            .unwrap_or(DEFAULT_OPENAI_API_KEY_ENV);
        let api_key = std::env::var(api_key_env).map_err(|_| {
            RuntimeError::Provider(format!("environment variable {api_key_env} is not set"))
        })?;
        let base_url = resolve_base_url(model_config)?;
        let model_name = resolve_model_name(model_config)?;
        let content = input_to_content(input)?;
        let request = build_chat_completion_body(model_config, model_name, content, input)?;

        let request_timeout =
            effective_request_timeout(model_config.request_timeout_seconds, action_timeout);
        let client = openai_client(api_key, &base_url);

        let response = self.runtime.block_on(async {
            tokio::time::timeout(request_timeout, client.chat().create_byot(request.body))
                .await
                .map_err(|_| provider_timeout_error("chat/completions", request_timeout))?
                .map_err(provider_error)
        })?;

        parse_chat_completion_content(&response, &request.tool_name_map)
    }
}

impl ModelProvider for OpenAiCompatibleModelProvider {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        self.call_model_with_request_timeout(name, input, None)
    }

    fn call_model_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        self.call_model_with_request_timeout(name, input, Some(timeout))
    }
}

fn provider_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Provider(sanitize_provider_error_text(&error.to_string()))
}

fn provider_timeout_error(operation: &str, timeout: Duration) -> RuntimeError {
    RuntimeError::Provider(format!(
        "{operation} timed out after {}s",
        timeout.as_secs()
    ))
}

fn effective_request_timeout(
    configured_timeout_seconds: Option<u64>,
    action_timeout: Option<Duration>,
) -> Duration {
    let configured_timeout = Duration::from_secs(configured_timeout_seconds.unwrap_or(120));
    action_timeout
        .map(|timeout| timeout.min(configured_timeout))
        .unwrap_or(configured_timeout)
}

fn resolve_base_url(model_config: &OpenAiModelConfig) -> Result<String, RuntimeError> {
    if let Some(base_url_env) = &model_config.base_url_env {
        match std::env::var(base_url_env) {
            Ok(value) if !value.trim().is_empty() => return Ok(value),
            Ok(_) => {
                return Err(RuntimeError::Provider(format!(
                    "environment variable {base_url_env} is empty"
                )));
            }
            Err(std::env::VarError::NotPresent) => {}
            Err(error) => return Err(provider_error(error)),
        }
    }
    match std::env::var(DEFAULT_OPENAI_BASE_URL_ENV) {
        Ok(value) if !value.trim().is_empty() => return Ok(value),
        Ok(_) => {
            return Err(RuntimeError::Provider(format!(
                "environment variable {DEFAULT_OPENAI_BASE_URL_ENV} is empty"
            )));
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(error) => return Err(provider_error(error)),
    }
    model_config.base_url.clone().ok_or_else(|| {
        RuntimeError::Provider(
            "model base_url is not configured; set OPENAI_BASE_URL or model.base_url".to_string(),
        )
    })
}

fn resolve_model_name(model_config: &OpenAiModelConfig) -> Result<String, RuntimeError> {
    if let Some(model_env) = &model_config.model_env {
        match std::env::var(model_env) {
            Ok(value) if !value.trim().is_empty() => return Ok(value),
            Ok(_) => {
                return Err(RuntimeError::Provider(format!(
                    "environment variable {model_env} is empty"
                )));
            }
            Err(std::env::VarError::NotPresent) => {}
            Err(error) => return Err(provider_error(error)),
        }
    }
    match std::env::var(DEFAULT_OPENAI_MODEL_ENV) {
        Ok(value) if !value.trim().is_empty() => return Ok(value),
        Ok(_) => {
            return Err(RuntimeError::Provider(format!(
                "environment variable {DEFAULT_OPENAI_MODEL_ENV} is empty"
            )));
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(error) => return Err(provider_error(error)),
    }
    if !model_config.model.trim().is_empty() {
        return Ok(model_config.model.clone());
    }
    Err(RuntimeError::Provider(
        "model name is not configured; set OPENAI_MODEL or model.model".to_string(),
    ))
}

fn openai_client(api_key: String, base_url: &str) -> OpenAiClient<OpenAIConfig> {
    let config = OpenAIConfig::new()
        .with_api_key(api_key)
        .with_api_base(base_url.trim_end_matches('/'));
    OpenAiClient::with_config(config)
}

fn build_chat_completion_body(
    model_config: &OpenAiModelConfig,
    model_name: String,
    content: String,
    input: &Value,
) -> Result<ChatCompletionBody, RuntimeError> {
    let mut messages = Vec::new();
    if let Some(system_prompt) = &model_config.system_prompt {
        messages.push(json!({
            "role": "system",
            "content": system_prompt
        }));
    }
    messages.push(json!({
        "role": "user",
        "content": content
    }));

    let mut body = json!({
        "model": model_name,
        "messages": messages
    });

    if let Some(extra_body) = &model_config.extra_body {
        merge_extra_body(&mut body, extra_body);
    }
    if let Some(temperature) = model_config.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(response_format) = resolved_response_format(model_config) {
        body["response_format"] = response_format;
    }

    let tool_name_map = if model_config.native_tool_calls == Some(true) {
        attach_native_tool_calls(&mut body, input)?
    } else {
        BTreeMap::new()
    };

    Ok(ChatCompletionBody {
        body,
        tool_name_map,
    })
}

fn attach_native_tool_calls(
    body: &mut Value,
    input: &Value,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    let Some(allowed_tools) = input.get("allowed_tools").and_then(Value::as_array) else {
        return Ok(BTreeMap::new());
    };
    let tool_schemas = input.get("tool_schemas").and_then(Value::as_object);
    let mut tools = Vec::new();
    let mut tool_name_map = BTreeMap::new();
    let mut used_names = BTreeMap::new();

    for tool in allowed_tools {
        let Some(original_name) = tool.as_str() else {
            continue;
        };
        if original_name.trim().is_empty() {
            continue;
        }

        let safe_name = safe_openai_tool_name(original_name, &mut used_names);
        tool_name_map.insert(safe_name.clone(), original_name.to_string());
        let schema = tool_schemas.and_then(|schemas| schemas.get(original_name));
        tools.push(json!({
            "type": "function",
            "function": {
                "name": safe_name,
                "description": native_tool_description(original_name, schema),
                "parameters": native_tool_parameters(schema),
                "strict": false
            }
        }));
    }

    if tools.is_empty() {
        return Ok(BTreeMap::new());
    }

    let Some(body) = body.as_object_mut() else {
        return Err(RuntimeError::Provider(
            "chat completion request body is not a JSON object".to_string(),
        ));
    };
    body.insert("tools".to_string(), Value::Array(tools));
    body.insert("tool_choice".to_string(), json!("auto"));

    Ok(tool_name_map)
}

fn safe_openai_tool_name(original: &str, used_names: &mut BTreeMap<String, usize>) -> String {
    let mut base = original
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if base.is_empty() {
        base.push_str("tool");
    }
    if base.len() > 64 {
        base.truncate(64);
    }

    let count = used_names.entry(base.clone()).or_insert(0);
    let name = if *count == 0 {
        base
    } else {
        let suffix = format!("__{}", *count);
        let max_base_len = 64usize.saturating_sub(suffix.len());
        format!("{}{}", &base[..base.len().min(max_base_len)], suffix)
    };
    *count += 1;
    name
}

fn native_tool_description(original_name: &str, schema: Option<&Value>) -> String {
    let Some(schema) = schema else {
        return format!("AIR tool {original_name}");
    };
    let mut parts = vec![format!("AIR tool {original_name}")];
    if let Some(required) = schema.get("required") {
        parts.push(format!("required: {}", compact_json(required)));
    }
    if let Some(optional) = schema.get("optional") {
        parts.push(format!("optional: {}", compact_json(optional)));
    }
    parts.join("; ")
}

fn native_tool_parameters(schema: Option<&Value>) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required_names = Vec::new();
    if let Some(schema) = schema {
        if let Some(required) = schema.get("required").and_then(Value::as_object) {
            for (name, description) in required {
                required_names.push(Value::String(name.clone()));
                properties.insert(
                    name.clone(),
                    json!({"description": schema_description(description)}),
                );
            }
        }
        if let Some(optional) = schema.get("optional").and_then(Value::as_object) {
            for (name, description) in optional {
                properties
                    .entry(name.clone())
                    .or_insert_with(|| json!({"description": schema_description(description)}));
            }
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required_names,
        "additionalProperties": true
    })
}

fn schema_description(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| compact_json(value))
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn resolved_response_format(model_config: &OpenAiModelConfig) -> Option<Value> {
    if let Some(response_format) = &model_config.response_format {
        return Some(response_format.clone());
    }
    if model_config.json_mode == Some(true) {
        return Some(json!({"type": "json_object"}));
    }
    None
}

fn merge_extra_body(body: &mut Value, extra_body: &Value) {
    let (Some(body), Some(extra_body)) = (body.as_object_mut(), extra_body.as_object()) else {
        return;
    };
    for (key, value) in extra_body {
        body.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

fn input_to_content(input: &Value) -> Result<String, RuntimeError> {
    match input {
        Value::String(value) => Ok(value.clone()),
        other => serde_json::to_string(other).map_err(provider_error),
    }
}

fn parse_chat_completion_content(
    response: &Value,
    tool_name_map: &BTreeMap<String, String>,
) -> Result<Value, RuntimeError> {
    if let Some(tool_calls) = parse_native_tool_calls(response, tool_name_map)? {
        return Ok(tool_calls);
    }

    let content = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeError::Provider("missing choices[0].message.content in response".to_string())
        })?;

    let normalized = strip_markdown_code_fence(content).unwrap_or(content);

    match serde_json::from_str(normalized) {
        Ok(json) => Ok(json),
        Err(_) => Ok(json!({ "content": normalized })),
    }
}

fn parse_native_tool_calls(
    response: &Value,
    tool_name_map: &BTreeMap<String, String>,
) -> Result<Option<Value>, RuntimeError> {
    if tool_name_map.is_empty() {
        return Ok(None);
    }
    let Some(tool_calls) = response
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    if tool_calls.is_empty() {
        return Ok(None);
    }

    let mut normalized_calls = Vec::new();
    for call in tool_calls {
        let Some(safe_name) = call.pointer("/function/name").and_then(Value::as_str) else {
            continue;
        };
        let original_name = tool_name_map.get(safe_name).cloned().ok_or_else(|| {
            RuntimeError::Provider(format!(
                "model returned undeclared native tool call {safe_name}"
            ))
        })?;
        let arguments = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}");
        let input: Value = serde_json::from_str(arguments).map_err(|error| {
            RuntimeError::Provider(format!(
                "invalid native tool call arguments for {original_name}: {error}"
            ))
        })?;
        let input = normalize_native_tool_arguments(input);
        normalized_calls.push(json!({
            "tool": original_name,
            "input": input
        }));
    }
    if normalized_calls.is_empty() {
        return Err(RuntimeError::Provider(
            "native tool call response did not contain function tool calls".to_string(),
        ));
    }

    let rationale = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty())
        .unwrap_or("model requested native tool calls");

    Ok(Some(json!({
        "complete": false,
        "tool_calls": normalized_calls,
        "rationale": rationale
    })))
}

fn normalize_native_tool_arguments(value: Value) -> Value {
    match value {
        Value::String(text) => normalize_native_tool_argument_string(text),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(normalize_native_tool_arguments)
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, normalize_native_tool_arguments(value)))
                .collect(),
        ),
        other => other,
    }
}

fn normalize_native_tool_argument_string(text: String) -> Value {
    let trimmed = text.trim();
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
            return normalize_native_tool_arguments(parsed);
        }
    }
    if trimmed == "true" {
        return Value::Bool(true);
    }
    if trimmed == "false" {
        return Value::Bool(false);
    }
    let looks_integer = trimmed
        .strip_prefix('-')
        .unwrap_or(trimmed)
        .chars()
        .all(|character| character.is_ascii_digit());
    if looks_integer {
        if let Ok(number) = trimmed.parse::<i64>() {
            return json!(number);
        }
    }
    Value::String(text)
}

fn sanitize_provider_error_text(error: &str) -> String {
    sanitize_trace_text(
        error,
        &TraceWriteOptions {
            redact_sensitive: true,
            max_string_chars: Some(2048),
            max_event_bytes: None,
        },
    )
}

fn strip_markdown_code_fence(content: &str) -> Option<&str> {
    let trimmed = content.trim();
    let after_open = trimmed.strip_prefix("```")?;
    let close_index = after_open.rfind("```")?;

    let inner = &after_open[..close_index];
    let inner = inner.trim_start();
    let inner = match inner.find('\n') {
        Some(index) => {
            let language = inner[..index].trim();
            if language.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            }) {
                &inner[index + 1..]
            } else {
                inner
            }
        }
        None => inner,
    };

    Some(inner.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_content_as_content_object() {
        let value = parse_chat_completion_content(
            &json!({
                "choices": [{"message": {"content": "hello from model"}}]
            }),
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(value, json!({"content": "hello from model"}));
    }

    #[test]
    fn parses_json_content_as_json() {
        let value = parse_chat_completion_content(
            &json!({
                "choices": [{"message": {"content": "{\"answer\":42}"}}]
            }),
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(value, json!({"answer": 42}));
    }

    #[test]
    fn parses_fenced_json_content_as_json() {
        let value = parse_chat_completion_content(&json!({
            "choices": [{
                "message": {
                    "content": "```json\n{\n  \"summary\": \"ok\",\n  \"risk_score\": 0.1\n}\n```"
                }
            }]
        }), &BTreeMap::new())
        .unwrap();

        assert_eq!(value, json!({"summary": "ok", "risk_score": 0.1}));
    }

    #[test]
    fn strips_non_json_fence_as_content_object() {
        let value = parse_chat_completion_content(
            &json!({
                "choices": [{"message": {"content": "```text\nnot json\n```"}}]
            }),
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(value, json!({"content": "not json"}));
    }

    #[test]
    fn parse_config_file_reports_invalid_model_config_field() {
        let path = temp_file_path("air-model-config-diagnostic", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "open.bigmodel.cn/api/coding/paas/v4",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "GLM-5.1"
                }
              }
            }"#,
        )
        .unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("models.planner.base_url"));
        assert!(message.contains("http:// or https://"));
    }

    #[test]
    fn parse_config_file_accepts_base_url_env_without_literal_url() {
        let path = temp_file_path("air-model-config-base-url-env", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url_env": "OPENAI_BASE_URL",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "gpt-5.1"
                }
              }
            }"#,
        )
        .unwrap();

        let config = parse_config_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(
            config.models["planner"].base_url_env.as_deref(),
            Some("OPENAI_BASE_URL")
        );
        assert_eq!(config.models["planner"].base_url, None);
    }

    #[test]
    fn parse_config_file_accepts_model_env_without_literal_model() {
        let path = temp_file_path("air-model-config-model-env", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model_env": "OPENAI_MODEL"
                }
              }
            }"#,
        )
        .unwrap();

        let config = parse_config_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(
            config.models["planner"].model_env.as_deref(),
            Some("OPENAI_MODEL")
        );
        assert_eq!(config.models["planner"].model, "");
    }

    #[test]
    fn parse_config_file_accepts_request_timeout_seconds() {
        let path = temp_file_path("air-model-config-timeout", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "gpt-5.1",
                  "request_timeout_seconds": 7
                }
              }
            }"#,
        )
        .unwrap();

        let config = parse_config_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(config.models["planner"].request_timeout_seconds, Some(7));
    }

    #[test]
    fn parse_config_file_rejects_zero_request_timeout_seconds() {
        let path = temp_file_path("air-model-config-timeout-zero", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "gpt-5.1",
                  "request_timeout_seconds": 0
                }
              }
            }"#,
        )
        .unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("models.planner.request_timeout_seconds"));
        assert!(message.contains("at least 1"));
    }

    #[test]
    fn json_mode_resolves_to_openai_json_object_response_format() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "gpt-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: Some(true),
            response_format: None,
            extra_body: None,
            native_tool_calls: None,
        };

        assert_eq!(
            resolved_response_format(&config),
            Some(json!({"type": "json_object"}))
        );
    }

    #[test]
    fn custom_response_format_takes_effect() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "gpt-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: None,
            response_format: Some(json!({
                "type": "json_schema",
                "json_schema": {"name": "decision", "schema": {"type": "object"}}
            })),
            extra_body: None,
            native_tool_calls: None,
        };

        assert_eq!(
            resolved_response_format(&config),
            Some(json!({
                "type": "json_schema",
                "json_schema": {"name": "decision", "schema": {"type": "object"}}
            }))
        );
    }

    #[test]
    fn parse_config_file_rejects_ambiguous_response_format_config() {
        let path = temp_file_path("air-model-config-response-format-ambiguous", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "gpt-5.1",
                  "json_mode": true,
                  "response_format": {"type": "json_object"}
                }
              }
            }"#,
        )
        .unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("models.planner"));
        assert!(message.contains("json_mode"));
        assert!(message.contains("response_format"));
    }

    #[test]
    fn parse_config_file_rejects_non_object_response_format() {
        let path = temp_file_path("air-model-config-response-format-invalid", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "gpt-5.1",
                  "response_format": "json"
                }
              }
            }"#,
        )
        .unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("models.planner.response_format"));
        assert!(message.contains("JSON object"));
    }

    #[test]
    fn parse_config_file_accepts_extra_body_object() {
        let path = temp_file_path("air-model-config-extra-body", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "glm-5.1",
                  "extra_body": {
                    "thinking": {
                      "type": "enabled",
                      "clear_thinking": false
                    }
                  }
                }
              }
            }"#,
        )
        .unwrap();

        let config = parse_config_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(
            config.models["planner"].extra_body,
            Some(json!({
                "thinking": {
                    "type": "enabled",
                    "clear_thinking": false
                }
            }))
        );
    }

    #[test]
    fn parse_config_file_rejects_non_object_extra_body() {
        let path = temp_file_path("air-model-config-extra-body-invalid", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "glm-5.1",
                  "extra_body": "thinking"
                }
              }
            }"#,
        )
        .unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("models.planner.extra_body"));
        assert!(message.contains("JSON object"));
    }

    #[test]
    fn extra_body_merges_provider_specific_request_fields() {
        let mut body = json!({
            "model": "glm-5.1",
            "messages": []
        });

        merge_extra_body(
            &mut body,
            &json!({
                "thinking": {
                    "type": "enabled",
                    "clear_thinking": false
                },
                "model": "overridden"
            }),
        );

        assert_eq!(
            body["thinking"],
            json!({
                "type": "enabled",
                "clear_thinking": false
            })
        );
        assert_eq!(body["model"], json!("glm-5.1"));
    }

    #[test]
    fn native_tool_calls_attach_openai_function_tools_when_enabled() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "glm-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
        };
        let request = build_chat_completion_body(
            &config,
            "glm-5.1".to_string(),
            "choose tools".to_string(),
            &json!({
                "allowed_tools": ["file.ops", "repo.search"],
                "tool_schemas": {
                    "file.ops": {
                        "required": {"operations": "array of edit operations"},
                        "optional": {"dry_run": "boolean"}
                    },
                    "repo.search": {
                        "required": {"query": "search text"}
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(request.tool_name_map["file_ops"], "file.ops");
        assert_eq!(request.tool_name_map["repo_search"], "repo.search");
        assert_eq!(request.body["tool_choice"], json!("auto"));
        assert_eq!(
            request.body["tools"][0]["function"]["parameters"]["required"],
            json!(["operations"])
        );
    }

    #[test]
    fn native_tool_calls_parse_to_air_decision_shape() {
        let mut tool_name_map = BTreeMap::new();
        tool_name_map.insert("file_ops".to_string(), "file.ops".to_string());

        let value = parse_chat_completion_content(
            &json!({
                "choices": [{
                    "message": {
                        "content": "Need an edit.",
                        "tool_calls": [{
                            "type": "function",
                            "function": {
                                "name": "file_ops",
                                "arguments": "{\"operations\":[{\"kind\":\"edit\"}]}"
                            }
                        }]
                    }
                }]
            }),
            &tool_name_map,
        )
        .unwrap();

        assert_eq!(
            value,
            json!({
                "complete": false,
                "tool_calls": [{
                    "tool": "file.ops",
                    "input": {
                        "operations": [{"kind": "edit"}]
                    }
                }],
                "rationale": "Need an edit."
            })
        );
    }

    #[test]
    fn native_tool_calls_normalize_stringified_arguments() {
        let mut tool_name_map = BTreeMap::new();
        tool_name_map.insert("repo_symbols".to_string(), "repo.symbols".to_string());

        let value = parse_chat_completion_content(
            &json!({
                "choices": [{
                    "message": {
                        "tool_calls": [{
                            "type": "function",
                            "function": {
                                "name": "repo_symbols",
                                "arguments": "{\"names\":\"[\\\"call_repo_references_tool\\\"]\",\"max_symbols\":\"3\",\"line_numbers\":\"true\"}"
                            }
                        }]
                    }
                }]
            }),
            &tool_name_map,
        )
        .unwrap();

        assert_eq!(
            value["tool_calls"][0]["input"],
            json!({
                "names": ["call_repo_references_tool"],
                "max_symbols": 3,
                "line_numbers": true
            })
        );
    }

    #[test]
    fn effective_request_timeout_uses_smaller_action_deadline() {
        assert_eq!(
            effective_request_timeout(Some(30), Some(Duration::from_secs(5))),
            Duration::from_secs(5)
        );
        assert_eq!(
            effective_request_timeout(Some(5), Some(Duration::from_secs(30))),
            Duration::from_secs(5)
        );
        assert_eq!(
            effective_request_timeout(None, None),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn parse_config_file_accepts_default_openai_env_contract() {
        let path = temp_file_path("air-model-config-default-openai-env", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "temperature": 0
                }
              }
            }"#,
        )
        .unwrap();

        let config = parse_config_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(config.models["planner"].api_key_env, None);
        assert_eq!(config.models["planner"].base_url_env, None);
        assert_eq!(config.models["planner"].model, "");
        assert_eq!(config.models["planner"].temperature, Some(0.0));
    }

    #[test]
    fn resolve_base_url_prefers_env_over_literal_url() {
        let env_name = format!("AIR_TEST_BASE_URL_{}", std::process::id());
        std::env::set_var(&env_name, "https://runtime.example/v1");

        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: Some(env_name.clone()),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "gpt-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: None,
        };

        assert_eq!(
            resolve_base_url(&config).unwrap(),
            "https://runtime.example/v1"
        );
        std::env::remove_var(env_name);
    }

    #[test]
    fn resolve_base_url_uses_default_openai_base_url_env() {
        std::env::set_var(DEFAULT_OPENAI_BASE_URL_ENV, "https://runtime.example/v1");

        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: None,
            model: "gpt-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: None,
        };

        assert_eq!(
            resolve_base_url(&config).unwrap(),
            "https://runtime.example/v1"
        );
        std::env::remove_var(DEFAULT_OPENAI_BASE_URL_ENV);
    }

    #[test]
    fn resolve_model_name_prefers_env_over_literal_model() {
        let env_name = format!("AIR_TEST_MODEL_{}", std::process::id());
        std::env::set_var(&env_name, "runtime-model");

        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "configured-model".to_string(),
            model_env: Some(env_name.clone()),
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: None,
        };

        assert_eq!(resolve_model_name(&config).unwrap(), "runtime-model");
        std::env::remove_var(env_name);
    }

    #[test]
    fn resolve_model_name_uses_default_openai_model_env() {
        std::env::set_var(DEFAULT_OPENAI_MODEL_ENV, "runtime-model");

        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: None,
            model: "configured-model".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: None,
        };

        assert_eq!(resolve_model_name(&config).unwrap(), "runtime-model");
        std::env::remove_var(DEFAULT_OPENAI_MODEL_ENV);
    }

    #[test]
    fn parse_config_file_reports_missing_model_aliases() {
        let path = temp_file_path("air-empty-model-config-diagnostic", "json");
        fs::write(&path, r#"{"models": {}}"#).unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("models must contain at least one alias"));
    }

    #[test]
    fn provider_error_redacts_and_truncates_response_body() {
        let body = format!(
            "api_key=sk-live-secret Authorization: Bearer provider-secret {}",
            "x".repeat(4096)
        );
        let error = provider_error(&body);
        let message = error.to_string();

        assert!(!message.contains("sk-live-secret"));
        assert!(!message.contains("provider-secret"));
        assert!(message.contains("[AIR_REDACTED]"));
        assert!(message.contains("[AIR_TRUNCATED]"));
        assert!(message.len() < body.len());
    }

    fn temp_file_path(prefix: &str, extension: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }
}
