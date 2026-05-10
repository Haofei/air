use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::tools::ToolProviderChoice;
use air_runtime::{ToolProvider, TraceEvent, TraceStatus};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub(crate) fn run_acceptance_checks(
    tools: &mut Option<ToolProviderChoice>,
    plan_profile: &PathBuf,
    tool_config: Option<PathBuf>,
    checks: Option<&Value>,
    scope: &str,
    events: &mut Vec<TraceEvent>,
    step: &mut u32,
) -> Result<Vec<Value>> {
    let checks = acceptance_checks(checks);
    if checks.is_empty() {
        return Ok(Vec::new());
    }
    let tool_config = resolve_code_project_tool_config(plan_profile, tool_config)?;
    let tools = tools.get_or_insert(ToolProviderChoice::from_config(tool_config, false)?);
    let mut results = Vec::new();
    for check in checks {
        *step += 1;
        let input = check.input.clone();
        let output = tools.call_tool("test.run", &input);
        match output {
            Ok(output) => {
                let success = output
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                events.push(TraceEvent {
                    agent: "air-code-project".to_string(),
                    step: *step,
                    rule: scope.to_string(),
                    action: "tool_call".to_string(),
                    input: Some(input),
                    output: Some(output.clone()),
                    meta: Some(json!({
                        "tool": "test.run",
                        "acceptance_id": check.id,
                        "scope": scope,
                    })),
                    status: if success {
                        TraceStatus::Ok
                    } else {
                        TraceStatus::Error
                    },
                    error: if success {
                        None
                    } else {
                        Some(format!("acceptance check {} failed", check.id))
                    },
                });
                results.push(json!({
                    "id": check.id,
                    "scope": scope,
                    "command": check.command,
                    "expected": check.expected,
                    "success": success,
                    "output": output,
                }));
            }
            Err(error) => {
                let error = error.to_string();
                events.push(TraceEvent {
                    agent: "air-code-project".to_string(),
                    step: *step,
                    rule: scope.to_string(),
                    action: "tool_call".to_string(),
                    input: Some(input),
                    output: None,
                    meta: Some(json!({
                        "tool": "test.run",
                        "acceptance_id": check.id,
                        "scope": scope,
                    })),
                    status: TraceStatus::Error,
                    error: Some(error.clone()),
                });
                results.push(json!({
                    "id": check.id,
                    "scope": scope,
                    "command": check.command,
                    "expected": check.expected,
                    "success": false,
                    "error": error,
                }));
            }
        }
    }
    Ok(results)
}

#[derive(Clone, Debug)]
pub(crate) struct AcceptanceCheck {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) expected: String,
    pub(crate) input: Value,
}

pub(crate) fn acceptance_checks(value: Option<&Value>) -> Vec<AcceptanceCheck> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Object(object) => {
                let command = object.get("command")?.as_str()?.to_string();
                let id = object
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(&command)
                    .to_string();
                let expected = object
                    .get("expected")
                    .and_then(Value::as_str)
                    .unwrap_or("command succeeds")
                    .to_string();
                let mut input = Map::new();
                input.insert("command".to_string(), Value::String(command.clone()));
                if let Some(Value::Object(parameters)) = object.get("parameters") {
                    for (key, value) in parameters {
                        input.insert(key.clone(), value.clone());
                    }
                }
                Some(AcceptanceCheck {
                    id,
                    command,
                    expected,
                    input: Value::Object(input),
                })
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn acceptance_checks_completed(checks: &[Value]) -> bool {
    checks.iter().all(|check| {
        check
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

fn resolve_code_project_tool_config(
    profile: &PathBuf,
    explicit: Option<PathBuf>,
) -> Result<Option<PathBuf>> {
    if explicit.is_some() {
        return Ok(explicit);
    }
    let profile_config = read_run_plan_profile(profile)?;
    Ok(profile_config
        .tool_config
        .as_ref()
        .map(|path| resolve_profile_path(profile, path)))
}
