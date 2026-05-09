use air_runtime::{sanitize_trace_text, ModelProvider, RuntimeError, TraceWriteOptions};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

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
    pub api_key_env: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub model_env: Option<String>,

    #[serde(default)]
    pub temperature: Option<f64>,

    #[serde(default)]
    pub system_prompt: Option<String>,
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
        if model.base_url.is_none() && model.base_url_env.is_none() {
            return Err(invalid_config(
                path,
                format!("models.{alias} must declare base_url or base_url_env"),
            ));
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
        if model.api_key_env.trim().is_empty() {
            return Err(invalid_config(
                path,
                format!("models.{alias}.api_key_env must not be empty"),
            ));
        }
        if model.model.trim().is_empty() && model.model_env.is_none() {
            return Err(invalid_config(
                path,
                format!("models.{alias} must declare model or model_env"),
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
    client: Client,
}

impl OpenAiCompatibleModelProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Result<Self, RuntimeError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(provider_error)?;

        Ok(Self { config, client })
    }
}

impl ModelProvider for OpenAiCompatibleModelProvider {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        let model_config = self
            .config
            .models
            .get(name)
            .ok_or_else(|| RuntimeError::Provider(format!("unknown model alias {name}")))?;
        let api_key = std::env::var(&model_config.api_key_env).map_err(|_| {
            RuntimeError::Provider(format!(
                "environment variable {} is not set",
                model_config.api_key_env
            ))
        })?;
        let base_url = resolve_base_url(model_config)?;
        let url = chat_completions_url(&base_url);
        let model_name = resolve_model_name(model_config)?;
        let content = input_to_content(input)?;

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

        if let Some(temperature) = model_config.temperature {
            body["temperature"] = json!(temperature);
        }

        let response = self
            .client
            .post(url)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .map_err(provider_error)?;

        let status = response.status();
        let response_text = response.text().map_err(provider_error)?;
        if !status.is_success() {
            return Err(provider_http_error(
                "chat/completions",
                status,
                &response_text,
            ));
        }

        parse_chat_completion_content(&response_text)
    }
}

fn provider_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Provider(error.to_string())
}

fn provider_http_error(
    operation: &str,
    status: reqwest::StatusCode,
    response_text: &str,
) -> RuntimeError {
    let body = sanitize_trace_text(
        response_text,
        &TraceWriteOptions {
            redact_sensitive: true,
            max_string_chars: Some(2048),
            max_event_bytes: None,
        },
    );
    RuntimeError::Provider(format!("{operation} failed with status {status}: {body}"))
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
    model_config
        .base_url
        .clone()
        .ok_or_else(|| RuntimeError::Provider("model base_url is not configured".to_string()))
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
    if !model_config.model.trim().is_empty() {
        return Ok(model_config.model.clone());
    }
    Err(RuntimeError::Provider(
        "model name is not configured".to_string(),
    ))
}

fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

fn input_to_content(input: &Value) -> Result<String, RuntimeError> {
    match input {
        Value::String(value) => Ok(value.clone()),
        other => serde_json::to_string(other).map_err(provider_error),
    }
}

fn parse_chat_completion_content(response_text: &str) -> Result<Value, RuntimeError> {
    let response: Value = serde_json::from_str(response_text).map_err(provider_error)?;
    let content = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeError::Provider("missing choices[0].message.content in response".to_string())
        })?;

    let normalized = strip_markdown_json_fence(content);

    match serde_json::from_str(normalized) {
        Ok(json) => Ok(json),
        Err(_) => Ok(json!({ "content": content })),
    }
}

fn strip_markdown_json_fence(content: &str) -> &str {
    let trimmed = content.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return content;
    };
    let Some(close_index) = after_open.rfind("```") else {
        return content;
    };

    let inner = &after_open[..close_index];
    let inner = inner.trim_start();
    let inner = inner
        .strip_prefix("json")
        .or_else(|| inner.strip_prefix("JSON"))
        .unwrap_or(inner);

    inner.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_content_as_content_object() {
        let value = parse_chat_completion_content(
            r#"{"choices":[{"message":{"content":"hello from model"}}]}"#,
        )
        .unwrap();

        assert_eq!(value, json!({"content": "hello from model"}));
    }

    #[test]
    fn parses_json_content_as_json() {
        let value = parse_chat_completion_content(
            r#"{"choices":[{"message":{"content":"{\"answer\":42}"}}]}"#,
        )
        .unwrap();

        assert_eq!(value, json!({"answer": 42}));
    }

    #[test]
    fn parses_fenced_json_content_as_json() {
        let value = parse_chat_completion_content(
            r#"{"choices":[{"message":{"content":"```json\n{\n  \"summary\": \"ok\",\n  \"risk_score\": 0.1\n}\n```"}}]}"#,
        )
        .unwrap();

        assert_eq!(value, json!({"summary": "ok", "risk_score": 0.1}));
    }

    #[test]
    fn leaves_non_json_fence_as_content_object() {
        let value = parse_chat_completion_content(
            r#"{"choices":[{"message":{"content":"```text\nnot json\n```"}}]}"#,
        )
        .unwrap();

        assert_eq!(value, json!({"content": "```text\nnot json\n```"}));
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
                  "api_key_env": "BIGMODEL_API_KEY",
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
    fn parse_config_file_requires_base_url_or_env() {
        let path = temp_file_path("air-model-config-missing-base-url", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "gpt-5.1"
                }
              }
            }"#,
        )
        .unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("models.planner"));
        assert!(message.contains("base_url or base_url_env"));
    }

    #[test]
    fn resolve_base_url_prefers_env_over_literal_url() {
        let env_name = format!("AIR_TEST_BASE_URL_{}", std::process::id());
        std::env::set_var(&env_name, "https://runtime.example/v1");

        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: Some(env_name.clone()),
            api_key_env: "OPENAI_API_KEY".to_string(),
            model: "gpt-5.1".to_string(),
            model_env: None,
            temperature: None,
            system_prompt: None,
        };

        assert_eq!(
            resolve_base_url(&config).unwrap(),
            "https://runtime.example/v1"
        );
        std::env::remove_var(env_name);
    }

    #[test]
    fn resolve_model_name_prefers_env_over_literal_model() {
        let env_name = format!("AIR_TEST_MODEL_{}", std::process::id());
        std::env::set_var(&env_name, "runtime-model");

        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: "OPENAI_API_KEY".to_string(),
            model: "configured-model".to_string(),
            model_env: Some(env_name.clone()),
            temperature: None,
            system_prompt: None,
        };

        assert_eq!(resolve_model_name(&config).unwrap(), "runtime-model");
        std::env::remove_var(env_name);
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
    fn provider_http_error_redacts_and_truncates_response_body() {
        let body = format!(
            "api_key=sk-live-secret Authorization: Bearer provider-secret {}",
            "x".repeat(4096)
        );
        let error =
            provider_http_error("chat/completions", reqwest::StatusCode::BAD_REQUEST, &body);
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
