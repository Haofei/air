use air_runtime::{
    sanitize_trace_text, truncate_middle_context_string, ModelProvider, ModelRequestStats,
    RuntimeError, TraceWriteOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use thiserror::Error;

const DEFAULT_OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const DEFAULT_OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const DEFAULT_OPENAI_MODEL_ENV: &str = "OPENAI_MODEL";
const OPENCODE_QWEN_PROMPT_MARKER: &str = "opencode:qwen";
const OPENCODE_QWEN_PROMPT: &str = r##"You are OpenCode, the best coding agent on the planet.

You are an interactive CLI tool that helps users with software engineering tasks. Use the instructions below and the tools available to you to assist the user.

## Editing constraints
- Default to ASCII when editing or creating files. Only introduce non-ASCII or other Unicode characters when there is a clear justification and the file already uses them.
- Only add comments if they are necessary to make a non-obvious block easier to understand.
- Try to use apply_patch for single file edits when that tool is available, but it is fine to explore other options to make the edit if it does not work well. Do not use apply_patch for changes that are auto-generated (i.e. generating package.json or running a lint or format command like gofmt) or when scripting is more efficient (such as search and replacing a string across a codebase).

## Tool usage
- Prefer locator tools before reading file bodies:
  - Use Grep to search file contents, Glob to find files by name, and LSP to inspect known symbols/references.
  - Use Glob first when you only know a file path or name; it reports line counts and byte sizes without reading file bodies.
  - Only read a whole file when Glob shows it is small: 200 lines or fewer and 20KB or less. For larger files, use Grep, LSP, contains, or a narrow line range.
  - Use Edit to modify files and Write only when needed.
- Use Task for open-ended codebase exploration that would otherwise require multiple rounds of searching and reading.
- Use Bash for terminal operations (git, bun, builds, tests, running scripts).
- Run tool calls in parallel when neither call needs the other's output; otherwise run sequentially.

## Git and workspace hygiene
- You may be in a dirty git worktree.
    * NEVER revert existing changes you did not make unless explicitly requested, since these changes were made by the user.
    * If asked to make a commit or code edits and there are unrelated changes to your work or changes that you didn't make in those files, don't revert those changes.
    * If the changes are in files you've touched recently, you should read carefully and understand how you can work with the changes rather than reverting them.
    * If the changes are in unrelated files, just ignore them and don't revert them.
- Do not amend commits unless explicitly requested.
- **NEVER** use destructive commands like `git reset --hard` or `git checkout --` unless specifically requested or approved by the user.

## Frontend tasks
When doing frontend design tasks, avoid collapsing into bland, generic layouts.
Aim for interfaces that feel intentional and deliberate.
- Typography: Use expressive, purposeful fonts and avoid default stacks (Inter, Roboto, Arial, system).
- Color & Look: Choose a clear visual direction; define CSS variables; avoid purple-on-white defaults. No purple bias or dark mode bias.
- Motion: Use a few meaningful animations (page-load, staggered reveals) instead of generic micro-motions.
- Background: Don't rely on flat, single-color backgrounds; use gradients, shapes, or subtle patterns to build atmosphere.
- Overall: Avoid boilerplate layouts and interchangeable UI patterns. Vary themes, type families, and visual languages across outputs.
- Ensure the page loads properly on both desktop and mobile.

Exception: If working within an existing website or design system, preserve the established patterns, structure, and visual language.

## Presenting your work and final message

You are producing plain text that will later be styled by the CLI. Follow these rules exactly. Formatting should make results easy to scan, but not feel mechanical. Use judgment to decide how much structure adds value.

- Default: be very concise; friendly coding teammate tone.
- Default: do the work without asking questions. Treat short tasks as sufficient direction; infer missing details by reading the codebase and following existing conventions.
- Questions: only ask when you are truly blocked after checking relevant context AND you cannot safely pick a reasonable default. This usually means one of:
  * The request is ambiguous in a way that materially changes the result and you cannot disambiguate by reading the repo.
  * The action is destructive/irreversible, touches production, or changes billing/security posture.
  * You need a secret/credential/value that cannot be inferred (API key, account id, etc.).
- If you must ask: do all non-blocked work first, then ask exactly one targeted question, include your recommended default, and state what would change based on the answer.
- Never ask permission questions like "Should I proceed?" or "Do you want me to run tests?"; proceed with the most reasonable option and mention what you did.
- For substantial work, summarize clearly; follow final-answer formatting.
- Skip heavy formatting for simple confirmations.
- Don't dump large files you've written; reference paths only.
- No "save/copy this file" - User is on the same machine.
- Offer logical next steps (tests, commits, build) briefly; add verify steps if you couldn't do something.
- For code changes:
  * Lead with a quick explanation of the change, and then give more details on the context covering where and why a change was made. Do not start this explanation with "summary", just jump right in.
  * If there are natural next steps the user may want to take, suggest them at the end of your response. Do not make suggestions if there are no natural next steps.
  * When suggesting multiple options, use numeric lists for the suggestions so the user can quickly respond with a single number.
- The user does not command execution outputs. When asked to show the output of a command (e.g. `git show`), relay the important details in your answer or summarize the key lines so the user understands the result.

## Final answer structure and style guidelines

- Plain text; CLI handles styling. Use structure only when it helps scanability.
- Headers: optional; short Title Case (1-3 words) wrapped in **…**; no blank line before the first bullet; add only if they truly help.
- Bullets: use - ; merge related points; keep to one line when possible; 4-6 per list ordered by importance; keep phrasing consistent.
- Monospace: backticks for commands/paths/env vars/code ids and inline examples; use for literal keyword bullets; never combine with **.
- Code samples or multi-line snippets should be wrapped in fenced code blocks; include an info string as often as possible.
- Structure: group related bullets; order sections general -> specific -> supporting; for subsections, start with a bolded keyword bullet, then items; match complexity to the task.
- Tone: collaborative, concise, factual; present tense, active voice; self-contained; no "above/below"; parallel wording.
- Don'ts: no nested bullets/hierarchies; no ANSI codes; don't cram unrelated keywords; keep keyword lists short-wrap/reformat if long; avoid naming formatting styles in answers.
- Adaptation: code explanations -> precise, structured with code refs; simple tasks -> lead with outcome; big changes -> logical walkthrough + rationale + next actions; casual one-offs -> plain sentences, no headers/bullets.
- File References: When referencing files in your response follow the below rules:
  * Use inline code to make file paths clickable.
  * Each reference should have a stand alone path. Even if it's the same file.
  * Accepted: absolute, workspace-relative, a/ or b/ diff prefixes, or bare filename/suffix.
  * Optionally include line/column (1-based): :line[:column] or #Lline[Ccolumn] (column defaults to 1).
  * Do not use URIs like file://, vscode://, or https://.
  * Do not provide range of lines
  * Examples: src/app.ts, src/app.ts:42, b/server/index.js#L10, C:\repo\project\main.rs:12:5"##;

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

    #[serde(default)]
    pub opencode_tool_mode: Option<OpenCodeToolMode>,

    #[serde(default)]
    pub trace_provider_io: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenCodeToolMode {
    Auto,
    ApplyPatch,
    EditWrite,
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
    last_request_stats: Option<ModelRequestStats>,
}

impl OpenAiCompatibleModelProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Result<Self, RuntimeError> {
        Ok(Self {
            config,
            last_request_stats: None,
        })
    }

    fn call_model_with_request_timeout(
        &mut self,
        name: &str,
        input: &Value,
        action_timeout: Option<Duration>,
    ) -> Result<Value, RuntimeError> {
        self.last_request_stats = None;
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
        let request = build_chat_completion_body(model_config, model_name, input)?;
        let trace_provider_io = model_config.trace_provider_io == Some(true);
        self.last_request_stats = Some(chat_completion_request_stats(
            &request.body,
            trace_provider_io,
        ));

        let request_timeout =
            effective_request_timeout(model_config.request_timeout_seconds, action_timeout);

        let response =
            send_chat_completion_request(api_key, &base_url, &request.body, request_timeout)?;

        if trace_provider_io {
            if let Some(stats) = self.last_request_stats.as_mut() {
                stats.provider_response = Some(response.clone());
            }
        }

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

    fn take_last_request_stats(&mut self) -> Option<ModelRequestStats> {
        self.last_request_stats.take()
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

fn send_chat_completion_request(
    api_key: String,
    base_url: &str,
    body: &Value,
    timeout: Duration,
) -> Result<Value, RuntimeError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(provider_error)?;
    let response = client
        .post(chat_completions_url(base_url))
        .bearer_auth(api_key)
        .json(body)
        .send()
        .map_err(|error| {
            if error.is_timeout() {
                provider_timeout_error("chat/completions", timeout)
            } else {
                provider_error(error)
            }
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_else(|error| error.to_string());
        return Err(RuntimeError::Provider(sanitize_provider_error_text(
            &format!("chat/completions returned HTTP {status}: {body}"),
        )));
    }

    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        let text = response.text().map_err(provider_error)?;
        return parse_chat_completion_stream(&text);
    }

    response.json::<Value>().map_err(provider_error)
}

fn parse_chat_completion_stream(text: &str) -> Result<Value, RuntimeError> {
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: BTreeMap<usize, StreamToolCall> = BTreeMap::new();

    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("data:") {
            continue;
        }
        let payload = line[5..].trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(payload).map_err(|error| {
            RuntimeError::Provider(format!("invalid chat/completions stream event: {error}"))
        })?;
        let choices = event
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for choice in choices {
            let Some(delta) = choice.get("delta").and_then(Value::as_object) else {
                continue;
            };
            if let Some(part) = delta.get("content").and_then(Value::as_str) {
                content.push_str(part);
            }
            if let Some(part) = delta.get("reasoning_content").and_then(Value::as_str) {
                reasoning.push_str(part);
            }
            for call in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let entry = tool_calls.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    entry.id.push_str(id);
                }
                if let Some(kind) = call.get("type").and_then(Value::as_str) {
                    entry.kind.push_str(kind);
                }
                if let Some(function) = call.get("function").and_then(Value::as_object) {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        entry.name.push_str(name);
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        entry.arguments.push_str(arguments);
                    }
                }
            }
        }
    }

    let mut message = Map::new();
    message.insert("role".to_string(), json!("assistant"));
    message.insert("content".to_string(), Value::String(content));
    if !reasoning.is_empty() {
        message.insert("reasoning_content".to_string(), Value::String(reasoning));
    }
    if !tool_calls.is_empty() {
        message.insert(
            "tool_calls".to_string(),
            Value::Array(
                tool_calls
                    .into_iter()
                    .map(|(index, call)| {
                        json!({
                            "index": index,
                            "id": if call.id.is_empty() { format!("call_{index}") } else { call.id },
                            "type": if call.kind.is_empty() { "function".to_string() } else { call.kind },
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments
                            }
                        })
                    })
                    .collect(),
            ),
        );
    }
    Ok(json!({
        "choices": [{
            "message": Value::Object(message)
        }]
    }))
}

#[derive(Default)]
struct StreamToolCall {
    id: String,
    kind: String,
    name: String,
    arguments: String,
}

fn chat_completions_url(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.ends_with("/chat/completions") {
        base_url.to_string()
    } else {
        format!("{base_url}/chat/completions")
    }
}

fn build_chat_completion_body(
    model_config: &OpenAiModelConfig,
    model_name: String,
    input: &Value,
) -> Result<ChatCompletionBody, RuntimeError> {
    let opencode_style = model_config.native_tool_calls == Some(true)
        && (uses_opencode_prompt(model_config) || is_opencode_code_input(input));
    let request_model_name = model_name;
    let model_feature_name = request_model_name.to_ascii_lowercase();
    let mut body = json!({
        "model": request_model_name
    });

    if opencode_style {
        body["max_tokens"] = json!(32_000);
        body["stream"] = json!(true);
        body["stream_options"] = json!({
            "include_usage": true
        });
        if let Some(reasoning_effort) = opencode_reasoning_effort(&model_feature_name) {
            body["reasoning_effort"] = json!(reasoning_effort);
        }
    } else {
        if let Some(extra_body) = &model_config.extra_body {
            merge_extra_body(&mut body, extra_body);
        }
        if let Some(temperature) = model_config.temperature {
            body["temperature"] = json!(temperature);
        }
    }
    let tool_name_map = if model_config.native_tool_calls == Some(true) {
        attach_native_tool_calls(
            &mut body,
            input,
            opencode_style,
            model_config,
            &request_model_name,
        )?
    } else {
        BTreeMap::new()
    };
    body["messages"] = Value::Array(build_chat_messages(
        model_config,
        &request_model_name,
        input,
        &tool_name_map,
        opencode_style,
    )?);
    if tool_name_map.is_empty() {
        if let Some(response_format) = resolved_response_format(model_config) {
            body["response_format"] = response_format;
        }
    }

    Ok(ChatCompletionBody {
        body,
        tool_name_map,
    })
}

fn opencode_reasoning_effort(model_name: &str) -> Option<&str> {
    if model_name.contains("gpt-5")
        && !model_name.contains("gpt-5-chat")
        && !model_name.contains("gpt-5-pro")
    {
        return Some("medium");
    }
    let (_, suffix) = model_name.rsplit_once(':')?;
    match suffix {
        "low" | "medium" | "high" | "xhigh" => Some(suffix),
        _ => None,
    }
}

fn build_chat_messages(
    model_config: &OpenAiModelConfig,
    model_name: &str,
    input: &Value,
    tool_name_map: &BTreeMap<String, String>,
    opencode_style: bool,
) -> Result<Vec<Value>, RuntimeError> {
    let mut messages = Vec::new();
    if let Some(mut system_prompt) = resolved_system_prompt(model_config) {
        if opencode_style {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&opencode_environment_prompt(model_name));
        }
        messages.push(json!({
            "role": "system",
            "content": system_prompt
        }));
    } else if opencode_style {
        messages.push(json!({
            "role": "system",
            "content": opencode_environment_prompt(model_name)
        }));
    }
    if model_config.native_tool_calls == Some(true) {
        messages.extend(input_to_native_tool_messages_with_names(
            input,
            tool_name_map,
            opencode_style,
        )?);
    } else {
        messages.push(json!({
            "role": "user",
            "content": input_to_content(input)?
        }));
    }
    Ok(messages)
}

fn resolved_system_prompt(model_config: &OpenAiModelConfig) -> Option<String> {
    let prompt = model_config.system_prompt.as_ref()?;
    if prompt.trim() == OPENCODE_QWEN_PROMPT_MARKER {
        return Some(OPENCODE_QWEN_PROMPT.to_string());
    }
    Some(prompt.clone())
}

fn uses_opencode_prompt(model_config: &OpenAiModelConfig) -> bool {
    model_config
        .system_prompt
        .as_deref()
        .is_some_and(|prompt| prompt.trim() == OPENCODE_QWEN_PROMPT_MARKER)
}

fn is_opencode_code_input(input: &Value) -> bool {
    let Some(object) = input.as_object() else {
        return false;
    };
    if !object.get("task").is_some_and(Value::is_string) {
        return false;
    }
    let tools = object
        .get("allowed_tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    tools.contains(&"read") && tools.contains(&"edit") && tools.contains(&"bash")
}

fn opencode_environment_prompt(model_name: &str) -> String {
    let directory = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| ".".to_string());
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let git_repo = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|value| value.trim() == "true");
    let date = Command::new("date")
        .arg("+%a %b %e %Y")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let exact_model_id = opencode_exact_model_id(model_name);
    [
        String::new(),
        format!("You are powered by the model named {model_name}. The exact model ID is {exact_model_id}"),
        "Here is some useful information about the environment you are running in:".to_string(),
        "<env>".to_string(),
        format!("  Working directory: {directory}"),
        format!("  Is directory a git repo: {}", if git_repo { "yes" } else { "no" }),
        format!("  Platform: {platform}"),
        format!("  Today's date: {date}"),
        "</env>".to_string(),
        "<files>".to_string(),
        "  ".to_string(),
        "</files>".to_string(),
    ]
    .join("\n")
}

fn opencode_exact_model_id(model_name: &str) -> String {
    if let Ok(prefix) = std::env::var("AIR_OPENCODE_MODEL_ID_PREFIX") {
        let prefix = prefix.trim().trim_end_matches('/');
        if !prefix.is_empty() {
            return format!("{prefix}/{model_name}");
        }
    }
    if model_name.to_ascii_lowercase().starts_with("glm") {
        format!("zhipuai-coding-plan/{model_name}")
    } else {
        format!("openai-compatible/{model_name}")
    }
}

fn chat_completion_request_stats(body: &Value, trace_provider_io: bool) -> ModelRequestStats {
    ModelRequestStats {
        provider_request_bytes: serde_json::to_vec(body)
            .map(|bytes| bytes.len())
            .unwrap_or(0),
        provider_user_content_bytes: provider_user_content_bytes(body),
        provider_tools_bytes: body
            .get("tools")
            .and_then(|tools| serde_json::to_vec(tools).ok())
            .map(|bytes| bytes.len())
            .unwrap_or(0),
        provider_request: trace_provider_io.then(|| body.clone()),
        provider_response: None,
    }
}

fn provider_user_content_bytes(body: &Value) -> usize {
    body.get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .map(|content| match content {
            Value::String(text) => text.len(),
            other => serde_json::to_vec(other)
                .map(|bytes| bytes.len())
                .unwrap_or(0),
        })
        .sum()
}

fn attach_native_tool_calls(
    body: &mut Value,
    input: &Value,
    opencode_style: bool,
    model_config: &OpenAiModelConfig,
    model_name: &str,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    let tool_schemas = input.get("tool_schemas").and_then(Value::as_object);
    let mut tool_names =
        if let Some(allowed_tools) = input.get("allowed_tools").and_then(Value::as_array) {
            allowed_tools
                .iter()
                .filter_map(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        } else {
            tool_schemas
                .into_iter()
                .flat_map(|schemas| schemas.keys())
                .cloned()
                .collect::<Vec<_>>()
        };
    if opencode_style {
        let use_patch = opencode_uses_apply_patch(model_config.opencode_tool_mode, model_name);
        for name in opencode_tool_order(use_patch) {
            if !tool_names.iter().any(|existing| existing == name) {
                tool_names.push((*name).to_string());
            }
        }
        if use_patch {
            tool_names.retain(|name| name != "edit" && name != "write");
        } else {
            tool_names.retain(|name| name != "apply_patch");
        }
        tool_names.sort_by_key(|name| {
            opencode_tool_order(use_patch)
                .iter()
                .position(|candidate| candidate == name)
                .unwrap_or(usize::MAX)
        });
    }
    if tool_names.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut tools = Vec::new();
    let mut tool_name_map = BTreeMap::new();
    let mut used_names = BTreeMap::new();

    for original_name in tool_names {
        if tool_schemas.is_some_and(|schemas| !schemas.contains_key(&original_name)) {
            continue;
        }
        let safe_name = safe_openai_tool_name(&original_name, &mut used_names);
        tool_name_map.insert(safe_name.clone(), original_name.clone());
        let schema = tool_schemas.and_then(|schemas| schemas.get(&original_name));
        let mut function = json!({
            "name": safe_name,
            "description": native_tool_description(&original_name, schema),
            "parameters": native_tool_parameters(&original_name, schema)
        });
        if !opencode_style {
            function["strict"] = json!(false);
        }
        tools.push(json!({
            "type": "function",
            "function": function
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

fn opencode_uses_apply_patch(mode: Option<OpenCodeToolMode>, model_name: &str) -> bool {
    match mode.unwrap_or(OpenCodeToolMode::Auto) {
        OpenCodeToolMode::ApplyPatch => true,
        OpenCodeToolMode::EditWrite => false,
        OpenCodeToolMode::Auto => {
            let model_name = model_name.to_ascii_lowercase();
            model_name.contains("gpt-")
                && !model_name.contains("oss")
                && !model_name.contains("gpt-4")
        }
    }
}

fn opencode_tool_order(use_patch: bool) -> &'static [&'static str] {
    if use_patch {
        &[
            "question",
            "bash",
            "grep",
            "glob",
            "lsp",
            "task",
            "read",
            "webfetch",
            "todowrite",
            "todoread",
            "skill",
            "apply_patch",
        ]
    } else {
        &[
            "question",
            "bash",
            "grep",
            "glob",
            "lsp",
            "task",
            "read",
            "edit",
            "write",
            "webfetch",
            "todowrite",
            "todoread",
            "skill",
        ]
    }
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
    match original_name {
        "question" => return opencode_question_description(),
        "glob" => return opencode_glob_description(),
        "read" => return opencode_read_description(),
        "grep" => return opencode_grep_description(),
        "edit" => return opencode_edit_description(),
        "write" => return opencode_write_description(),
        "apply_patch" => return opencode_apply_patch_description(),
        "task" => return opencode_task_description(),
        "webfetch" => return opencode_webfetch_description(),
        "lsp" => return opencode_lsp_description(),
        "bash" => return opencode_bash_description(),
        "todowrite" => return opencode_todowrite_description(),
        "todoread" => return "Use this tool to read your todo list".to_string(),
        "skill" => return opencode_skill_description(),
        _ => {}
    }
    let Some(schema) = schema else {
        return format!("Tool {original_name}");
    };
    let mut parts = vec![format!("Tool {original_name}")];
    if let Some(required) = schema.get("required") {
        parts.push(format!("required: {}", compact_json(required)));
    }
    if let Some(optional) = schema.get("optional") {
        parts.push(format!("optional: {}", compact_json(optional)));
    }
    parts.join("; ")
}

fn opencode_bash_description() -> String {
    let directory = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| ".".to_string());
    r##"Executes a given bash command in a persistent shell session with optional timeout, ensuring proper handling and security measures.

All commands run in ${directory} by default. Use the `workdir` parameter if you need to run a command in a different directory. AVOID using `cd <directory> && <command>` patterns - use `workdir` instead.

IMPORTANT: This tool is for terminal operations like git, npm, docker, etc. DO NOT use it for file operations (reading, writing, editing, searching, finding files) - use the specialized tools for this instead.

Before executing the command, please follow these steps:

1. Directory Verification:
   - If the command will create new directories or files, first use `ls` to verify the parent directory exists and is the correct location
   - For example, before running "mkdir foo/bar", first use `ls foo` to check that "foo" exists and is the intended parent directory

2. Command Execution:
   - Always quote file paths that contain spaces with double quotes (e.g., rm "path with spaces/file.txt")
   - Examples of proper quoting:
     - mkdir "/Users/name/My Documents" (correct)
     - mkdir /Users/name/My Documents (incorrect - will fail)
     - python "/path/with spaces/script.py" (correct)
     - python /path/with spaces/script.py (incorrect - will fail)
   - After ensuring proper quoting, execute the command.
   - Capture the output of the command.

Usage notes:
  - The command argument is required.
  - You can specify an optional timeout in milliseconds. If not specified, commands will time out after 120000ms (2 minutes).
  - It is very helpful if you write a clear, concise description of what this command does in 5-10 words.
  - If the output exceeds ${maxLines} lines or ${maxBytes} bytes, it will be truncated and the full output will be written to a file. You can use Read with offset/limit to read specific sections or Grep to search the full content. Because of this, you do NOT need to use `head`, `tail`, or other truncation commands to limit output - just run the command directly.

  - Avoid using Bash with the `find`, `grep`, `cat`, `head`, `tail`, `sed`, `awk`, or `echo` commands, unless explicitly instructed or when these commands are truly necessary for the task. Instead, always prefer using the dedicated tools for these commands:
    - File search: Use Glob (NOT find or ls)
    - Content search: Use Grep (NOT grep or rg)
    - Read files: Use Read (NOT cat/head/tail)
    - Edit files: Use Edit (NOT sed/awk)
    - Write files: Use Write (NOT echo >/cat <<EOF)
    - Communication: Output text directly (NOT echo/printf)
  - When issuing multiple commands:
    - If the commands are independent and can run in parallel, make multiple Bash tool calls in a single message. For example, if you need to run "git status" and "git diff", send a single message with two Bash tool calls in parallel.
    - If the commands depend on each other and must run sequentially, use a single Bash call with '&&' to chain them together (e.g., `git add . && git commit -m "message" && git push`). For instance, if one operation must complete before another starts (like mkdir before cp, Write before Bash for git operations, or git add before git commit), run these operations sequentially instead.
    - Use ';' only when you need to run commands sequentially but don't care if earlier commands fail
    - DO NOT use newlines to separate commands (newlines are ok in quoted strings)
  - AVOID using `cd <directory> && <command>`. Use the `workdir` parameter to change directories instead.
    <good-example>
    Use workdir="/foo/bar" with command: pytest tests
    </good-example>
    <bad-example>
    cd /foo/bar && pytest tests
    </bad-example>

# Committing changes with git

Only create commits when requested by the user. If unclear, ask first. When the user asks you to create a new git commit, follow these steps carefully:

Git Safety Protocol:
- NEVER update the git config
- NEVER run destructive/irreversible git commands (like push --force, hard reset, etc) unless the user explicitly requests them
- NEVER skip hooks (--no-verify, --no-gpg-sign, etc) unless the user explicitly requests it
- NEVER run force push to main/master, warn the user if they request it
- Avoid git commit --amend. ONLY use --amend when ALL conditions are met:
  (1) User explicitly requested amend, OR commit SUCCEEDED but pre-commit hook auto-modified files that need including
  (2) HEAD commit was created by you in this conversation (verify: git log -1 --format='%an %ae')
  (3) Commit has NOT been pushed to remote (verify: git status shows "Your branch is ahead")
- CRITICAL: If commit FAILED or was REJECTED by hook, NEVER amend - fix the issue and create a NEW commit
- CRITICAL: If you already pushed to remote, NEVER amend unless user explicitly requests it (requires force push)
- NEVER commit changes unless the user explicitly asks you to. It is VERY IMPORTANT to only commit when explicitly asked, otherwise the user will feel that you are being too proactive.

1. You can call multiple tools in a single response. When multiple independent pieces of information are requested and all commands are likely to succeed, run multiple tool calls in parallel for optimal performance. run the following bash commands in parallel, each using the Bash tool:
  - Run a git status command to see all untracked files.
  - Run a git diff command to see both staged and unstaged changes that will be committed.
  - Run a git log command to see recent commit messages, so that you can follow this repository's commit message style.
2. Analyze all staged changes (both previously staged and newly added) and draft a commit message:
  - Summarize the nature of the changes (eg. new feature, enhancement to an existing feature, bug fix, refactoring, test, docs, etc.). Ensure the message accurately reflects the changes and their purpose (i.e. "add" means a wholly new feature, "update" means an enhancement to an existing feature, "fix" means a bug fix, etc.).
  - Do not commit files that likely contain secrets (.env, credentials.json, etc.). Warn the user if they specifically request to commit those files
  - Draft a concise (1-2 sentences) commit message that focuses on the "why" rather than the "what"
  - Ensure it accurately reflects the changes and their purpose
3. You can call multiple tools in a single response. When multiple independent pieces of information are requested and all commands are likely to succeed, run multiple tool calls in parallel for optimal performance. run the following commands:
   - Add relevant untracked files to the staging area.
   - Create the commit with a message
   - Run git status after the commit completes to verify success.
   Note: git status depends on the commit completing, so run it sequentially after the commit.
4. If the commit fails due to pre-commit hook, fix the issue and create a NEW commit (see amend rules above)

Important notes:
- NEVER run additional commands to read or explore code, besides git bash commands
- NEVER use the TodoWrite or Task tools
- DO NOT push to the remote repository unless the user explicitly asks you to do so
- IMPORTANT: Never use git commands with the -i flag (like git rebase -i or git add -i) since they require interactive input which is not supported.
- If there are no changes to commit (i.e., no untracked files and no modifications), do not create an empty commit

# Creating pull requests
Use the gh command via the Bash tool for ALL GitHub-related tasks including working with issues, pull requests, checks, and releases. If given a Github URL use the gh command to get the information needed.

IMPORTANT: When the user asks you to create a pull request, follow these steps carefully:

1. You can call multiple tools in a single response. When multiple independent pieces of information are requested and all commands are likely to succeed, run multiple tool calls in parallel for optimal performance. run the following bash commands in parallel using the Bash tool, in order to understand the current state of the branch since it diverged from the main branch:
   - Run a git status command to see all untracked files
   - Run a git diff command to see both staged and unstaged changes that will be committed
   - Check if the current branch tracks a remote branch and is up to date with the remote, so you know if you need to push to the remote
   - Run a git log command and `git diff [base-branch]...HEAD` to understand the full commit history for the current branch (from the time it diverged from the base branch)
2. Analyze all changes that will be included in the pull request, making sure to look at all relevant commits (NOT just the latest commit, but ALL commits that will be included in the pull request!!!), and draft a pull request summary
3. You can call multiple tools in a single response. When multiple independent pieces of information are requested and all commands are likely to succeed, run multiple tool calls in parallel for optimal performance. run the following commands in parallel:
   - Create new branch if needed
   - Push to remote with -u flag if needed
   - Create PR using gh pr create with the format below. Use a HEREDOC to pass the body to ensure correct formatting.
<example>
gh pr create --title "the pr title" --body "$(cat <<'EOF'
## Summary
<1-3 bullet points>
</example>

Important:
- DO NOT use the TodoWrite or Task tools
- Return the PR URL when you're done, so the user can see it

# Other common operations
- View comments on a Github PR: gh api repos/foo/bar/pulls/123/comments"##
        .replace("${directory}", &directory)
        .replace("${maxLines}", "2000")
        .replace("${maxBytes}", "51200")
}

fn opencode_question_description() -> String {
    r##"Use this tool when you need to ask the user questions during execution. This allows you to:
1. Gather user preferences or requirements
2. Clarify ambiguous instructions
3. Get decisions on implementation choices as you work
4. Offer choices to the user about what direction to take.

Usage notes:
- When `custom` is enabled (default), a "Type your own answer" option is added automatically; don't include "Other" or catch-all options
- Answers are returned as arrays of labels; set `multiple: true` to allow selecting more than one
- If you recommend a specific option, make that the first option in the list and add "(Recommended)" at the end of the label
"##
        .to_string()
}

fn opencode_read_description() -> String {
    r##"Read file content only after you have localized a small file, known symbol, or narrow line range.
This is not the default tool for exploring a path. If you only know a file path or name, use Glob first to inspect line_count and source_bytes without reading the file body. If you need a symbol or phrase, use Grep or LSP before Read.

Usage:
- The filePath parameter may be workspace-relative or absolute; prefer workspace-relative paths or exact paths returned by Glob/Grep/Read
- Do not invent absolute paths from the model server or API bridge process; use the current working directory shown in the environment
- If you only know a file path, use Glob first to inspect line_count and source_bytes before reading the file body
- Do not whole-file Read files larger than 200 lines or 20KB
- For unfamiliar or large files, use Grep, LSP, or contains first to locate the relevant symbols or line ranges
- Whole-file Read is only reasonable after Glob shows the file is 200 lines or fewer and 20KB or less
- Without offset/limit, large files may return only a bounded preview; continue with targeted line ranges around known matches
- Any lines longer than 2000 characters will be truncated
- Results are returned using cat -n format, with line numbers starting at 1
- You can call multiple tools in one response; prefer batching Glob/Grep/LSP locator calls before reading content
- If you read a file that exists but has empty contents you will receive a system reminder warning in place of file contents.
- You can read image files using this tool.
"##
        .to_string()
}

fn opencode_grep_description() -> String {
    r##"- Fast content search tool that works with any codebase size
- Searches file contents using regular expressions
- Supports full regex syntax (eg. "log.*Error", "function\s+\w+", etc.)
- Filter files by pattern with the include parameter (eg. "*.js", "*.{ts,tsx}")
- Returns file paths and line numbers with at least one match sorted by modification time
- Use this tool before Read when you know a function, type, error text, config key, or other symbol-like phrase
- If you need to identify/count the number of matches within files, use the Bash tool with `rg` (ripgrep) directly. Do NOT use `grep`.
- When you are doing an open-ended search that may require multiple rounds of globbing and grepping, use the Task tool instead
"##
        .to_string()
}

fn opencode_glob_description() -> String {
    r##"- Fast file pattern matching tool that works with any codebase size
- Supports glob patterns like "**/*.js" or "src/**/*.ts"
- Returns matching file paths sorted by modification time, plus line_count and source_bytes metadata
- Use this tool when you need to find files by name patterns
- Use this tool before Read when you only know a file path or file name, so you can decide whether the file is small enough to read whole
- When you are doing an open-ended search that may require multiple rounds of globbing and grepping, use the Task tool instead
- You have the capability to call multiple tools in a single response. It is always better to speculatively perform multiple searches as a batch that are potentially useful.
"##
        .to_string()
}

fn opencode_edit_description() -> String {
    r##"Performs exact string replacements in files.

Usage:
- You must use your `Read` tool at least once in the conversation before editing. This tool will error if you attempt an edit without reading the file.
- The filePath parameter may be workspace-relative or absolute; prefer workspace-relative paths or exact paths returned by Glob/Grep/Read.
- When editing text from Read tool output, ensure you preserve the exact indentation (tabs/spaces) as it appears AFTER the line number prefix. The line number prefix format is: spaces + line number + tab. Everything after that tab is the actual file content to match. Never include any part of the line number prefix in the oldString or newString.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- Only use emojis if the user explicitly requests it. Avoid adding emojis to files unless asked.
- The edit will FAIL if `oldString` is not found in the file with an error "oldString not found in content".
- The edit will FAIL if `oldString` is found multiple times in the file with an error "oldString found multiple times and requires more code context to uniquely identify the intended match". Either provide a larger string with more surrounding context to make it unique or use `replaceAll` to change every instance of `oldString`.
- Use `replaceAll` for replacing and renaming strings across the file. This parameter is useful if you want to rename a variable for instance.
"##
        .to_string()
}

fn opencode_write_description() -> String {
    r##"Writes a file to the local filesystem.

Usage:
- This tool will overwrite the existing file if there is one at the provided path.
- The filePath parameter may be workspace-relative or absolute; prefer workspace-relative paths under the current workspace.
- If this is an existing file, you MUST use the Read tool first to read the file's contents. This tool will fail if you did not read the file first.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- NEVER proactively create documentation files (*.md) or README files. Only create documentation files if explicitly requested by the User.
- Only use emojis if the user explicitly requests it. Avoid writing emojis to files unless asked.
"##
        .to_string()
}

fn opencode_task_description() -> String {
    r##"Launch a new agent to handle complex, multistep tasks autonomously.

Available agent types and the tools they have access to:
- explore: Fast read-only agent specialized for exploring codebases. Use this when you need to quickly find files by patterns (eg. "src/components/**/*.tsx"), search code for keywords (eg. "API endpoints"), or answer questions about the codebase (eg. "how do API endpoints work?"). When calling this agent, specify the desired thoroughness level: "quick" for basic searches, "medium" for moderate exploration, or "very thorough" for comprehensive analysis across multiple locations and naming conventions.

When using the Task tool, you must specify a subagent_type parameter to select which agent type to use.

When to use the Task tool:
- Use the explore subagent proactively for broad or open-ended codebase investigation that would otherwise require multiple rounds of Glob, Grep, and Read.
- Use the explore subagent when you need a concise map of relevant files, symbols, line ranges, and next steps before editing.
- When you are instructed to execute custom slash commands. Use the Task tool with the slash command invocation as the entire prompt. The slash command can take arguments. For example: Task(description="Check the file", prompt="/check-file path/to/file.py")

When NOT to use the Task tool:
- If you only know a specific file path, use Glob first to inspect line_count/source_bytes instead of launching Task
- If you are searching for a specific class definition like "class Foo", use the Glob tool instead, to find the match more quickly
- If you are searching for code within a specific file or set of 2-3 files, use Grep/LSP first and then Read the narrow matching range
- Other tasks that are not related to the agent descriptions above


Usage notes:
1. Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses
2. When the agent is done, it will return a single message back to you. The result returned by the agent is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.
3. Each agent invocation is stateless unless you provide a session_id. Your prompt should contain a highly detailed task description for the agent to perform autonomously and you should specify exactly what information the agent should return back to you in its final and only message to you.
4. The agent's outputs should generally be trusted
5. The explore agent is read-only. Ask it for findings, evidence, and exact next read/edit/test recommendations; do not ask it to modify files.
6. If the agent description mentions that it should be used proactively, then you should try your best to use it without the user having to ask for it first. Use your judgement.

<example>
user: "Refactor the code agent tool handling."
<commentary>
The request is broad and may require several searches across the codebase. Use the explore subagent first.
</commentary>
assistant: Uses the Task tool with subagent_type="explore".
</example>
"##
        .to_string()
}

fn opencode_webfetch_description() -> String {
    r##"- Fetches content from a specified URL
- Takes a URL and optional format as input
- Fetches the URL content, converts to requested format (markdown by default)
- Returns the content in the specified format
- Use this tool when you need to retrieve and analyze web content

Usage notes:
  - IMPORTANT: if another tool is present that offers better web fetching capabilities, is more targeted to the task, or has fewer restrictions, prefer using that tool instead of this one.
  - The URL must be a fully-formed valid URL
  - HTTP URLs will be automatically upgraded to HTTPS
  - Format options: "markdown" (default), "text", or "html"
  - This tool is read-only and does not modify any files
  - Results may be summarized if the content is very large
"##
        .to_string()
}

fn opencode_lsp_description() -> String {
    r##"Interact with Language Server Protocol (LSP) servers to get code intelligence features.

Supported operations:
- diagnostics: Get workspace or file diagnostics
- references/findReferences: Find all references for a known Rust symbol in a known file

Use LSP after Grep/Read has identified the relevant file or symbol. For initial symbol lookup across unknown files, use Grep first, then LSP references once you have a file path.

References accepts:
- filePath/path: file to operate on
- symbol: optional Rust symbol name to locate in that file
- line/character: optional 1-based editor position

Note: LSP servers must be configured for the file type. If no server is available, an error will be returned."##
        .to_string()
}

fn opencode_skill_description() -> String {
    "Load a skill to get detailed instructions for a specific task. No skills are currently available."
        .to_string()
}

fn opencode_todowrite_description() -> String {
    r##"Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool
Use this tool proactively in these scenarios:

1. Complex multistep tasks - When a task requires 3 or more distinct steps or actions
2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
3. User explicitly requests todo list - When the user directly asks you to use the todo list
4. User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)
5. After receiving new instructions - Immediately capture user requirements as todos. Feel free to edit the todo list based on new information.
6. After completing a task - Mark it complete and add any new follow-up tasks
7. When you start working on a new task, mark the todo as in_progress. Ideally you should only have one todo as in_progress at a time. Complete existing tasks before starting new ones.

## When NOT to Use This Tool

Skip using this tool when:
1. There is only a single, straightforward task
2. The task is trivial and tracking it provides no organizational benefit
3. The task can be completed in less than 3 trivial steps
4. The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.

## Examples of When to Use the Todo List

<example>
User: I want to add a dark mode toggle to the application settings. Make sure you run the tests and build when you're done!
Assistant: I'll help add a dark mode toggle to your application settings. Let me create a todo list to track this implementation.
*Creates todo list with the following items:*
1. Create dark mode toggle component in Settings page
2. Add dark mode state management (context/store)
3. Implement CSS-in-JS styles for dark theme
4. Update existing components to support theme switching
5. Run tests and build process, addressing any failures or errors that occur
*Begins working on the first task*

<reasoning>
The assistant used the todo list because:
1. Adding dark mode is a multi-step feature requiring UI, state management, and styling changes
2. The user explicitly requested tests and build be run afterward
3. The assistant inferred that tests and build need to pass by adding "Ensure tests and build succeed" as the final task
</reasoning>
</example>


<example>
User: Help me rename the function getCwd to getCurrentWorkingDirectory across my project
Assistant: Let me first search through your codebase to find all occurrences of 'getCwd'.
*Uses grep or search tools to locate all instances of getCwd in the codebase*
Assistant: I've found 15 instances of 'getCwd' across 8 different files. Let me create a todo list to track these changes.
*Creates todo list with specific items for each file that needs updating*

<reasoning>
The assistant used the todo list because:
1. First, the assistant searched to understand the scope of the task
2. Upon finding multiple occurrences across different files, it determined this was a complex task with multiple steps
3. The todo list helps ensure every instance is tracked and updated systematically
4. This approach prevents missing any occurrences and maintains code consistency
</reasoning>
</example>

<example>
User: I need to implement these features for my e-commerce site: user registration, product catalog, shopping cart, and checkout flow.
Assistant: I'll help implement these features. First, let's add all the features to the todo list.
*Creates a todo list breaking down each feature into specific tasks based on the project architecture*
Assistant: Let's start with user registration. This will involve creating database models, API endpoints, and frontend forms that integrate with your existing authentication system.

<reasoning>
The assistant used the todo list because:
1. The user provided multiple complex features to implement in a comma separated list
2. The todo list helps organize these large features into manageable tasks
3. This approach allows for tracking progress across the entire implementation
</reasoning>
</example>

<example>
User: Can you help optimize my React application? It's rendering slowly and has performance issues.</user>
Assistant: I'll help optimize your React application. First, let me examine your codebase to identify potential performance bottlenecks.
*Reviews component structure, render patterns, state management, and data fetching*
Assistant: After analyzing your codebase, I've identified several performance issues. Let me create a todo list to track our optimization efforts.
*Creates todo list with items like: 1) Implement memoization for expensive calculations in ProductList, 2) Add virtualization for long lists in Dashboard, 3) Optimize image loading in Gallery component, 4) Fix state update loops in ShoppingCart, 5) Review bundle size and implement code splitting*
Let's start by implementing memoization for the expensive calculations in your ProductList component.</assistant>

<reasoning>
The assistant used the todo list because:
1. First, the assistant examined the codebase to identify specific performance issues
2. Based on this analysis, it identified multiple optimization opportunities
3. Performance optimization is a non-trivial task requiring multiple steps
4. The todo list helps methodically track improvements across different components
5. This systematic approach ensures all performance bottlenecks are addressed
</reasoning>
</example>

## Examples of When NOT to Use the Todo List

<example>
User: How do I print 'Hello World' in Python?
Assistant: In Python, you can print "Hello World" with this simple code:

python
print("Hello World")

This will output the text "Hello World" to the console when executed.</assistant>

<reasoning>
The assistant did not use the todo list because this is a single, trivial task that can be completed in one step. There's no need to track multiple tasks or steps for such a straightforward request.
</reasoning>
</example>

<example>
User: What does the git status command do?
Assistant: The git status command shows the current state of your working directory and staging area. It displays which changes have been staged, which haven't, and which files aren't being tracked by Git. This is a fundamental command for understanding the current state of your repository before making commits.

<reasoning>
The assistant did not use the todo list because this is an informational request with no actual coding task to complete. The user is simply asking for an explanation, not for the assistant to perform multiple steps or tasks.
</reasoning>
</example>

<example>
User: Can you add a comment to the calculateTotal function to explain what it does?
Assistant: Sure, let me add a comment to the calculateTotal function to explain what it does.
* Uses the Edit tool to add a comment to the calculateTotal function *

<reasoning>
The assistant did not use the todo list because this is a single, straightforward task confined to one location in the code. Adding a comment doesn't require tracking multiple steps or systematic organization.
</reasoning>
</example>

<example>
User: Run npm install for me and tell me what happens.
Assistant: I'll run the npm install command for you.

*Executes: npm install*

The command completed successfully. Here's the output:
[Output of npm install command]

All dependencies have been installed according to your package.json file.

<reasoning>
The assistant did not use the todo list because this is a single command execution with immediate results. There are no multiple steps to track or organize, making the todo list unnecessary for this straightforward task.
</reasoning>
</example>

## Task States and Management

1. **Task States**: Use these states to track progress:
   - pending: Task not yet started
   - in_progress: Currently working on (limit to ONE task at a time)
   - completed: Task finished successfully
   - cancelled: Task no longer needed

2. **Task Management**:
   - Update task status in real-time as you work
   - Mark tasks complete IMMEDIATELY after finishing (don't batch completions)
   - Only have ONE task in_progress at any time
   - Complete current tasks before starting new ones
   - Cancel tasks that become irrelevant

3. **Task Breakdown**:
   - Create specific, actionable items
   - Break complex tasks into smaller, manageable steps
   - Use clear, descriptive task names

When in doubt, use this tool. Being proactive with task management demonstrates attentiveness and ensures you complete all requirements successfully.

"##
        .to_string()
}

fn opencode_apply_patch_description() -> String {
    r##"Use the `apply_patch` tool to edit files. Your patch language is a stripped-down, file-oriented diff format designed to be easy to parse and safe to apply. You can think of it as a high-level envelope:

*** Begin Patch
[ one or more file sections ]
*** End Patch

Within that envelope, you get a sequence of file operations.
You MUST include a header to specify the action you are taking.
Each operation starts with one of three headers:

*** Add File: <path> - create a new file. Every following line is a + line (the initial contents).
*** Delete File: <path> - remove an existing file. Nothing follows.
*** Update File: <path> - patch an existing file in place (optionally with a rename).

Example patch:

```
*** Begin Patch
*** Add File: hello.txt
+Hello world
*** Update File: src/app.py
*** Move to: src/main.py
@@ def greet():
-print("Hi")
+print("Hello, world!")
*** Delete File: obsolete.txt
*** End Patch
```

It is important to remember:

- You must include a header with your intended action (Add/Delete/Update)
- You must prefix new lines with `+` even when creating a new file"##
        .to_string()
}

fn native_tool_parameters(original_name: &str, schema: Option<&Value>) -> Value {
    match original_name {
        "question" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "questions": {
                        "description": "Questions to ask",
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {"description": "Complete question", "type": "string"},
                                "header": {"description": "Very short label (max 30 chars)", "type": "string"},
                                "options": {
                                    "description": "Available choices",
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {"description": "Display text (1-5 words, concise)", "type": "string"},
                                            "description": {"description": "Explanation of choice", "type": "string"}
                                        },
                                        "ref": "QuestionOption",
                                        "required": ["label", "description"],
                                        "additionalProperties": false
                                    }
                                },
                                "multiple": {"description": "Allow selecting multiple choices", "type": "boolean"}
                            },
                            "required": ["question", "header", "options"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["questions"],
                "additionalProperties": false
            });
        }
        "read" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Workspace-relative or absolute path to a small file or already-localized line range. If you only know the path, call Glob first for line_count/source_bytes."},
                    "offset": {"type": "number", "description": "The line number to start reading from (0-based)"},
                    "limit": {"type": "number", "description": "The number of lines to read (defaults to 200)"},
                    "contains": {"type": "string", "description": "Find the first matching line containing this text and return a narrow context window around it"},
                    "context_lines": {"type": "number", "description": "Number of lines before and after a contains match to return"},
                    "occurrence": {"type": "number", "description": "1-based contains match occurrence to return"}
                },
                "required": ["filePath"],
                "additionalProperties": false
            });
        }
        "apply_patch" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "patchText": {
                        "type": "string",
                        "description": "The full patch text that describes all changes to be made"
                    }
                },
                "required": ["patchText"],
                "additionalProperties": false
            });
        }
        "glob" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The glob pattern to match files against"},
                    "path": {"type": "string", "description": "The directory to search in. If not specified, the current working directory will be used. IMPORTANT: Omit this field to use the default directory. DO NOT enter \"undefined\" or \"null\" - simply omit it for the default behavior. Must be a valid directory path if provided."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            });
        }
        "grep" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The regex pattern to search for in file contents"},
                    "path": {"type": "string", "description": "The directory to search in. Defaults to the current working directory."},
                    "include": {"type": "string", "description": "File pattern to include in the search (e.g. \"*.js\", \"*.{ts,tsx}\")"}
                },
                "required": ["pattern"],
                "additionalProperties": false
            });
        }
        "edit" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Workspace-relative or absolute path to the file to modify. Prefer workspace-relative paths or exact paths returned by Glob/Grep/Read."},
                    "oldString": {"type": "string", "description": "The text to replace"},
                    "newString": {"type": "string", "description": "The text to replace it with (must be different from oldString)"},
                    "replaceAll": {"type": "boolean", "description": "Replace all occurrences of oldString (default false)"}
                },
                "required": ["filePath", "oldString", "newString"],
                "additionalProperties": false
            });
        }
        "write" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "content": {"description": "The content to write to the file", "type": "string"},
                    "filePath": {"description": "Workspace-relative or absolute path to the file to write. Prefer workspace-relative paths under the current workspace.", "type": "string"}
                },
                "required": ["content", "filePath"],
                "additionalProperties": false
            });
        }
        "task" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "description": {"description": "A short (3-5 words) description of the task", "type": "string"},
                    "prompt": {"description": "The task for the agent to perform", "type": "string"},
                    "subagent_type": {"description": "The type of specialized agent to use for this task", "type": "string", "enum": ["explore"]},
                    "session_id": {"description": "Existing Task session to continue", "type": "string"},
                    "command": {"description": "The command that triggered this task", "type": "string"}
                },
                "required": ["description", "prompt", "subagent_type"],
                "additionalProperties": false
            });
        }
        "webfetch" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "url": {"description": "The URL to fetch content from", "type": "string"},
                    "format": {
                        "description": "The format to return the content in (text, markdown, or html). Defaults to markdown.",
                        "default": "markdown",
                        "type": "string",
                        "enum": ["text", "markdown", "html"]
                    },
                    "timeout": {"description": "Optional timeout in seconds (max 120)", "type": "number"}
                },
                "required": ["url", "format"],
                "additionalProperties": false
            });
        }
        "lsp" => {
            return json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "enum": ["references", "diagnostics", "findReferences"], "description": "LSP operation. Use findReferences/references for references or diagnostics for diagnostics."},
                    "filePath": {"type": "string", "description": "The file to operate on"},
                    "path": {"type": "string", "description": "The file to operate on"},
                    "symbol": {"type": "string", "description": "Optional Rust symbol name to locate in the file."},
                    "line": {"type": "integer", "description": "The line number (1-based, as shown in editors)"},
                    "character": {"type": "integer", "description": "The character offset (1-based, as shown in editors)"},
                    "include_declaration": {"type": "boolean", "description": "Whether to include the declaration location."},
                    "max_results": {"type": "integer", "description": "Optional maximum references to return."},
                    "max_diagnostics": {"type": "integer", "description": "Optional maximum diagnostics to return."}
                },
                "required": [],
                "additionalProperties": false
            });
        }
        "bash" => {
            let directory = std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| ".".to_string());
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to execute"},
                    "timeout": {"type": "number", "description": "Optional timeout in milliseconds"},
                    "workdir": {"type": "string", "description": format!("The working directory to run the command in. Defaults to {directory}. Use this instead of 'cd' commands.")},
                    "description": {"type": "string", "description": "Clear, concise description of what this command does in 5-10 words. Examples:\nInput: ls\nOutput: Lists files in current directory\n\nInput: git status\nOutput: Shows working tree status\n\nInput: npm install\nOutput: Installs package dependencies\n\nInput: mkdir foo\nOutput: Creates directory 'foo'"}
                },
                "required": ["command", "description"],
                "additionalProperties": false
            });
        }
        "todowrite" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "todos": {
                        "description": "The updated todo list",
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"description": "Brief description of the task", "type": "string"},
                                "status": {"description": "Current status of the task: pending, in_progress, completed, cancelled", "type": "string"},
                                "priority": {"description": "Priority level of the task: high, medium, low", "type": "string"},
                                "id": {"description": "Unique identifier for the todo item", "type": "string"}
                            },
                            "required": ["content", "status", "priority", "id"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["todos"],
                "additionalProperties": false
            });
        }
        "todoread" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {},
                "additionalProperties": false
            });
        }
        "skill" => {
            return json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "name": {"description": "The skill to load. No skills are currently available.", "type": "string"}
                },
                "required": ["name"],
                "additionalProperties": false
            });
        }
        _ => {}
    }
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

fn compact_json_fallback(value: &Value) -> String {
    truncate_text(&compact_json(value), 4_000)
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

fn input_to_native_user_content(input: &Value) -> Result<String, RuntimeError> {
    let Some(object) = input.as_object() else {
        return input_to_content(input);
    };
    if let Some(task) = object
        .get("task")
        .or_else(|| object.get("query"))
        .and_then(Value::as_str)
    {
        return Ok(task.to_string());
    }
    input_to_content(input)
}

fn input_to_opencode_user_content(input: &Value) -> Result<String, RuntimeError> {
    let content = input_to_native_user_content(input)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&content).map_err(provider_error)?
    ))
}

fn input_to_native_tool_messages_with_names(
    input: &Value,
    tool_name_map: &BTreeMap<String, String>,
    opencode_style: bool,
) -> Result<Vec<Value>, RuntimeError> {
    let Some(object) = input.as_object() else {
        return Ok(vec![json!({
            "role": "user",
            "content": input_to_content(input)?
        })]);
    };

    let mut messages = vec![json!({
        "role": "user",
        "content": if opencode_style {
            input_to_opencode_user_content(input)?
        } else {
            input_to_native_user_content(input)?
        }
    })];
    let Some(observations) = object.get("observations") else {
        return Ok(messages);
    };
    let Some(items) = observations.as_array() else {
        return Ok(messages);
    };

    for (observation_index, observation) in items.iter().enumerate() {
        let Some(results) = observation.get("result").and_then(Value::as_array) else {
            continue;
        };
        let mut tool_calls = Vec::new();
        let mut tool_messages = Vec::new();
        for (result_index, result) in results.iter().enumerate() {
            let Some(result_object) = result.as_object() else {
                continue;
            };
            let tool = result_object
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let requested = observation
                .get("requested")
                .and_then(Value::as_array)
                .and_then(|items| items.get(result_index));
            let safe_name = native_history_tool_name(result_object, requested, tool, tool_name_map);
            let call_id = native_history_tool_call_id(
                result_object,
                requested,
                observation_index,
                result_index,
                &safe_name,
            );
            let arguments = result_object
                .get("input")
                .cloned()
                .unwrap_or_else(|| json!({}));

            tool_calls.push(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": safe_name,
                    "arguments": compact_json(&arguments)
                }
            }));
            tool_messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": render_native_tool_message_content(result, Some(&safe_name))
            }));
        }
        if tool_calls.is_empty() {
            continue;
        }
        let mut assistant_message = json!({
            "role": "assistant",
            "content": assistant_message_content(observation, opencode_style),
            "tool_calls": tool_calls
        });
        if opencode_style {
            if let Some(reasoning) = observation_assistant_reasoning(observation) {
                assistant_message["reasoning_content"] = Value::String(reasoning);
            }
        }
        messages.push(assistant_message);
        messages.extend(tool_messages);
    }

    Ok(messages)
}

fn assistant_message_content(observation: &Value, opencode_style: bool) -> Value {
    if opencode_style {
        observation_assistant_content_only(observation)
            .map(Value::String)
            .unwrap_or_else(|| Value::String(String::new()))
    } else {
        observation_assistant_content(observation)
            .map(Value::String)
            .unwrap_or(Value::Null)
    }
}

fn observation_assistant_content(observation: &Value) -> Option<String> {
    let assistant = observation.get("assistant")?;
    let mut parts = Vec::new();
    for pointer in [
        "/_air_assistant/reasoning",
        "/_air_assistant/content",
        "/reasoning",
        "/content",
        "/answer",
    ] {
        let Some(text) = assistant.pointer(pointer).and_then(Value::as_str) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() || parts.contains(&text) {
            continue;
        }
        parts.push(text);
    }
    if parts.is_empty() {
        None
    } else {
        Some(truncate_text(
            &parts.join("\n\n"),
            NATIVE_ASSISTANT_HISTORY_MAX_CHARS,
        ))
    }
}

fn observation_assistant_content_only(observation: &Value) -> Option<String> {
    assistant_text_from_pointers(
        observation.get("assistant")?,
        &["/_air_assistant/content", "/content", "/answer"],
    )
}

fn observation_assistant_reasoning(observation: &Value) -> Option<String> {
    assistant_text_from_pointers(
        observation.get("assistant")?,
        &["/_air_assistant/reasoning", "/reasoning"],
    )
}

fn assistant_text_from_pointers(assistant: &Value, pointers: &[&str]) -> Option<String> {
    for pointer in pointers {
        let Some(text) = assistant.pointer(pointer).and_then(Value::as_str) else {
            continue;
        };
        let text = text.trim();
        if !text.is_empty() {
            return Some(truncate_text(text, NATIVE_ASSISTANT_HISTORY_MAX_CHARS));
        }
    }
    None
}

fn native_history_tool_name(
    result: &Map<String, Value>,
    requested: Option<&Value>,
    fallback_tool: &str,
    tool_name_map: &BTreeMap<String, String>,
) -> String {
    result
        .get("_air_tool_name")
        .and_then(Value::as_str)
        .or_else(|| {
            requested
                .and_then(|value| value.get("_air_tool_name"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .unwrap_or_else(|| safe_openai_tool_name_for_history(fallback_tool, tool_name_map))
}

fn native_history_tool_call_id(
    result: &Map<String, Value>,
    requested: Option<&Value>,
    observation_index: usize,
    result_index: usize,
    safe_name: &str,
) -> String {
    result
        .get("_air_tool_call_id")
        .and_then(Value::as_str)
        .or_else(|| {
            requested
                .and_then(|value| value.get("_air_tool_call_id"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .unwrap_or_else(|| {
            generated_native_history_tool_call_id(observation_index, result_index, safe_name)
        })
}

fn safe_openai_tool_name_for_history(
    original_name: &str,
    tool_name_map: &BTreeMap<String, String>,
) -> String {
    if let Some((safe_name, _)) = tool_name_map
        .iter()
        .find(|(_, mapped_name)| mapped_name.as_str() == original_name)
    {
        return safe_name.clone();
    }
    let mut used_names = BTreeMap::new();
    safe_openai_tool_name(original_name, &mut used_names)
}

fn generated_native_history_tool_call_id(
    observation_index: usize,
    result_index: usize,
    safe_name: &str,
) -> String {
    let mut name = safe_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if name.len() > 32 {
        name.truncate(32);
    }
    format!("call_air_{observation_index}_{result_index}_{name}")
}

fn render_native_tool_message_content(value: &Value, exposed_tool_name: Option<&str>) -> String {
    let Some(object) = value.as_object() else {
        return truncate_text(&compact_json(value), 12_000);
    };
    let tool = exposed_tool_name
        .or_else(|| object.get("_air_tool_name").and_then(Value::as_str))
        .or_else(|| object.get("tool").and_then(Value::as_str))
        .unwrap_or("tool");
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut lines = Vec::new();
    if !matches!(status, "ok" | "success") {
        lines.push(format!("status: {status}"));
    }
    if let Some(error) = object.get("error").and_then(Value::as_str) {
        lines.push(format!("error: {}", truncate_text(error, 1_000)));
    }
    if let Some(output) = object.get("output") {
        render_tool_output_transcript(tool, output, &mut lines);
    }
    if lines.is_empty() {
        lines.push(compact_json_fallback(value));
    }
    let transcript = lines.join("\n");
    if matches!(
        tool,
        "file.read" | "file_read" | "read" | "file.read_many" | "file_read_many" | "read_many"
    ) {
        truncate_tail_text(&transcript, NATIVE_FILE_READ_MESSAGE_MAX_CHARS)
    } else {
        truncate_text(&transcript, NATIVE_TOOL_OUTPUT_TRANSCRIPT_MAX_CHARS)
    }
}

fn render_tool_output_transcript(tool: &str, output: &Value, lines: &mut Vec<String>) {
    match tool {
        "file.read" | "file_read" | "read" => render_file_read_transcript(output, lines),
        "file.read_many" | "file_read_many" | "read_many" => {
            render_file_read_many_transcript(output, lines)
        }
        "file.search" | "file_search" | "grep" => render_file_search_transcript(output, lines),
        "apply_patch" => render_apply_patch_transcript(output, lines),
        "file.edit" | "file_edit" | "edit" => render_file_edit_transcript(output, lines),
        "repo.files" | "repo_files" | "glob" => render_glob_transcript(output, lines),
        "repo.symbols" | "repo_symbols" => render_symbols_transcript(output, lines),
        "rust_analyzer" | "lsp" => render_lsp_transcript(output, lines),
        "bash" | "command.run" | "command_run" => render_bash_transcript(output, lines),
        "task" => render_task_transcript(output, lines),
        "todowrite" | "todo.write" | "todo_write" => render_todowrite_transcript(output, lines),
        _ => {
            lines.push(format!("output: {}", compact_json_fallback(output)));
        }
    }
}

fn task_transcript_output_text(output: &Value) -> String {
    if let Some(text) = output.get("output").and_then(Value::as_str) {
        truncate_text(text, NATIVE_TOOL_OUTPUT_TRANSCRIPT_MAX_CHARS)
    } else {
        compact_json_fallback(output)
    }
}

fn render_task_transcript(output: &Value, lines: &mut Vec<String>) {
    lines.push(task_transcript_output_text(output));
    if let Some(subagent_type) = output.get("subagent_type").and_then(Value::as_str) {
        lines.push(format!(
            "<task_metadata>\nsubagent_type: {subagent_type}\n</task_metadata>"
        ));
    }
}

fn render_file_edit_transcript(output: &Value, lines: &mut Vec<String>) {
    let applied = output
        .get("applied")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if applied {
        lines.push("Edit applied successfully.".to_string());
    } else {
        lines.push("Edit was not applied.".to_string());
    }

    if let Some(diagnostics) = output.get("diagnostics").and_then(Value::as_array) {
        for diagnostic in diagnostics.iter().take(4) {
            lines.push(format!(
                "diagnostic: {}",
                truncate_text(&compact_json(diagnostic), 1_000)
            ));
        }
    }
}

fn render_apply_patch_transcript(output: &Value, lines: &mut Vec<String>) {
    if let Some(summary) = output.get("output").and_then(Value::as_str) {
        lines.push(truncate_text(summary, 2_000));
        return;
    }
    render_file_edit_transcript(output, lines);
}

fn render_file_read_transcript(output: &Value, lines: &mut Vec<String>) {
    lines.push("<file>".to_string());
    let content = output
        .get("content_preview")
        .or_else(|| output.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if content.is_empty() {
        lines.push(compact_json_fallback(output));
    } else {
        lines.push(truncate_tail_text(
            content,
            NATIVE_FILE_READ_TRANSCRIPT_MAX_CHARS,
        ));
    }
    let end_line = output
        .get("end_line")
        .and_then(Value::as_u64)
        .or_else(|| output.get("total_lines").and_then(Value::as_u64))
        .unwrap_or(0);
    if output.get("truncated").and_then(Value::as_bool) == Some(true) {
        let max_bytes = output
            .get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(NATIVE_FILE_READ_TRANSCRIPT_MAX_CHARS as u64);
        lines.push(format!(
            "(Output truncated at {max_bytes} bytes. Use Grep, LSP, contains, or a targeted line range around known matches.)"
        ));
    } else if let Some(total_lines) = output.get("total_lines").and_then(Value::as_u64) {
        if end_line > 0 && total_lines > end_line {
            let hint = if output
                .get("range_limited_unscoped_read")
                .and_then(Value::as_bool)
                == Some(true)
            {
                "Large file preview only. Use Grep, LSP, or contains to locate symbols, then read a narrow line range around the matching lines.".to_string()
            } else {
                format!(
                    "Selected range ended at line {end_line}. Use targeted search or a specific line range for the next relevant symbol."
                )
            };
            lines.push(format!("({hint})"));
        } else {
            lines.push(format!("(End of file - total {total_lines} lines)"));
        }
    }
    lines.push("</file>".to_string());
}

fn render_file_read_many_transcript(output: &Value, lines: &mut Vec<String>) {
    let Some(files) = output.get("files").and_then(Value::as_array) else {
        lines.push(format!("output: {}", compact_json_fallback(output)));
        return;
    };
    for file in files.iter().take(8) {
        render_file_read_transcript(file, lines);
    }
    if let Some(truncation_hint) = output.get("truncation_hint").and_then(Value::as_str) {
        lines.push(format!(
            "truncation_hint: {}",
            truncate_text(truncation_hint, 1_000)
        ));
    }
}

fn render_file_search_transcript(output: &Value, lines: &mut Vec<String>) {
    let match_count = output
        .get("match_count")
        .and_then(Value::as_u64)
        .map(|count| count as usize)
        .or_else(|| {
            output
                .get("matches")
                .and_then(Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or(0);
    lines.push(format!("Found {match_count} matches"));
    if match_count == 0 {
        if let Some(hint) = output.get("search_hint").and_then(Value::as_str) {
            lines.push(hint.to_string());
        }
    }
    let Some(matches) = output.get("matches").and_then(Value::as_array) else {
        return;
    };
    let base_path = output.get("base_path").and_then(Value::as_str);
    let mut current_path = None;
    for item in matches.iter().take(40) {
        render_file_search_match(item, None, base_path, &mut current_path, lines);
    }
    if match_count > 0 {
        lines.push(
            "(Use Read with a small offset+limit around the relevant matching lines.)".to_string(),
        );
    }
}

fn render_glob_transcript(output: &Value, lines: &mut Vec<String>) {
    let Some(files) = output.get("files").and_then(Value::as_array) else {
        lines.push(compact_json_fallback(output));
        return;
    };
    if files.is_empty() {
        if let Some(hint) = output.get("search_hint").and_then(Value::as_str) {
            lines.push(hint.to_string());
        } else {
            lines.push("No files found".to_string());
        }
        return;
    }
    let file_infos = output.get("file_infos").and_then(Value::as_array);
    for file in files.iter().take(200).filter_map(Value::as_str) {
        lines.push(render_glob_file_line(file, file_infos));
    }
    if output.get("truncated").and_then(Value::as_bool) == Some(true) {
        lines.push("(Results truncated)".to_string());
    }
}

fn render_glob_file_line(path: &str, file_infos: Option<&Vec<Value>>) -> String {
    let Some(info) = file_infos
        .into_iter()
        .flatten()
        .find(|info| info.get("path").and_then(Value::as_str) == Some(path))
    else {
        return path.to_string();
    };
    let line_count = info
        .get("line_count")
        .and_then(Value::as_u64)
        .map(|lines| format!("{lines} lines"))
        .unwrap_or_else(|| "unknown lines".to_string());
    let source_bytes = info
        .get("source_bytes")
        .and_then(Value::as_u64)
        .map(|bytes| format!("{bytes} bytes"))
        .unwrap_or_else(|| "unknown bytes".to_string());
    if info.get("large").and_then(Value::as_bool) == Some(true) {
        format!("{path} ({line_count}, {source_bytes}; large, use Grep/contains/range before Read)")
    } else {
        format!("{path} ({line_count}, {source_bytes}; small, whole-file Read is reasonable)")
    }
}

fn render_bash_transcript(output: &Value, lines: &mut Vec<String>) {
    if let Some(log) = output.get("log").and_then(Value::as_str) {
        if log.trim().is_empty() {
            lines.push("(No output)".to_string());
        } else {
            lines.push(truncate_text(log, NATIVE_TOOL_OUTPUT_TRANSCRIPT_MAX_CHARS));
        }
    }
    if output.get("truncated").and_then(Value::as_bool) == Some(true) {
        let full_log = output
            .get("full_log_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        if full_log.is_empty() {
            lines.push("(Output truncated)".to_string());
        } else {
            lines.push(format!("(Output truncated. Full output: {full_log})"));
        }
    }
    if let Some(status) = output.get("status").and_then(Value::as_i64) {
        if status != 0 {
            lines.push(format!(
                "<bash_metadata>\nexit code: {status}\n</bash_metadata>"
            ));
        }
    }
}

fn render_lsp_transcript(output: &Value, lines: &mut Vec<String>) {
    if let Some(references) = output.get("references").and_then(Value::as_array) {
        lines.push(format!("references: {}", references.len()));
        for reference in references.iter().take(100) {
            lines.push(truncate_text(&compact_json(reference), 1_000));
        }
        return;
    }
    if let Some(diagnostics) = output.get("diagnostics").and_then(Value::as_array) {
        lines.push(format!("diagnostics: {}", diagnostics.len()));
        for diagnostic in diagnostics.iter().take(100) {
            lines.push(truncate_text(&compact_json(diagnostic), 1_000));
        }
        return;
    }
    lines.push(compact_json_fallback(output));
}

fn render_todowrite_transcript(output: &Value, lines: &mut Vec<String>) {
    let Some(todos) = output.get("todos").and_then(Value::as_array) else {
        lines.push(compact_json_fallback(output));
        return;
    };
    lines.push(
        serde_json::to_string_pretty(todos)
            .unwrap_or_else(|_| serde_json::to_string(todos).unwrap_or_else(|_| "[]".to_string())),
    );
}

fn render_file_search_match(
    value: &Value,
    fallback_path: Option<&str>,
    base_path: Option<&str>,
    current_path: &mut Option<String>,
    lines: &mut Vec<String>,
) {
    let Some(object) = value.as_object() else {
        return;
    };
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .or(fallback_path)
        .unwrap_or("");
    if let Some(before) = object.get("before").and_then(Value::as_array) {
        for item in before {
            render_file_search_match_line(item, Some(path), base_path, current_path, lines);
        }
    }
    render_file_search_match_line(value, Some(path), base_path, current_path, lines);
    if let Some(after) = object.get("after").and_then(Value::as_array) {
        for item in after {
            render_file_search_match_line(item, Some(path), base_path, current_path, lines);
        }
    }
}

fn render_file_search_match_line(
    value: &Value,
    fallback_path: Option<&str>,
    base_path: Option<&str>,
    current_path: &mut Option<String>,
    lines: &mut Vec<String>,
) {
    let Some(object) = value.as_object() else {
        return;
    };
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .or(fallback_path)
        .unwrap_or("");
    let line = object
        .get("line_number")
        .or_else(|| object.get("line"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let text = object
        .get("text")
        .or_else(|| object.get("line_text"))
        .or_else(|| object.get("line"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if path.is_empty() {
        lines.push(format!("Line {line}: {}", truncate_text(text, 500)));
        return;
    }
    let display_path = display_file_search_path(path, base_path);
    if current_path.as_deref() != Some(display_path.as_str()) {
        lines.push(format!("{display_path}:"));
        *current_path = Some(display_path);
    }
    lines.push(format!("  Line {line}: {}", truncate_text(text, 500)));
}

fn display_file_search_path(path: &str, base_path: Option<&str>) -> String {
    let path_value = Path::new(path);
    if path_value.is_absolute() {
        path.to_string()
    } else if let Some(base_path) = base_path {
        Path::new(base_path).join(path_value).display().to_string()
    } else {
        path.to_string()
    }
}

fn render_symbols_transcript(output: &Value, lines: &mut Vec<String>) {
    let Some(symbols) = output.get("symbols").and_then(Value::as_array) else {
        lines.push(format!("output: {}", compact_json_fallback(output)));
        return;
    };
    lines.push("symbols:".to_string());
    for symbol in symbols.iter().take(60) {
        let Some(object) = symbol.as_object() else {
            continue;
        };
        let path = object.get("path").and_then(Value::as_str).unwrap_or("");
        let line = object.get("line").and_then(Value::as_u64).unwrap_or(0);
        let end_line = object
            .get("end_line")
            .and_then(Value::as_u64)
            .unwrap_or(line);
        let kind = object.get("kind").and_then(Value::as_str).unwrap_or("");
        let name = object.get("name").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("{path}:{line}-{end_line} {kind} {name}"));
    }
}

const NATIVE_ASSISTANT_HISTORY_MAX_CHARS: usize = 32 * 1024;
const NATIVE_TOOL_OUTPUT_TRANSCRIPT_MAX_CHARS: usize = 50 * 1024;
const NATIVE_FILE_READ_TRANSCRIPT_MAX_CHARS: usize = 60 * 1024;
const NATIVE_FILE_READ_MESSAGE_MAX_CHARS: usize = 60 * 1024;

fn truncate_text(text: &str, max_chars: usize) -> String {
    truncate_middle_context_string(text, max_chars, "AIR_COMPACTED")
}

fn truncate_tail_text(text: &str, max_chars: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return "[AIR_COMPACTED]".to_string();
    }

    let mut marker = format!("\n[AIR_COMPACTED] {} chars omitted at end\n", total_chars);
    for _ in 0..4 {
        let marker_chars = marker.chars().count();
        if marker_chars >= max_chars {
            return "[AIR_COMPACTED]".chars().take(max_chars).collect();
        }
        let keep_chars = max_chars - marker_chars;
        let omitted_chars = total_chars.saturating_sub(keep_chars);
        let next_marker = format!("\n[AIR_COMPACTED] {omitted_chars} chars omitted at end\n");
        if next_marker == marker {
            let prefix = text.chars().take(keep_chars).collect::<String>();
            return format!("{prefix}{marker}");
        }
        marker = next_marker;
    }

    let marker_chars = marker.chars().count();
    if marker_chars >= max_chars {
        return "[AIR_COMPACTED]".chars().take(max_chars).collect();
    }
    let keep_chars = max_chars - marker_chars;
    let prefix = text.chars().take(keep_chars).collect::<String>();
    format!("{prefix}{marker}")
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
        Err(_) if !tool_name_map.is_empty() => {
            let mut decision = Map::new();
            decision.insert("complete".to_string(), Value::Bool(true));
            decision.insert("tool_calls".to_string(), Value::Array(Vec::new()));
            if let Some(assistant) = native_assistant_metadata(response) {
                decision.insert("_air_assistant".to_string(), assistant);
            }
            Ok(Value::Object(decision))
        }
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
        let repaired_name = if tool_name_map.contains_key(safe_name) {
            safe_name.to_string()
        } else {
            safe_name.to_ascii_lowercase()
        };
        let original_name = tool_name_map.get(&repaired_name).cloned().ok_or_else(|| {
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
        let mut normalized_call = Map::new();
        normalized_call.insert("tool".to_string(), Value::String(original_name));
        normalized_call.insert("input".to_string(), input);
        normalized_call.insert("_air_tool_name".to_string(), Value::String(repaired_name));
        if let Some(call_id) = call.get("id").and_then(Value::as_str) {
            normalized_call.insert(
                "_air_tool_call_id".to_string(),
                Value::String(call_id.to_string()),
            );
        }
        normalized_calls.push(Value::Object(normalized_call));
    }
    if normalized_calls.is_empty() {
        return Err(RuntimeError::Provider(
            "native tool call response did not contain function tool calls".to_string(),
        ));
    }

    let mut decision = Map::new();
    decision.insert("complete".to_string(), Value::Bool(false));
    decision.insert("tool_calls".to_string(), Value::Array(normalized_calls));
    if let Some(assistant) = native_assistant_metadata(response) {
        decision.insert("_air_assistant".to_string(), assistant);
    }

    Ok(Some(Value::Object(decision)))
}

fn native_assistant_metadata(response: &Value) -> Option<Value> {
    let content = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|content| !content.is_empty());
    let reasoning = response
        .pointer("/choices/0/message/reasoning_content")
        .or_else(|| response.pointer("/choices/0/message/reasoning"))
        .or_else(|| response.pointer("/choices/0/message/thinking"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reasoning| !reasoning.is_empty());
    if content.is_none() && reasoning.is_none() {
        return None;
    }

    let mut assistant = Map::new();
    if let Some(content) = content {
        assistant.insert(
            "content".to_string(),
            Value::String(truncate_text(content, NATIVE_ASSISTANT_HISTORY_MAX_CHARS)),
        );
    }
    if let Some(reasoning) = reasoning {
        assistant.insert(
            "reasoning".to_string(),
            Value::String(truncate_text(reasoning, NATIVE_ASSISTANT_HISTORY_MAX_CHARS)),
        );
    }
    Some(Value::Object(assistant))
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
    fn request_stats_include_provider_request_only_when_enabled() {
        let body = json!({
            "model": "glm-5.1",
            "messages": [{"role": "user", "content": "inspect the code"}],
            "tools": [{"type": "function", "function": {"name": "file_read"}}]
        });

        let plain = chat_completion_request_stats(&body, false);
        assert_eq!(plain.provider_user_content_bytes, "inspect the code".len());
        assert!(plain.provider_request.is_none());
        assert!(plain.provider_response.is_none());

        let traced = chat_completion_request_stats(&body, true);
        assert_eq!(traced.provider_request, Some(body));
        assert!(traced.provider_response.is_none());
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
    fn parse_config_file_accepts_provider_io_trace_flag() {
        let path = temp_file_path("air-model-config-provider-io", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "planner": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "gpt-5.1",
                  "trace_provider_io": true
                }
              }
            }"#,
        )
        .unwrap();

        let config = parse_config_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(config.models["planner"].trace_provider_io, Some(true));
    }

    #[test]
    fn parse_config_file_accepts_opencode_tool_mode() {
        let path = temp_file_path("air-model-config-opencode-tool-mode", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "code_edit_decider": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "GLM-5.1",
                  "native_tool_calls": true,
                  "opencode_tool_mode": "edit_write"
                }
              }
            }"#,
        )
        .unwrap();

        let config = parse_config_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(
            config.models["code_edit_decider"].opencode_tool_mode,
            Some(OpenCodeToolMode::EditWrite)
        );
    }

    #[test]
    fn parse_config_file_rejects_invalid_opencode_tool_mode() {
        let path = temp_file_path("air-model-config-opencode-tool-mode-invalid", "json");
        fs::write(
            &path,
            r#"{
              "models": {
                "code_edit_decider": {
                  "base_url": "https://example.com/v1",
                  "api_key_env": "OPENAI_API_KEY",
                  "model": "GLM-5.1",
                  "opencode_tool_mode": "unknown"
                }
              }
            }"#,
        )
        .unwrap();

        let error = parse_config_file(&path).unwrap_err();
        let _ = fs::remove_file(&path);

        assert!(error.to_string().contains("unknown variant"));
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
            opencode_tool_mode: None,
            trace_provider_io: None,
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
            opencode_tool_mode: None,
            trace_provider_io: None,
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
            json_mode: Some(true),
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "glm-5.1".to_string(),
            &json!({
                "task": "choose tools",
                "allowed_tools": ["edit", "repo.search", "repo.context"],
                "tool_schemas": {
                    "edit": {
                        "required": {
                            "filePath": "repo-relative file path",
                            "oldString": "exact text to replace",
                            "newString": "replacement text"
                        },
                        "optional": {"replaceAll": "boolean"}
                    },
                    "repo.search": {
                        "required": {"query": "search text"}
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(request.tool_name_map["edit"], "edit");
        assert_eq!(request.tool_name_map["repo_search"], "repo.search");
        assert!(!request
            .tool_name_map
            .values()
            .any(|name| name == "repo.context"));
        assert_eq!(request.body["tool_choice"], json!("auto"));
        assert_eq!(
            request.body["tools"][0]["function"]["parameters"]["required"],
            json!(["filePath", "oldString", "newString"])
        );
        assert!(
            request.body.get("response_format").is_none(),
            "native tool calls should not force JSON response_format; provider tools carry the action schema"
        );
        assert!(
            request.body["tools"][0]["function"]["parameters"]["properties"]
                .get("edits")
                .is_none()
        );
        assert_eq!(
            request.body["tools"][0]["function"]["parameters"]["additionalProperties"],
            json!(false)
        );
        let content = request.body["messages"][0]["content"].as_str().unwrap();
        assert_eq!(content, "choose tools");
        assert!(
            !content.contains("tool_schemas") && !content.contains("allowed_tools"),
            "native tool mode must pass tool definitions through provider tools, not duplicate them in user content"
        );
        assert!(
            serde_json::from_str::<Value>(content).is_err(),
            "native tool content should be OpenCode-style user text, not an AIR JSON envelope"
        );
    }

    #[test]
    fn native_tool_calls_default_to_all_tool_schemas_without_allowed_tools() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "glm-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: Some(true),
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "glm-5.1".to_string(),
            &json!({
                "task": "edit the code",
                "tool_schemas": {
                    "read": {
                        "required": {"filePath": "repo-relative file path"}
                    },
                    "edit": {
                        "required": {
                            "filePath": "repo-relative file path",
                            "oldString": "text to replace",
                            "newString": "replacement text"
                        }
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(request.tool_name_map["edit"], "edit");
        assert_eq!(request.tool_name_map["read"], "read");
        assert_eq!(request.body["tool_choice"], json!("auto"));
        assert_eq!(request.body["tools"].as_array().unwrap().len(), 2);
        assert_eq!(
            request.body["messages"][0]["content"],
            json!("edit the code")
        );
    }

    #[test]
    fn opencode_style_native_tool_calls_use_streaming_like_opencode() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "glm-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: Some(OPENCODE_QWEN_PROMPT_MARKER.to_string()),
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "glm-5.1".to_string(),
            &json!({
                "task": "edit the code",
                "allowed_tools": ["read", "edit", "bash"],
                "tool_schemas": {
                    "read": {"required": {"filePath": "repo-relative file path"}},
                    "edit": {
                        "required": {
                            "filePath": "repo-relative file path",
                            "oldString": "text to replace",
                            "newString": "replacement text"
                        }
                    },
                    "bash": {"required": {"command": "command to run"}}
                }
            }),
        )
        .unwrap();

        assert_eq!(request.body["stream"], json!(true));
        assert_eq!(
            request.body["stream_options"],
            json!({"include_usage": true})
        );
    }

    #[test]
    fn opencode_style_preserves_request_model_case() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "GLM-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: Some(OPENCODE_QWEN_PROMPT_MARKER.to_string()),
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "GLM-5.1".to_string(),
            &json!({
                "task": "edit the code",
                "allowed_tools": ["read", "edit", "bash"]
            }),
        )
        .unwrap();

        assert_eq!(request.body["model"], json!("GLM-5.1"));
    }

    #[test]
    fn opencode_style_gpt_models_use_apply_patch_instead_of_edit_write() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "gpt-5.3-codex:high".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: Some(OPENCODE_QWEN_PROMPT_MARKER.to_string()),
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "gpt-5.3-codex:high".to_string(),
            &json!({
                "task": "edit the code",
                "allowed_tools": ["read", "edit", "write", "apply_patch", "bash"]
            }),
        )
        .unwrap();

        let exposed = request
            .body
            .get("tools")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .and_then(|name| request.tool_name_map.get(name))
                    .cloned()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(exposed.contains(&"apply_patch".to_string()));
        assert!(!exposed.contains(&"edit".to_string()));
        assert!(!exposed.contains(&"write".to_string()));
        assert_eq!(request.body["reasoning_effort"], json!("medium"));
        let apply_patch = request.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "apply_patch")
            .unwrap();
        assert_eq!(
            apply_patch["function"]["parameters"]["required"],
            json!(["patchText"])
        );
    }

    #[test]
    fn opencode_style_glm_models_keep_edit_write_tools() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "glm-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: Some(OPENCODE_QWEN_PROMPT_MARKER.to_string()),
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "glm-5.1".to_string(),
            &json!({
                "task": "edit the code",
                "allowed_tools": ["read", "edit", "write", "apply_patch", "bash"]
            }),
        )
        .unwrap();

        let exposed = request
            .body
            .get("tools")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .and_then(|name| request.tool_name_map.get(name))
                    .cloned()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(exposed.contains(&"edit".to_string()));
        assert!(exposed.contains(&"write".to_string()));
        assert!(!exposed.contains(&"apply_patch".to_string()));
    }

    #[test]
    fn opencode_style_prioritizes_locator_tools_before_read() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "glm-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: Some(OPENCODE_QWEN_PROMPT_MARKER.to_string()),
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "glm-5.1".to_string(),
            &json!({
                "task": "inspect code",
                "allowed_tools": ["question", "bash", "read", "glob", "grep", "lsp", "edit", "write", "task"]
            }),
        )
        .unwrap();

        let exposed = request
            .body
            .get("tools")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .and_then(|name| request.tool_name_map.get(name))
                    .cloned()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let grep_index = exposed.iter().position(|tool| tool == "grep").unwrap();
        let glob_index = exposed.iter().position(|tool| tool == "glob").unwrap();
        let lsp_index = exposed.iter().position(|tool| tool == "lsp").unwrap();
        let task_index = exposed.iter().position(|tool| tool == "task").unwrap();
        let read_index = exposed.iter().position(|tool| tool == "read").unwrap();
        assert!(grep_index < read_index, "{exposed:?}");
        assert!(glob_index < read_index, "{exposed:?}");
        assert!(lsp_index < read_index, "{exposed:?}");
        assert!(task_index < read_index, "{exposed:?}");

        let system = request.body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("Prefer locator tools before reading file bodies"));
        assert!(system.contains("Use Glob first when you only know a file path"));
        assert!(system.contains("200 lines or fewer and 20KB or less"));
        assert!(!system.contains("Use Read after you have a target file"));
    }

    #[test]
    fn opencode_style_explicit_apply_patch_mode_overrides_model_name() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "GLM-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: Some(OPENCODE_QWEN_PROMPT_MARKER.to_string()),
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: Some(OpenCodeToolMode::ApplyPatch),
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "GLM-5.1".to_string(),
            &json!({
                "task": "edit the code",
                "allowed_tools": ["read", "edit", "write", "apply_patch", "bash"]
            }),
        )
        .unwrap();

        let exposed = request.tool_name_map.values().cloned().collect::<Vec<_>>();
        assert!(exposed.contains(&"apply_patch".to_string()));
        assert!(!exposed.contains(&"edit".to_string()));
        assert!(!exposed.contains(&"write".to_string()));
    }

    #[test]
    fn native_tool_calls_include_lsp_tool_schemas() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "glm-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: Some(true),
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let request = build_chat_completion_body(
            &config,
            "glm-5.1".to_string(),
            &json!({
                "task": "inspect references",
                "tool_schemas": {
                    "lsp": {
                        "required": {},
                        "optional": {
                            "command": "references or diagnostics",
                            "path": "repo-relative Rust file path",
                            "symbol": "Rust symbol name"
                        }
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(request.tool_name_map["lsp"], "lsp");
        let tools = request.body["tools"].as_array().unwrap();
        let lsp = tools
            .iter()
            .find(|tool| tool["function"]["name"] == "lsp")
            .unwrap();
        assert_eq!(lsp["function"]["parameters"]["required"], json!([]));
        assert!(lsp["function"]["description"]
            .as_str()
            .unwrap()
            .contains("Language Server Protocol"));
        assert_eq!(
            lsp["function"]["parameters"]["properties"]["line"]["description"],
            json!("The line number (1-based, as shown in editors)")
        );
        assert_eq!(
            lsp["function"]["parameters"]["properties"]["command"]["enum"],
            json!(["references", "diagnostics", "findReferences"])
        );
    }

    #[test]
    fn native_read_tool_schema_matches_opencode_style_ranges() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "glm-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: Some(OPENCODE_QWEN_PROMPT_MARKER.to_string()),
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };

        let request = build_chat_completion_body(
            &config,
            "glm-5.1".to_string(),
            &json!({
                "task": "read file",
                "allowed_tools": ["read"]
            }),
        )
        .unwrap();

        let read_tool = request.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "read")
            .unwrap();
        let description = read_tool["function"]["description"].as_str().unwrap();
        assert!(description.starts_with("Read file content only after"));
        assert!(description.contains("use Grep, LSP, or contains first"));
        assert!(description.contains("use Glob first to inspect line_count"));
        assert!(description.contains("larger than 200 lines or 20KB"));
        assert!(description.contains("workspace-relative"));
        assert!(description.contains("Do not invent absolute paths"));
        assert!(description.contains("targeted line ranges"));
        assert!(!description.contains("Reads a file from the local filesystem"));
        assert!(!description.contains("speculatively read multiple files"));
        assert!(!description.contains("recommended to read the whole file"));
        assert!(!description.contains("If the User provides a path to a file assume"));
        assert!(!description.contains("must be an absolute path"));
        let properties = &read_tool["function"]["parameters"]["properties"];
        assert!(properties.get("filePath").is_some());
        assert!(properties["filePath"]["description"]
            .as_str()
            .unwrap()
            .contains("call Glob first"));
        assert!(properties.get("offset").is_some());
        assert!(properties.get("limit").is_some());
        assert!(properties.get("contains").is_some());
        assert!(properties.get("context_lines").is_some());
        assert!(properties.get("occurrence").is_some());
        assert_eq!(
            properties["limit"]["description"],
            json!("The number of lines to read (defaults to 200)")
        );
        assert!(properties.get("start_line").is_none());
        assert!(properties.get("end_line").is_none());
        assert!(properties.get("repeat_reason").is_none());
        assert_eq!(
            read_tool["function"]["parameters"]["additionalProperties"],
            json!(false)
        );
        assert!(
            read_tool["function"].get("strict").is_none(),
            "OpenCode-style tool schemas should not include the OpenAI strict extension"
        );
    }

    #[test]
    fn native_opencode_history_replays_assistant_reasoning_and_content() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "glm-5.1".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: Some(OPENCODE_QWEN_PROMPT_MARKER.to_string()),
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let input = json!({
            "task": "edit the helper",
            "allowed_tools": ["edit"],
            "observations": [{
                "action": "tool_result",
                "assistant": {
                    "_air_assistant": {
                        "reasoning": "I found the exact helper and can patch it now.",
                        "content": "I'll apply the local edit."
                    },
                    "tool_calls": [{
                        "tool": "edit",
                        "input": {
                            "filePath": "src/lib.rs",
                            "oldString": "old",
                            "newString": "new"
                        },
                        "_air_tool_name": "edit",
                        "_air_tool_call_id": "call_edit_1"
                    }]
                },
                "requested": [{
                    "tool": "edit",
                    "input": {
                        "filePath": "src/lib.rs",
                        "oldString": "old",
                        "newString": "new"
                    },
                    "_air_tool_name": "edit",
                    "_air_tool_call_id": "call_edit_1"
                }],
                "result": [{
                    "tool": "edit",
                    "status": "ok",
                    "_air_tool_name": "edit",
                    "_air_tool_call_id": "call_edit_1",
                    "output": {"applied": true}
                }]
            }]
        });

        let request = build_chat_completion_body(&config, "glm-5.1".to_string(), &input).unwrap();
        let messages = request.body["messages"].as_array().unwrap();
        let assistant = messages
            .iter()
            .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
            .unwrap();

        assert_eq!(assistant["content"], json!("I'll apply the local edit."));
        assert_eq!(
            assistant["reasoning_content"],
            json!("I found the exact helper and can patch it now.")
        );
    }

    #[test]
    fn native_tool_messages_replay_observations_as_tool_history() {
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
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let long_content = "x".repeat(60_000);
        let input = json!({
            "task": "inspect",
            "allowed_tools": ["file.read"],
            "tool_schemas": {
                "file.read": {
                    "required": {"path": "repo-relative path"}
                }
            },
            "observations": [{
                "action": "tool_batch_dispatch",
                "assistant": {
                    "_air_assistant": {
                        "content": "I will inspect the narrow target file first.",
                        "reasoning": "Need the current implementation before editing."
                    },
                    "complete": false,
                    "tool_calls": [{
                        "tool": "file.read",
                        "input": {"path": "src/lib.rs"},
                        "_air_tool_name": "file_read",
                        "_air_tool_call_id": "call_original_123"
                    }]
                },
                "requested": [{
                    "tool": "file.read",
                    "input": {"path": "src/lib.rs", "irrelevant": long_content},
                    "_air_tool_name": "file_read",
                    "_air_tool_call_id": "call_original_123"
                }],
                "result": [{
                    "tool": "file.read",
                    "status": "ok",
                    "input": {"path": "src/lib.rs"},
                    "_air_tool_name": "file_read",
                    "_air_tool_call_id": "call_original_123",
                    "output": {
                        "path": "src/lib.rs",
                        "start_line": 1,
                        "end_line": 500,
                        "content": long_content,
                        "artifacts": [{"id": 1, "content": long_content}]
                    }
                }]
            }]
        });

        let request = build_chat_completion_body(&config, "glm-5.1".to_string(), &input).unwrap();
        let messages = request.body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[0]["content"], json!("inspect"));
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert!(
            messages[1]["content"]
                .as_str()
                .is_some_and(|content| content.contains("I will inspect the narrow target file first.")),
            "native replay should preserve assistant text so the next call does not rediscover prior conclusions"
        );
        assert!(
            serde_json::to_string(&request.body)
                .unwrap()
                .contains("I will inspect the narrow target file first."),
            "native replay should include prior assistant prose like OpenCode session history"
        );
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["name"],
            json!("file_read")
        );
        assert_eq!(
            messages[1]["tool_calls"][0]["id"],
            json!("call_original_123")
        );
        assert_eq!(messages[2]["role"], json!("tool"));
        assert!(messages[2].get("name").is_none());
        assert_eq!(messages[2]["tool_call_id"], json!("call_original_123"));
        let content = messages[2]["content"].as_str().unwrap();

        assert!(content.contains("<file>"), "{content}");
        assert!(content.contains("</file>"), "{content}");
        assert!(!content.contains("[AIR_COMPACTED]"), "{content}");
        assert!(
            content.len() > 50_000,
            "native read transcript should preserve roughly OpenCode's 50KB read budget, got {} chars",
            content.len()
        );
        assert!(!content.contains("artifact_refs"));
        assert!(!content.contains("\"artifacts\""));
        assert!(!content.contains("\"tool_schemas\""));
        assert!(!content.contains("\"recent_tool_results\""));
        assert!(!content.contains("\"observations\""));
        assert!(!content.contains("rationale: read file"));
        assert!(content.len() < 70_000, "{content}");
    }

    #[test]
    fn native_read_transcript_reports_more_lines_for_bounded_range() {
        let input = json!({
            "task": "inspect",
            "observations": [{
                "action": "file_read",
                "requested": [{
                    "tool": "read",
                    "input": {"filePath": "src/lib.rs", "offset": 100}
                }],
                "result": [{
                    "tool": "read",
                    "status": "ok",
                    "input": {"filePath": "src/lib.rs", "offset": 100},
                    "output": {
                        "path": "src/lib.rs",
                        "content": "00101| fn helper() {}",
                        "start_line": 101,
                        "end_line": 300,
                        "total_lines": 350,
                        "truncated": false,
                        "content_format": "line_numbered"
                    }
                }]
            }]
        });

        let messages =
            input_to_native_tool_messages_with_names(&input, &BTreeMap::new(), false).unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert!(content.contains("00101| fn helper() {}"), "{content}");
        assert!(
            content.contains("Selected range ended at line 300"),
            "{content}"
        );
        assert!(content.contains("targeted search"), "{content}");
        assert!(!content.contains("Use 'offset' parameter"), "{content}");
        assert!(!content.contains("End of file"), "{content}");
    }

    #[test]
    fn native_tool_messages_preserve_structured_tool_error_feedback() {
        let input = json!({
            "task": "avoid loop",
            "observations": [{
                "action": "tool_batch_dispatch",
                "rationale": "same read again",
                "requested": [{
                    "tool": "file.read",
                    "input": {"path": "src/lib.rs"}
                }],
                "result": [{
                    "tool": "file.read",
                    "status": "error",
                    "input": {"path": "src/lib.rs"},
                    "error": "policy.max_repeated_tool_calls exceeded for tool file.read: limit=3 attempted=4",
                    "output": {
                        "status": "error",
                        "error": "policy.max_repeated_tool_calls exceeded for tool file.read: limit=3 attempted=4",
                        "error_code": "doom_loop",
                        "permission": "doom_loop",
                        "message": "Use existing observations or choose a different next action."
                    }
                }]
            }]
        });

        let messages =
            input_to_native_tool_messages_with_names(&input, &BTreeMap::new(), false).unwrap();
        let content = messages[2]["content"].as_str().unwrap();
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert_eq!(messages[2]["role"], json!("tool"));
        assert!(!content.contains("<tool_results>"), "{content}");
        assert!(content.contains("error_code"), "{content}");
        assert!(content.contains("doom_loop"), "{content}");
        assert!(content.contains("permission"), "{content}");
        assert!(content.contains("different next action"), "{content}");
        assert!(
            serde_json::from_str::<Value>(content).is_err(),
            "native tool result content should be plain text"
        );
    }

    #[test]
    fn native_tool_messages_render_task_output_as_subagent_answer() {
        let input = json!({
            "task": "continue after subagent",
            "observations": [{
                "action": "tool_batch_dispatch",
                "requested": [{
                    "tool": "task",
                    "input": {"description": "Inspect helper", "subagent_type": "explore"}
                }],
                "result": [{
                    "tool": "task",
                    "status": "ok",
                    "input": {"description": "Inspect helper", "subagent_type": "explore"},
                    "output": {
                        "output": "target file: crates/air-tools/src/subagent_tools.rs\nnext action: read lines 80-115",
                        "subagent_type": "explore",
                        "raw_output_bytes": 33937
                    }
                }]
            }]
        });

        let messages =
            input_to_native_tool_messages_with_names(&input, &BTreeMap::new(), false).unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert!(content.contains("target file: crates/air-tools/src/subagent_tools.rs"));
        assert!(content.contains("next action: read lines 80-115"));
        assert!(content.contains("subagent_type: explore"));
        assert!(!content.contains("raw_output_bytes"), "{content}");
    }

    #[test]
    fn opencode_tool_messages_render_task_output_as_subagent_answer() {
        let input = json!({
            "task": "continue after subagent",
            "observations": [{
                "action": "tool_batch_dispatch",
                "requested": [{
                    "tool": "task",
                    "input": {"description": "Inspect helper", "subagent_type": "explore"}
                }],
                "result": [{
                    "_air_tool_call_id": "call_task_1",
                    "_air_tool_name": "task",
                    "tool": "task",
                    "status": "ok",
                    "input": {"description": "Inspect helper", "subagent_type": "explore"},
                    "output": {
                        "output": "Findings:\n- crates/air-tools/src/subagent_tools.rs:135-147 `normalize_subagent_command_output` - raw log lookup lives here\nNext action:\n- read crates/air-tools/src/subagent_tools.rs lines 131-150",
                        "subagent_type": "explore",
                        "raw_output_bytes": 6530
                    }
                }]
            }]
        });

        let mut tool_name_map = BTreeMap::new();
        tool_name_map.insert("task".to_string(), "task".to_string());
        let messages =
            input_to_native_tool_messages_with_names(&input, &tool_name_map, true).unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert!(content.contains("Findings:"), "{content}");
        assert!(
            content.contains("normalize_subagent_command_output"),
            "{content}"
        );
        assert!(content.contains("Next action:"), "{content}");
        assert!(!content.contains("raw_output_bytes"), "{content}");
    }

    #[test]
    fn opencode_task_description_encourages_proactive_exploration() {
        let description = opencode_task_description();

        assert!(description.contains("Use the explore subagent proactively"));
        assert!(description.contains("broad or open-ended codebase investigation"));
        assert!(!description.contains("- general:"), "{description}");
    }

    #[test]
    fn native_tool_messages_render_edit_like_opencode_without_diff_or_snippets() {
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
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let large_diff = "diff-line\n".repeat(5_000);
        let input = json!({
            "task": "continue after edit",
            "observations": [{
                "action": "tool_batch_dispatch",
                "assistant": {
                    "_air_assistant": {
                        "content": "Apply the helper extraction."
                    },
                    "tool_calls": [{
                        "tool": "edit",
                        "input": {"filePath": "src/lib.rs"},
                        "_air_tool_name": "edit",
                        "_air_tool_call_id": "call_edit_1"
                    }]
                },
                "requested": [{
                    "tool": "edit",
                    "input": {"filePath": "src/lib.rs"},
                    "_air_tool_name": "edit",
                    "_air_tool_call_id": "call_edit_1"
                }],
                "result": [{
                    "tool": "edit",
                    "status": "ok",
                    "_air_tool_name": "edit",
                    "_air_tool_call_id": "call_edit_1",
                    "output": {
                        "path": "src/lib.rs",
                        "applied": true,
                        "replacements": 1,
                        "edit_count": 1,
                        "files": [{"path": "src/lib.rs"}],
                        "diff": large_diff,
                        "post_edit_snippets": [{
                            "path": "src/lib.rs",
                            "start_line": 10,
                            "end_line": 12,
                            "content": "00010| fn helper() {}\n00011| fn caller() {}"
                        }]
                    }
                }]
            }]
        });

        let request = build_chat_completion_body(&config, "glm-5.1".to_string(), &input).unwrap();
        let messages = request.body["messages"].as_array().unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert_eq!(content, "Edit applied successfully.");
        assert!(!content.contains("<content>"), "{content}");
        assert!(!content.contains("fn helper"), "{content}");
        assert!(!content.contains("diff-line"), "{content}");
    }

    #[test]
    fn native_tool_messages_render_apply_patch_updated_files_summary() {
        let config = OpenAiModelConfig {
            base_url: Some("https://configured.example/v1".to_string()),
            base_url_env: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            model: "gpt-5.3-codex:high".to_string(),
            model_env: None,
            temperature: None,
            request_timeout_seconds: None,
            system_prompt: None,
            json_mode: None,
            response_format: None,
            extra_body: None,
            native_tool_calls: Some(true),
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let large_diff = "diff-line\n".repeat(5_000);
        let input = json!({
            "task": "continue after patch",
            "observations": [{
                "action": "tool_batch_dispatch",
                "assistant": {
                    "_air_assistant": {"content": ""},
                    "tool_calls": [{
                        "tool": "apply_patch",
                        "input": {"patchText": "*** Begin Patch\n*** End Patch"},
                        "_air_tool_name": "apply_patch",
                        "_air_tool_call_id": "call_patch_1"
                    }]
                },
                "requested": [{
                    "tool": "apply_patch",
                    "input": {"patchText": "*** Begin Patch\n*** End Patch"},
                    "_air_tool_name": "apply_patch",
                    "_air_tool_call_id": "call_patch_1"
                }],
                "result": [{
                    "tool": "apply_patch",
                    "status": "ok",
                    "_air_tool_name": "apply_patch",
                    "_air_tool_call_id": "call_patch_1",
                    "output": {
                        "applied": true,
                        "output": "Success. Updated the following files:\nM crates/air-tools/src/http_tools.rs",
                        "files": [{"path": "crates/air-tools/src/http_tools.rs"}],
                        "diff": large_diff
                    }
                }]
            }]
        });

        let request =
            build_chat_completion_body(&config, "gpt-5.3-codex:high".to_string(), &input).unwrap();
        let messages = request.body["messages"].as_array().unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert_eq!(
            content,
            "Success. Updated the following files:\nM crates/air-tools/src/http_tools.rs"
        );
        assert!(!content.contains("diff-line"), "{content}");
    }

    #[test]
    fn native_tool_messages_preserve_symbol_navigation_output() {
        let input = json!({
            "task": "inspect helper",
            "observations": [{
                "action": "code_context",
                "requested": [{
                    "tool": "repo.symbols",
                    "input": {
                        "path": "crates/air-tools/src/file_tools.rs",
                        "query": "maybe_save_full_output"
                    }
                }],
                "result": [{
                    "tool": "repo.symbols",
                    "status": "ok",
                    "input": {
                        "path": "crates/air-tools/src/file_tools.rs",
                        "query": "maybe_save_full_output"
                    },
                    "output": {
                        "repo": "/Users/hwang/work/air",
                        "query": "maybe_save_full_output",
                        "effective_query": "maybe_save_full_output",
                        "mode": "fixed",
                        "bytes": 72,
                        "symbols": [{
                            "name": "maybe_save_full_output",
                            "kind": "function",
                            "path": "crates/air-tools/src/file_tools.rs",
                            "line": 473,
                            "end_line": 487,
                            "text": "fn maybe_save_full_output("
                        }]
                    }
                }]
            }]
        });

        let messages =
            input_to_native_tool_messages_with_names(&input, &BTreeMap::new(), false).unwrap();
        let content = messages[2]["content"].as_str().unwrap();
        assert!(content.contains("symbols:"), "{content}");
        assert!(
            content.contains(
                "crates/air-tools/src/file_tools.rs:473-487 function maybe_save_full_output"
            ),
            "{content}"
        );
        assert!(!content.contains("\"recent_tool_results\""));
    }

    #[test]
    fn native_tool_messages_render_file_search_lines_and_context() {
        let input = json!({
            "task": "find helper",
            "observations": [{
                "action": "tool_result",
                "result": [{
                    "tool": "grep",
                    "status": "ok",
                    "input": {
                        "path": "src/lib.rs",
                        "pattern": "repo_search_query_input",
                        "contextLines": 1
                    },
                    "output": {
                        "match_count": 1,
                        "base_path": "/repo",
                        "matches": [{
                            "path": "src/lib.rs",
                            "line_number": 42,
                            "line": "fn repo_search_query_input() {}",
                            "before": [{
                                "path": "src/lib.rs",
                                "line_number": 41,
                                "line": "fn required_input_string() {}"
                            }],
                            "after": [{
                                "path": "src/lib.rs",
                                "line_number": 43,
                                "line": "fn repo_glob_input() {}"
                            }]
                        }]
                    }
                }]
            }]
        });

        let messages =
            input_to_native_tool_messages_with_names(&input, &BTreeMap::new(), false).unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert!(content.contains("Found 1 matches"), "{content}");
        assert!(content.contains("src/lib.rs:\n"), "{content}");
        assert!(
            content.contains("  Line 41: fn required_input_string() {}"),
            "{content}"
        );
        assert!(
            content.contains("  Line 42: fn repo_search_query_input() {}"),
            "{content}"
        );
        assert!(
            content.contains("  Line 43: fn repo_glob_input() {}"),
            "{content}"
        );
        assert!(
            content.contains("small offset+limit around the relevant matching lines"),
            "{content}"
        );
        assert!(!content.contains(":0:"), "{content}");
    }

    #[test]
    fn native_tool_messages_render_glob_file_size_metadata() {
        let input = json!({
            "task": "continue after glob",
            "observations": [{
                "action": "tool_result",
                "result": [{
                    "tool": "glob",
                    "status": "ok",
                    "input": {"pattern": "src/*.rs"},
                    "output": {
                        "files": ["src/small.rs", "src/large.rs"],
                        "file_infos": [
                            {
                                "path": "src/small.rs",
                                "line_count": 40,
                                "source_bytes": 1200,
                                "large": false
                            },
                            {
                                "path": "src/large.rs",
                                "line_count": 950,
                                "source_bytes": 48000,
                                "large": true
                            }
                        ]
                    }
                }]
            }]
        });

        let messages =
            input_to_native_tool_messages_with_names(&input, &BTreeMap::new(), false).unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert!(
            content.contains("src/small.rs (40 lines, 1200 bytes; small"),
            "{content}"
        );
        assert!(
            content.contains("src/large.rs (950 lines, 48000 bytes; large"),
            "{content}"
        );
        assert!(
            content.contains("use Grep/contains/range before Read"),
            "{content}"
        );
    }

    #[test]
    fn native_tool_messages_render_file_search_no_match_hint() {
        let input = json!({
            "task": "find helper",
            "observations": [{
                "action": "tool_result",
                "result": [{
                    "tool": "grep",
                    "status": "ok",
                    "input": {
                        "pattern": "fn test.*file_search"
                    },
                    "output": {
                        "match_count": 0,
                        "matches": [],
                        "search_hint": "No matches in the current directory scope. Read a known likely file instead of repeating speculative searches."
                    }
                }]
            }]
        });

        let messages =
            input_to_native_tool_messages_with_names(&input, &BTreeMap::new(), false).unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert!(content.contains("Found 0 matches"), "{content}");
        assert!(
            content.contains("Read a known likely file instead of repeating speculative searches."),
            "{content}"
        );
    }

    #[test]
    fn native_tool_messages_render_read_many_files_as_file_snippets() {
        let input = json!({
            "task": "move helper",
            "observations": [{
                "action": "file_read_many",
                "requested": [{
                    "tool": "read_many",
                    "input": {
                        "files": ["src/lib.rs", "src/git_tools.rs"],
                        "line_numbers": true
                    }
                }],
                "result": [{
                    "tool": "read_many",
                    "status": "ok",
                    "input": {
                        "files": ["src/lib.rs", "src/git_tools.rs"],
                        "line_numbers": true
                    },
                    "output": {
                        "file_count": 2,
                        "bytes": 128,
                        "truncated": false,
                        "files": [{
                            "path": "src/lib.rs",
                            "content": "10 fn old_helper() {}\n11 fn keep() {}",
                            "start_line": 10,
                            "end_line": 11,
                            "content_format": "line_numbered"
                        }, {
                            "path": "src/diff_tools.rs",
                            "content": "1 pub(super) fn diff_paths() {}",
                            "start_line": 1,
                            "end_line": 1,
                            "content_format": "line_numbered"
                        }]
                    }
                }]
            }]
        });

        let messages =
            input_to_native_tool_messages_with_names(&input, &BTreeMap::new(), false).unwrap();
        let content = messages[2]["content"].as_str().unwrap();

        assert!(content.contains("<file>"), "{content}");
        assert!(content.contains("fn old_helper"), "{content}");
        assert!(content.contains("fn diff_paths"), "{content}");
        assert!(!content.contains("\"tool_transcript\""));
    }

    #[test]
    fn native_tool_messages_preserve_current_run_tool_results_like_opencode() {
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
            opencode_tool_mode: None,
            trace_provider_io: None,
        };
        let target_context = "target-context-".repeat(1_000);
        let todo_content = "todo updated";
        let observations = (0..2)
            .map(|index| {
                if index == 1 {
                    json!({
                        "action": "tool_batch_dispatch",
                        "requested": [{
                            "tool": "todowrite",
                            "input": {"todos": [{"content": todo_content, "status": "completed"}]}
                        }],
                        "result": [{
                            "tool": "todowrite",
                            "status": "ok",
                            "input": {"todos": [{"content": todo_content, "status": "completed"}]},
                            "output": {
                                "todos": [{"content": todo_content, "status": "completed"}]
                            }
                        }]
                    })
                } else {
                    json!({
                        "action": "tool_batch_dispatch",
                        "requested": [{
                            "tool": "file.read",
                            "input": {"path": "src/lib.rs"}
                        }],
                        "result": [{
                            "tool": "file.read",
                            "status": "ok",
                            "input": {"path": "src/lib.rs"},
                            "output": {
                                "path": "src/lib.rs",
                                "content": target_context
                            }
                        }]
                    })
                }
            })
            .collect::<Vec<_>>();
        let input = json!({
            "task": "inspect",
            "allowed_tools": ["file.read", "todowrite"],
            "tool_schemas": {
                "file.read": {
                    "required": {"path": "repo-relative path"}
                },
                "todowrite": {
                    "required": {"todos": "todo list"}
                }
            },
            "observations": observations
        });

        let request = build_chat_completion_body(&config, "glm-5.1".to_string(), &input).unwrap();
        let messages = request.body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"], json!("inspect"));
        let content = messages
            .iter()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
            .filter_map(|message| message.get("content").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            content.contains("target-context-"),
            "current-run file context should remain available"
        );
        assert!(
            content.contains(todo_content),
            "recent todo output should remain available"
        );
        assert!(
            content.contains("\"status\": \"completed\"")
                || content.contains("\"status\":\"completed\""),
            "todo output should be replayed as OpenCode-style JSON, not markdown checkboxes: {content}"
        );
        assert!(!content.contains("- [completed]"), "{content}");
        assert!(!content.contains("Old tool result content cleared"));
    }

    #[test]
    fn native_tool_calls_parse_to_air_decision_shape() {
        let mut tool_name_map = BTreeMap::new();
        tool_name_map.insert("edit".to_string(), "edit".to_string());

        let value = parse_chat_completion_content(
            &json!({
                "choices": [{
                    "message": {
                        "content": "Need an edit.",
                        "tool_calls": [{
                            "id": "call_edit_1",
                            "type": "function",
                            "function": {
                                "name": "edit",
                                "arguments": "{\"filePath\":\"src/lib.rs\",\"oldString\":\"old\",\"newString\":\"new\"}"
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
                "_air_assistant": {
                    "content": "Need an edit."
                },
                "tool_calls": [{
                    "tool": "edit",
                    "_air_tool_name": "edit",
                    "_air_tool_call_id": "call_edit_1",
                    "input": {
                        "filePath": "src/lib.rs",
                        "oldString": "old",
                        "newString": "new"
                    }
                }]
            })
        );
    }

    #[test]
    fn streaming_chat_completion_parses_native_tool_call_chunks() {
        let response = parse_chat_completion_stream(
            r#"data: {"choices":[{"delta":{"reasoning_content":"thinking ","tool_calls":[{"index":0,"id":"call_edit","type":"function","function":{"name":"edit","arguments":"{\"filePath\":\"src/lib.rs\","}}]}}]}

data: {"choices":[{"delta":{"content":"Applying edit.","tool_calls":[{"index":0,"function":{"arguments":"\"oldString\":\"old\",\"newString\":\"new\"}"}}]}}]}

data: [DONE]
"#,
        )
        .unwrap();
        let mut tool_name_map = BTreeMap::new();
        tool_name_map.insert("edit".to_string(), "edit".to_string());
        let value = parse_chat_completion_content(&response, &tool_name_map).unwrap();

        assert_eq!(value["complete"], json!(false));
        assert_eq!(value["_air_assistant"]["content"], json!("Applying edit."));
        assert_eq!(value["_air_assistant"]["reasoning"], json!("thinking"));
        assert_eq!(value["tool_calls"][0]["tool"], json!("edit"));
        assert_eq!(
            value["tool_calls"][0]["input"],
            json!({
                "filePath": "src/lib.rs",
                "oldString": "old",
                "newString": "new"
            })
        );
    }

    #[test]
    fn native_tool_call_response_keeps_long_reasoning_for_history() {
        let long_reasoning = format!(
            "I know the target edit now. {}",
            "preserve the implementation plan. ".repeat(500)
        );
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Now I can write the file.",
                    "reasoning_content": long_reasoning,
                    "tool_calls": [{
                        "id": "call_write",
                        "type": "function",
                        "function": {
                            "name": "write",
                            "arguments": "{\"filePath\":\"src/lib.rs\",\"content\":\"fn main() {}\"}"
                        }
                    }]
                }
            }]
        });
        let mut tool_name_map = BTreeMap::new();
        tool_name_map.insert("write".to_string(), "write".to_string());
        let value = parse_chat_completion_content(&response, &tool_name_map).unwrap();
        let reasoning = value["_air_assistant"]["reasoning"].as_str().unwrap();

        assert!(reasoning.contains("I know the target edit now."));
        assert!(reasoning.len() > 10_000);
    }

    #[test]
    fn native_tool_calls_parse_no_tool_assistant_text_as_complete_decision() {
        let mut tool_name_map = BTreeMap::new();
        tool_name_map.insert("edit".to_string(), "edit".to_string());

        let value = parse_chat_completion_content(
            &json!({
                "choices": [{
                    "message": {
                        "content": "All checks pass; no more tool calls are needed."
                    }
                }]
            }),
            &tool_name_map,
        )
        .unwrap();

        assert_eq!(
            value,
            json!({
                "complete": true,
                "_air_assistant": {
                    "content": "All checks pass; no more tool calls are needed."
                },
                "tool_calls": []
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
    fn chat_completion_url_appends_endpoint_once() {
        assert_eq!(
            chat_completions_url("https://provider.example/v1"),
            "https://provider.example/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://provider.example/v1/"),
            "https://provider.example/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://provider.example/v1/chat/completions"),
            "https://provider.example/v1/chat/completions"
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
            opencode_tool_mode: None,
            trace_provider_io: None,
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
            opencode_tool_mode: None,
            trace_provider_io: None,
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
            opencode_tool_mode: None,
            trace_provider_io: None,
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
            opencode_tool_mode: None,
            trace_provider_io: None,
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
