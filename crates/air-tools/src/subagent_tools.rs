use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use air_runtime::RuntimeError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::canonicalize_tool_path;
use super::command_config::{CommandRunOptions, TruncationDirection};
use super::command_run::run_command_argv;

const SUBAGENT_OUTPUT_MAX_CHARS: usize = 12_000;
const SUBAGENT_OUTPUT_CONTRACT: &str = r#"Subagent output contract:
Return a concise handoff for the parent agent in this shape:
Findings:
- path:line-line `symbol_or_anchor` - what matters
Evidence:
- smallest useful quoted anchor or fact
Next action:
- the exact next read/edit/test action you recommend
Parent read again:
- yes/no, with the specific path and line range if yes"#;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct SubagentProfileConfig {
    pub(super) cwd: PathBuf,
    pub(super) command: Vec<String>,

    #[serde(default)]
    pub(super) timeout_seconds: Option<u64>,

    #[serde(default)]
    pub(super) max_bytes: Option<usize>,

    #[serde(default)]
    pub(super) truncation_direction: Option<TruncationDirection>,

    #[serde(default)]
    pub(super) output_contract: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SubagentToolOptions {
    pub(super) timeout_seconds: u64,
    pub(super) max_bytes: usize,
    pub(super) truncation_direction: TruncationDirection,
}

pub(super) fn call_subagent_tool(
    name: &str,
    input: &Value,
    profiles: &BTreeMap<String, SubagentProfileConfig>,
    workspace_dir: &Path,
) -> Result<Value, RuntimeError> {
    let prompt = required_string(name, input, "prompt")?;
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("subagent task");
    let subagent_type = input
        .get("subagent_type")
        .and_then(Value::as_str)
        .unwrap_or("explore");
    let profile = profiles.get(subagent_type).ok_or_else(|| {
        let available = profiles.keys().cloned().collect::<Vec<_>>().join(", ");
        RuntimeError::Provider(format!(
            "tool {name} unknown subagent_type {subagent_type:?}; available subagents: {available}"
        ))
    })?;
    if profile.command.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} subagent {subagent_type} command must not be empty"
        )));
    }
    let cwd = canonicalize_tool_path(name, "cwd", &workspace_dir.join(&profile.cwd))?;
    let options = profile_options(profile);
    let task_prompt = subagent_task_prompt(prompt, profile.output_contract.as_deref());
    let input_file = write_subagent_input(name, &cwd, &task_prompt, description, subagent_type)?;
    let argv = profile
        .command
        .iter()
        .map(|part| render_subagent_arg(name, part, input, &input_file))
        .collect::<Result<Vec<_>, _>>()?;
    let mut output = run_command_argv(
        name,
        subagent_type,
        &cwd,
        &argv,
        CommandRunOptions {
            timeout_seconds: options.timeout_seconds,
            max_bytes: options.max_bytes,
            truncation_direction: options.truncation_direction,
        },
    )?;
    if let Some(object) = output.as_object_mut() {
        normalize_subagent_command_output(object);
        object.insert("title".to_string(), Value::String(description.to_string()));
        object.insert(
            "subagent_type".to_string(),
            Value::String(subagent_type.to_string()),
        );
        object.insert(
            "input_file".to_string(),
            Value::String(input_file.display().to_string()),
        );
        if let Some(log) = object.get("log").and_then(Value::as_str) {
            object.insert("output".to_string(), Value::String(log.to_string()));
        }
    }
    Ok(output)
}

pub(super) fn validate_subagent_profiles(
    tool_name: &str,
    profiles: &BTreeMap<String, SubagentProfileConfig>,
    path: &Path,
) -> anyhow::Result<()> {
    if profiles.is_empty() {
        anyhow::bail!(
            "tool config {} tools.{tool_name}.subagents must contain at least one subagent",
            path.display()
        );
    }
    for (profile_name, profile) in profiles {
        if profile_name.trim().is_empty() {
            anyhow::bail!(
                "tool config {} tools.{tool_name}.subagents contains an empty subagent name",
                path.display()
            );
        }
        if profile.cwd.as_os_str().is_empty() {
            anyhow::bail!(
                "tool config {} tools.{tool_name}.subagents.{profile_name}.cwd must not be empty",
                path.display()
            );
        }
        if profile.command.is_empty() || profile.command.iter().any(|part| part.trim().is_empty()) {
            anyhow::bail!(
                "tool config {} tools.{tool_name}.subagents.{profile_name}.command must contain non-empty command parts",
                path.display()
            );
        }
        if profile.timeout_seconds.is_some_and(|value| value == 0) {
            anyhow::bail!(
                "tool config {} tools.{tool_name}.subagents.{profile_name}.timeout_seconds must be greater than 0",
                path.display()
            );
        }
        if profile.max_bytes.is_some_and(|value| value == 0) {
            anyhow::bail!(
                "tool config {} tools.{tool_name}.subagents.{profile_name}.max_bytes must be greater than 0",
                path.display()
            );
        }
        if profile
            .output_contract
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            anyhow::bail!(
                "tool config {} tools.{tool_name}.subagents.{profile_name}.output_contract must not be empty when provided",
                path.display()
            );
        }
    }
    Ok(())
}

fn profile_options(profile: &SubagentProfileConfig) -> SubagentToolOptions {
    SubagentToolOptions {
        timeout_seconds: profile.timeout_seconds.unwrap_or(600),
        max_bytes: profile.max_bytes.unwrap_or(256 * 1024),
        truncation_direction: profile
            .truncation_direction
            .unwrap_or(TruncationDirection::Tail),
    }
}

fn subagent_task_prompt(prompt: &str, output_contract: Option<&str>) -> String {
    format!(
        "{}\n\n{}",
        prompt.trim(),
        output_contract.unwrap_or(SUBAGENT_OUTPUT_CONTRACT)
    )
}

fn required_string<'a>(name: &str, input: &'a Value, field: &str) -> Result<&'a str, RuntimeError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.{field} must be a string"))
        })
}

fn subagent_input_payload(prompt: &str, description: &str, subagent_type: &str) -> Value {
    json!({
        "task": prompt,
        "description": description,
        "subagent_type": subagent_type
    })
}

fn write_subagent_input(
    name: &str,
    cwd: &Path,
    prompt: &str,
    description: &str,
    subagent_type: &str,
) -> Result<std::path::PathBuf, RuntimeError> {
    let dir = cwd.join(".air").join("subagents");
    fs::create_dir_all(&dir).map_err(|error| {
        RuntimeError::Provider(format!("tool {name} create subagent input dir: {error}"))
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} timestamp: {error}")))?
        .as_millis();
    let path = dir.join(format!("task-{timestamp}.input.json"));
    let payload = subagent_input_payload(prompt, description, subagent_type);
    fs::write(
        &path,
        serde_json::to_vec_pretty(&payload).map_err(|error| {
            RuntimeError::Provider(format!("tool {name} serialize subagent input: {error}"))
        })?,
    )
    .map_err(|error| {
        RuntimeError::Provider(format!("tool {name} write subagent input: {error}"))
    })?;
    Ok(path)
}

fn subagent_raw_log(object: &serde_json::Map<String, Value>) -> &str {
    object
        .get("log")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn normalize_subagent_command_output(object: &mut serde_json::Map<String, Value>) {
    let raw_log = subagent_raw_log(object);
    let concise_output = extract_subagent_output(raw_log);
    if let Some(bytes) = object.get("bytes").cloned() {
        object.insert("raw_output_bytes".to_string(), bytes);
    }
    if let Some(handoff) = parse_subagent_handoff(&concise_output) {
        object.insert("handoff".to_string(), handoff);
    }
    object.insert("output".to_string(), Value::String(concise_output));
    object.remove("log");
    object.remove("artifacts");
}

fn extract_subagent_output(raw_log: &str) -> String {
    let raw_log = raw_log.trim();
    if let Ok(value) = serde_json::from_str::<Value>(raw_log) {
        for pointer in [
            "/result/answer",
            "/result/decision/_air_assistant/content",
            "/result/edit/rationale",
            "/answer",
            "/content",
            "/response/content",
        ] {
            if let Some(text) = value.pointer(pointer).and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    return truncate_chars(text, SUBAGENT_OUTPUT_MAX_CHARS);
                }
            }
        }
        if let Some(result) = value.get("result") {
            return truncate_chars(
                &serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string()),
                SUBAGENT_OUTPUT_MAX_CHARS,
            );
        }
    }
    truncate_chars(raw_log, SUBAGENT_OUTPUT_MAX_CHARS)
}

fn parse_subagent_handoff(output: &str) -> Option<Value> {
    let mut findings = Vec::new();
    let mut evidence = Vec::new();
    let mut next_action = Vec::new();
    let mut parent_read_again = Vec::new();
    let mut current = "";

    for line in output.lines() {
        let trimmed = line.trim();
        match trimmed {
            "Findings:" => {
                current = "findings";
                continue;
            }
            "Evidence:" => {
                current = "evidence";
                continue;
            }
            "Next action:" => {
                current = "next_action";
                continue;
            }
            "Parent read again:" => {
                current = "parent_read_again";
                continue;
            }
            _ => {}
        }
        if trimmed.is_empty() {
            continue;
        }
        let value = trimmed.trim_start_matches("- ").trim().to_string();
        match current {
            "findings" => findings.push(value),
            "evidence" => evidence.push(value),
            "next_action" => next_action.push(value),
            "parent_read_again" => parent_read_again.push(value),
            _ => {}
        }
    }

    if findings.is_empty()
        && evidence.is_empty()
        && next_action.is_empty()
        && parent_read_again.is_empty()
    {
        return None;
    }

    Some(json!({
        "findings": findings,
        "evidence": evidence,
        "next_action": next_action,
        "parent_read_again": parent_read_again
    }))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let marker = "\n[AIR_SUBAGENT_OUTPUT_TRUNCATED]";
    let keep_chars = max_chars.saturating_sub(marker.chars().count());
    format!(
        "{}{}",
        value.chars().take(keep_chars).collect::<String>(),
        marker
    )
}

fn render_subagent_arg(
    name: &str,
    template: &str,
    input: &Value,
    input_file: &Path,
) -> Result<String, RuntimeError> {
    let mut rendered = template
        .replace("{air_exe}", &current_exe(name)?)
        .replace("{input_file}", &input_file.display().to_string());
    for field in [
        "description",
        "prompt",
        "subagent_type",
        "session_id",
        "command",
    ] {
        let value = input.get(field).and_then(Value::as_str).unwrap_or_default();
        rendered = rendered.replace(&format!("{{{field}}}"), value);
    }
    render_env_placeholders(name, &rendered)
}

fn current_exe(name: &str) -> Result<String, RuntimeError> {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .map_err(|error| {
            RuntimeError::Provider(format!("tool {name} resolve current exe: {error}"))
        })
}

fn render_env_placeholders(name: &str, template: &str) -> Result<String, RuntimeError> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{env:") {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 5..];
        let Some(end) = after_start.find('}') else {
            output.push_str(&rest[start..]);
            return Ok(output);
        };
        let env_name = &after_start[..end];
        if env_name.trim().is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} contains an empty env placeholder"
            )));
        }
        let value = std::env::var(env_name).map_err(|_| {
            RuntimeError::Provider(format!(
                "tool {name} environment variable {env_name} is not set"
            ))
        })?;
        output.push_str(&value);
        rest = &after_start[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}
