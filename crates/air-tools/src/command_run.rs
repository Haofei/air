use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use air_runtime::RuntimeError;
use serde_json::{json, Value};

use super::bash_classify::{
    forbidden_git_workspace_command, is_verification_bash_command,
    normalize_bash_verification_result,
};
use super::command_config::{CommandParameterAllow, CommandParameterRule, CommandRunOptions};
use super::command_diagnostics::extract_command_diagnostics;
use super::{canonicalize_tool_path, validate_relative_path_filter};
use air_tools_core::text::bytes_to_limited_text_with_direction;
use air_tools_core::workspace_snapshot::{workspace_snapshot_ignore_set, CommandWorkspaceSnapshot};

pub(super) fn call_command_run_tool(
    name: &str,
    input: &Value,
    cwd: &Path,
    commands: &BTreeMap<String, Vec<String>>,
    parameters: &BTreeMap<String, CommandParameterRule>,
    options: CommandRunOptions,
) -> Result<Value, RuntimeError> {
    let command_name = match input.get("command").and_then(Value::as_str) {
        Some(command) => command,
        None if commands.len() == 1 => commands
            .keys()
            .next()
            .map(String::as_str)
            .expect("single command map has one key"),
        None => {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.command must be a string when multiple commands are configured"
            )));
        }
    };
    let Some(command) = commands.get(command_name) else {
        return Err(RuntimeError::Provider(format!(
            "tool {name} command {command_name} is not configured"
        )));
    };
    let cwd = canonicalize_tool_path(name, "cwd", cwd)?;
    let (program, args) = command.split_first().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} command {command_name} is empty"))
    })?;
    let rendered_args = render_command_args(name, input, args, parameters)?;
    let mut argv = Vec::with_capacity(command.len());
    argv.push(program.clone());
    argv.extend(rendered_args.iter().cloned());
    run_command_argv_with_workspace_change(name, command_name, &cwd, &cwd, &argv, options)
}

pub(super) fn call_bash_tool(
    name: &str,
    input: &Value,
    cwd: &Path,
    mut options: CommandRunOptions,
) -> Result<Value, RuntimeError> {
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.command must be a string"))
        })?;
    if command.trim().is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.command must not be empty"
        )));
    }
    if let Some(git_subcommand) = forbidden_git_workspace_command(command) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} refuses workspace-mutating git command `git {git_subcommand}`; use non-mutating git commands such as git diff or git status for inspection"
        )));
    }
    if let Some(timeout_ms) = input.get("timeout").and_then(Value::as_u64) {
        if timeout_ms == 0 {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.timeout must be greater than 0"
            )));
        }
        options.timeout_seconds = timeout_ms.saturating_add(999) / 1000;
    }
    let base_cwd = canonicalize_tool_path(name, "cwd", cwd)?;
    let requested_cwd = input
        .get("workdir")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let cwd = if let Some(workdir) = requested_cwd {
        let candidate = if Path::new(workdir).is_absolute() {
            std::path::PathBuf::from(workdir)
        } else {
            base_cwd.join(workdir)
        };
        let workdir = canonicalize_tool_path(name, "input.workdir", &candidate)?;
        if !workdir.starts_with(&base_cwd) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.workdir is outside configured cwd"
            )));
        }
        workdir
    } else {
        base_cwd.clone()
    };
    let argv = vec![
        "bash".to_string(),
        "-lc".to_string(),
        format!("set -o pipefail; {command}"),
    ];
    let mut output =
        run_command_argv_with_workspace_change(name, "bash", &cwd, &base_cwd, &argv, options)?;
    if let Some(object) = output.as_object_mut() {
        let description = input.get("description").and_then(Value::as_str);
        let verification = is_verification_bash_command(command, description);
        object.insert(
            "input_command".to_string(),
            Value::String(command.to_string()),
        );
        object.insert("verification".to_string(), Value::Bool(verification));
        if verification {
            normalize_bash_verification_result(object);
        }
        if let Some(description) = description {
            object.insert(
                "description".to_string(),
                Value::String(description.to_string()),
            );
        }
    }
    Ok(output)
}

fn run_command_argv_with_workspace_change(
    name: &str,
    command_name: &str,
    cwd: &Path,
    snapshot_root: &Path,
    argv: &[String],
    options: CommandRunOptions,
) -> Result<Value, RuntimeError> {
    let ignore = workspace_snapshot_ignore_set(&options.workspace_snapshot_ignore)?;
    let snapshot_root = canonicalize_tool_path(name, "snapshot_root", snapshot_root)?;
    let before = CommandWorkspaceSnapshot::capture(&snapshot_root, &ignore);
    let mut output = run_command_argv(name, command_name, cwd, argv, options)?;
    let after = CommandWorkspaceSnapshot::capture(&snapshot_root, &ignore);
    let changed_files = before.changed_files(&after);
    if let Some(object) = output.as_object_mut() {
        object.insert(
            "workspace_changed".to_string(),
            Value::Bool(!changed_files.is_empty()),
        );
        object.insert(
            "workspace_changed_files".to_string(),
            Value::Array(changed_files.into_iter().map(Value::String).collect()),
        );
    }
    Ok(output)
}

pub(super) fn run_command_argv(
    name: &str,
    command_name: &str,
    cwd: &Path,
    argv: &[String],
    options: CommandRunOptions,
) -> Result<Value, RuntimeError> {
    let cwd = canonicalize_tool_path(name, "cwd", cwd)?;
    let (program, args) = argv.split_first().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} command {command_name} is empty"))
    })?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(&cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} command run: {error}")))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} command stdout pipe was unavailable"))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} command stderr pipe was unavailable"))
    })?;
    let stdout_reader = read_command_stream(stdout);
    let stderr_reader = read_command_stream(stderr);
    let timeout = Duration::from_secs(options.timeout_seconds);
    let started_at = std::time::Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|error| RuntimeError::Provider(format!("tool {name} command wait: {error}")))?
        {
            Some(status) => {
                let stdout = join_command_stream(name, command_name, "stdout", stdout_reader)?;
                let stderr = join_command_stream(name, command_name, "stderr", stderr_reader)?;
                let mut combined = Vec::new();
                combined.extend_from_slice(&stdout);
                if !stderr.is_empty() {
                    combined.extend_from_slice(b"\n[stderr]\n");
                    combined.extend_from_slice(&stderr);
                }
                let combined_text = String::from_utf8_lossy(&combined).to_string();
                let diagnostics = extract_command_diagnostics(&combined_text, 50);
                let diagnostics_count = diagnostics.len();
                let (log, truncated, bytes) = bytes_to_limited_text_with_direction(
                    &combined,
                    options.max_bytes,
                    options.truncation_direction,
                );
                let full_log_path = if truncated {
                    Some(save_command_full_log(name, command_name, &cwd, &combined)?)
                } else {
                    None
                };
                let truncation_hint = full_log_path.as_ref().map(|path| {
                    format!(
                        "The command output was truncated. Full command output saved to: {path}. Use file.search or bounded span inspection to inspect specific sections."
                    )
                });
                return Ok(json!({
                    "command": command_name,
                    "argv": argv,
                    "cwd": cwd.display().to_string(),
                    "status": status.code(),
                    "success": status.success(),
                    "log": log.clone(),
                    "diagnostics": diagnostics,
                    "bytes": bytes,
                    "truncated": truncated,
                    "full_log_path": full_log_path.clone(),
                    "truncation_hint": truncation_hint.clone(),
                    "artifacts": [{
                        "id": format!("command-run:{}:{}", cwd.display(), command_name),
                        "kind": "test_log",
                        "title": format!("command run: {command_name}"),
                        "uri": cwd.display().to_string(),
                        "content": log,
                        "metadata": {
                            "provider": "command_run",
                            "command": command_name,
                            "argv": argv,
                            "cwd": cwd.display().to_string(),
                            "status": status.code(),
                            "success": status.success(),
                            "diagnostics_count": diagnostics_count,
                            "bytes": bytes,
                            "truncated": truncated,
                            "full_log_path": full_log_path,
                            "truncation_hint": truncation_hint
                        }
                    }]
                }));
            }
            None if started_at.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_command_stream(name, command_name, "stdout", stdout_reader);
                let _ = join_command_stream(name, command_name, "stderr", stderr_reader);
                return Err(RuntimeError::Provider(format!(
                    "tool {name} command {command_name} exceeded timeout_seconds={}",
                    options.timeout_seconds
                )));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn read_command_stream<R: Read + Send + 'static>(
    mut stream: R,
) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_command_stream(
    name: &str,
    command_name: &str,
    stream_name: &str,
    reader: JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, RuntimeError> {
    reader
        .join()
        .map_err(|_| {
            RuntimeError::Provider(format!(
                "tool {name} command {command_name} {stream_name} reader panicked"
            ))
        })?
        .map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {name} command {command_name} read {stream_name}: {error}"
            ))
        })
}

fn save_command_full_log(
    name: &str,
    command_name: &str,
    cwd: &Path,
    combined: &[u8],
) -> Result<String, RuntimeError> {
    let output_dir = cwd.join(".air").join("tool-output");
    fs::create_dir_all(&output_dir).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool {name} command {command_name} create output dir: {error}"
        ))
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {name} command {command_name} timestamp: {error}"
            ))
        })?
        .as_millis();
    let filename = format!(
        "{}-{timestamp}.log",
        sanitize_command_output_filename(command_name)
    );
    let path = output_dir.join(filename);
    fs::write(&path, combined).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool {name} command {command_name} write full log: {error}"
        ))
    })?;
    Ok(path.display().to_string())
}

fn sanitize_command_output_filename(command_name: &str) -> String {
    let sanitized = command_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "command".to_string()
    } else {
        trimmed.to_string()
    }
}

fn render_command_args(
    tool_name: &str,
    input: &Value,
    args: &[String],
    parameters: &BTreeMap<String, CommandParameterRule>,
) -> Result<Vec<String>, RuntimeError> {
    args.iter()
        .map(|arg| render_command_arg(tool_name, input, arg, parameters))
        .collect()
}

fn render_command_arg(
    tool_name: &str,
    input: &Value,
    arg: &str,
    parameters: &BTreeMap<String, CommandParameterRule>,
) -> Result<String, RuntimeError> {
    if !arg.contains("{{") {
        return Ok(arg.to_string());
    }

    let mut rendered = String::new();
    let mut rest = arg;
    while let Some(start) = rest.find("{{") {
        let (before, after_start) = rest.split_at(start);
        rendered.push_str(before);
        let after_start = &after_start[2..];
        let Some(end) = after_start.find("}}") else {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} command template contains an unterminated parameter"
            )));
        };
        let parameter = after_start[..end].trim();
        if parameter.is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} command template contains an empty parameter"
            )));
        }
        let rule = parameters.get(parameter).ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} command template parameter {parameter} is not configured"
            ))
        })?;
        let value = required_command_parameter_input_string(tool_name, input, parameter)?;
        validate_command_parameter_value(tool_name, parameter, &value, rule)?;
        rendered.push_str(&value);
        rest = &after_start[end + 2..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

pub(super) fn command_template_parameters(template: &str) -> Vec<String> {
    let mut parameters = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            break;
        };
        let parameter = after_start[..end].trim();
        if !parameter.is_empty() {
            parameters.push(parameter.to_string());
        }
        rest = &after_start[end + 2..];
    }
    parameters
}

fn required_command_parameter_input_string<'a>(
    tool_name: &str,
    input: &'a Value,
    field: &str,
) -> Result<Cow<'a, str>, RuntimeError> {
    if let Some(value) = input.get(field) {
        return command_parameter_value_to_string(tool_name, value, &format!("input.{field}"));
    }
    for alias in ["args", "arguments"] {
        if let Some(value) = input.get(alias).and_then(|args| args.get(field)) {
            return command_parameter_value_to_string(
                tool_name,
                value,
                &format!("input.{alias}.{field}"),
            );
        }
    }
    Err(RuntimeError::Provider(format!(
        "tool {tool_name} input.{field} must be a string"
    )))
}

fn command_parameter_value_to_string<'a>(
    tool_name: &str,
    value: &'a Value,
    label: &str,
) -> Result<Cow<'a, str>, RuntimeError> {
    match value {
        Value::String(value) => Ok(Cow::Borrowed(value)),
        Value::Number(value) => Ok(Cow::Owned(value.to_string())),
        Value::Bool(value) => Ok(Cow::Owned(value.to_string())),
        _ => Err(RuntimeError::Provider(format!(
            "tool {tool_name} {label} must be a string, number, or boolean"
        ))),
    }
}

pub(super) fn validate_command_parameter_value(
    tool_name: &str,
    parameter: &str,
    value: &str,
    rule: &CommandParameterRule,
) -> Result<(), RuntimeError> {
    if value.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{parameter} must not be empty"
        )));
    }
    if let Some(max_chars) = rule.max_chars {
        let chars = value.chars().count();
        if chars > max_chars {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} input.{parameter} must contain at most {max_chars} characters"
            )));
        }
    }
    if let Some(values) = &rule.values {
        if !values.iter().any(|allowed| allowed == value) {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} input.{parameter} is not an allowed value"
            )));
        }
    }
    let valid = match rule.allow {
        CommandParameterAllow::Identifier => value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':')),
        CommandParameterAllow::Path => {
            validate_relative_path_filter(tool_name, value).is_ok()
                && value.chars().all(|ch| {
                    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':')
                })
        }
        CommandParameterAllow::Text => value
            .chars()
            .all(|ch| !ch.is_control() || matches!(ch, '\t')),
        CommandParameterAllow::SafeArg => value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '_' | '-' | '.' | '/' | ':' | '+' | '=' | '@')
        }),
    };
    if !valid {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{parameter} contains characters not allowed by command parameter policy"
        )));
    }
    Ok(())
}
