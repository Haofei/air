use air_runtime::RuntimeError;
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(crate) fn call_code_assert_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    max_assertions: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let assertions = input
        .get("assertions")
        .or_else(|| input.get("checks"))
        .or_else(|| input.get("conditions"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.assertions must be an array"))
        })?;
    if assertions.len() > max_assertions {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.assertions must contain at most {max_assertions} items"
        )));
    }

    let base = super::canonicalize_tool_path(name, "base_dir", base_dir)?;
    let mut results = Vec::new();
    let mut passed_count = 0usize;
    for (index, assertion) in assertions.iter().enumerate() {
        let result = evaluate_assertion(name, assertion, &base, index)?;
        if result.passed {
            passed_count += 1;
        }
        results.push(result.into_value());
    }
    let failed_count = results.len().saturating_sub(passed_count);
    let passed = failed_count == 0;
    let rendered = serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
    let (content, truncated, bytes) = super::bytes_to_limited_text(rendered.as_bytes(), max_bytes);

    Ok(json!({
        "passed": passed,
        "total": results.len(),
        "passed_count": passed_count,
        "failed_count": failed_count,
        "assertions": results,
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": "code-assert:latest",
            "kind": "code_assert",
            "title": if passed { "code assertions passed" } else { "code assertions failed" },
            "uri": base.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "code_assert",
                "tool": name,
                "total": results.len(),
                "passed_count": passed_count,
                "failed_count": failed_count,
                "truncated": truncated,
                "bytes": bytes
            }
        }]
    }))
}

#[derive(Debug)]
struct AssertionResult {
    kind: String,
    passed: bool,
    message: String,
    path: Option<String>,
    name: Option<String>,
    pattern: Option<String>,
    matches: Vec<Value>,
}

impl AssertionResult {
    fn into_value(self) -> Value {
        json!({
            "kind": self.kind,
            "passed": self.passed,
            "message": self.message,
            "path": self.path,
            "name": self.name,
            "pattern": self.pattern,
            "matches": self.matches,
        })
    }
}

fn evaluate_assertion(
    tool_name: &str,
    assertion: &Value,
    base: &Path,
    index: usize,
) -> Result<AssertionResult, RuntimeError> {
    let object = assertion.as_object().ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} input.assertions[{index}] must be an object"
        ))
    })?;
    let kind = required_assertion_string(tool_name, assertion, index, "kind")?;
    match kind {
        "symbol_present" | "symbol_absent" => {
            let path = required_assertion_string(tool_name, assertion, index, "path")?;
            let name = required_assertion_string(tool_name, assertion, index, "name")?;
            let absolute = assertion_path(tool_name, base, path)?;
            let source = read_assertion_file(tool_name, &absolute)?;
            let matches = symbol_matches(path, &source, name);
            let exists = !matches.is_empty();
            let expected_present = kind == "symbol_present";
            let passed = exists == expected_present;
            let message = match (expected_present, exists) {
                (true, true) => format!("symbol {name} is present in {path}"),
                (true, false) => format!("symbol {name} is missing from {path}"),
                (false, false) => format!("symbol {name} is absent from {path}"),
                (false, true) => format!("symbol {name} is still present in {path}"),
            };
            Ok(AssertionResult {
                kind: kind.to_string(),
                passed,
                message,
                path: Some(path.to_string()),
                name: Some(name.to_string()),
                pattern: None,
                matches,
            })
        }
        "file_contains" | "file_not_contains" => {
            let path = required_assertion_string(tool_name, assertion, index, "path")?;
            let pattern = required_assertion_pattern(tool_name, assertion, index)?;
            let mode = optional_assertion_string(assertion, "mode").unwrap_or("fixed");
            if !matches!(mode, "fixed" | "regex") {
                return Err(RuntimeError::Provider(format!(
                    "tool {tool_name} input.assertions[{index}].mode must be fixed or regex"
                )));
            }
            let absolute = assertion_path(tool_name, base, path)?;
            let source = read_assertion_file(tool_name, &absolute)?;
            let matches = file_pattern_matches(tool_name, &source, pattern, mode)?;
            let exists = !matches.is_empty();
            let expected_present = kind == "file_contains";
            let passed = exists == expected_present;
            let message = match (expected_present, exists) {
                (true, true) => format!("{path} contains pattern"),
                (true, false) => format!("{path} does not contain pattern"),
                (false, false) => format!("{path} does not contain forbidden pattern"),
                (false, true) => format!("{path} still contains forbidden pattern"),
            };
            Ok(AssertionResult {
                kind: kind.to_string(),
                passed,
                message,
                path: Some(path.to_string()),
                name: None,
                pattern: Some(pattern.to_string()),
                matches,
            })
        }
        "command_passes" => {
            let command = optional_assertion_string(assertion, "command")
                .or_else(|| optional_assertion_string(assertion, "name"));
            let result = object
                .get("result")
                .or_else(|| object.get("output"))
                .unwrap_or(assertion);
            let success = result
                .get("success")
                .and_then(Value::as_bool)
                .or_else(|| object.get("success").and_then(Value::as_bool))
                .ok_or_else(|| {
                    RuntimeError::Provider(format!(
                        "tool {tool_name} input.assertions[{index}].result.success must be a boolean"
                    ))
                })?;
            let command_matches = command.is_none_or(|expected| {
                result
                    .get("command")
                    .and_then(Value::as_str)
                    .or_else(|| object.get("actual_command").and_then(Value::as_str))
                    .is_none_or(|actual| actual == expected)
            });
            let passed = success && command_matches;
            Ok(AssertionResult {
                kind: kind.to_string(),
                passed,
                message: if passed {
                    "command result passed".to_string()
                } else {
                    "command result did not pass".to_string()
                },
                path: None,
                name: command.map(ToString::to_string),
                pattern: None,
                matches: Vec::new(),
            })
        }
        _ => Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.assertions[{index}].kind must be symbol_present, symbol_absent, file_contains, file_not_contains, or command_passes"
        ))),
    }
}

fn assertion_path(tool_name: &str, base: &Path, path: &str) -> Result<PathBuf, RuntimeError> {
    if path.trim().is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} assertion path must not be empty"
        )));
    }
    let absolute = super::canonicalize_tool_path(tool_name, "path", &base.join(path))?;
    if !absolute.starts_with(base) {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} path {} is outside base_dir {}",
            absolute.display(),
            base.display()
        )));
    }
    Ok(absolute)
}

fn read_assertion_file(tool_name: &str, path: &Path) -> Result<String, RuntimeError> {
    if !path.is_file() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} path {} is not a file",
            path.display()
        )));
    }
    std::fs::read_to_string(path)
        .map_err(|error| RuntimeError::Provider(format!("tool {tool_name} read file: {error}")))
}

fn symbol_matches(path: &str, source: &str, name: &str) -> Vec<Value> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let (kind, symbol_name) = super::parse_symbol_declaration(line)?;
            (symbol_name == name).then(|| {
                json!({
                    "path": path,
                    "line": index + 1,
                    "kind": kind,
                    "name": symbol_name,
                    "text": line.trim(),
                })
            })
        })
        .collect()
}

fn file_pattern_matches(
    tool_name: &str,
    source: &str,
    pattern: &str,
    mode: &str,
) -> Result<Vec<Value>, RuntimeError> {
    if mode == "regex" {
        let regex = Regex::new(pattern).map_err(|error| {
            RuntimeError::Provider(format!("tool {tool_name} invalid regex: {error}"))
        })?;
        return Ok(source
            .lines()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line))
            .map(|(index, line)| {
                json!({
                    "line": index + 1,
                    "text": line.trim(),
                })
            })
            .collect());
    }

    Ok(source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(pattern))
        .map(|(index, line)| {
            json!({
                "line": index + 1,
                "text": line.trim(),
            })
        })
        .collect())
}

fn required_assertion_string<'a>(
    tool_name: &str,
    assertion: &'a Value,
    index: usize,
    field: &str,
) -> Result<&'a str, RuntimeError> {
    assertion.get(field).and_then(Value::as_str).ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} input.assertions[{index}].{field} must be a string"
        ))
    })
}

fn optional_assertion_string<'a>(assertion: &'a Value, field: &str) -> Option<&'a str> {
    assertion.get(field).and_then(Value::as_str)
}

fn required_assertion_pattern<'a>(
    tool_name: &str,
    assertion: &'a Value,
    index: usize,
) -> Result<&'a str, RuntimeError> {
    assertion
        .get("pattern")
        .or_else(|| assertion.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} input.assertions[{index}].pattern must be a string"
            ))
        })
}
