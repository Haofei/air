use air_runtime::{ModelProvider, RuntimeError};
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
    pub base_url: String,
    pub api_key_env: String,
    pub model: String,

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
        if model.base_url.trim().is_empty() {
            return Err(invalid_config(
                path,
                format!("models.{alias}.base_url must not be empty"),
            ));
        }
        if !(model.base_url.starts_with("http://") || model.base_url.starts_with("https://")) {
            return Err(invalid_config(
                path,
                format!("models.{alias}.base_url must start with http:// or https://"),
            ));
        }
        if model.api_key_env.trim().is_empty() {
            return Err(invalid_config(
                path,
                format!("models.{alias}.api_key_env must not be empty"),
            ));
        }
        if model.model.trim().is_empty() {
            return Err(invalid_config(
                path,
                format!("models.{alias}.model must not be empty"),
            ));
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
        let url = chat_completions_url(&model_config.base_url);
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
            "model": model_config.model,
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
            return Err(RuntimeError::Provider(format!(
                "chat/completions failed with status {status}: {response_text}"
            )));
        }

        parse_chat_completion_content(&response_text)
    }
}

fn provider_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Provider(error.to_string())
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
    fn parse_config_file_reports_missing_model_aliases() {
        let path = temp_file_path("air-empty-model-config-diagnostic", "json");
        fs::write(&path, r#"{"models": {}}"#).unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("models must contain at least one alias"));
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
