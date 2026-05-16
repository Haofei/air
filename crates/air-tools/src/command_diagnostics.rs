//! Command diagnostics parser helpers.

use serde_json::{json, Value};

fn command_run_diagnostic(
    severity: impl Into<String>,
    path: impl Into<String>,
    line: u64,
    column: u64,
    message: impl Into<String>,
    raw: impl Into<String>,
) -> Value {
    json!({
        "source": "command_run",
        "severity": severity.into(),
        "path": path.into(),
        "line": line,
        "column": column,
        "message": message.into(),
        "raw": raw.into()
    })
}

pub(crate) fn extract_command_diagnostics(log: &str, max_diagnostics: usize) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    let mut pending_rust = None;
    let mut pending_python_frames: Vec<(String, u64, String)> = Vec::new();
    let mut pending_diagnostic_file: Option<String> = None;
    for line in log.lines() {
        let trimmed = line.trim_start();
        if let Some((severity, message)) = parse_rust_severity_message(trimmed) {
            pending_rust = Some((severity, message, trimmed.to_string()));
            continue;
        }
        if let Some(path) = parse_diagnostic_file_context(line) {
            pending_diagnostic_file = Some(path);
            continue;
        }
        if let Some(path) = pending_diagnostic_file.as_deref() {
            if let Some(diagnostic) = parse_indented_line_column_diagnostic(line, path) {
                diagnostics.push(diagnostic);
                pending_rust = None;
                pending_python_frames.clear();
            }
        }
        if let Some((path, line_number)) = parse_python_traceback_location(trimmed) {
            pending_python_frames.push((path, line_number, trimmed.to_string()));
            continue;
        }
        if let Some(message) = parse_python_exception_line(trimmed) {
            if let Some((path, line_number, raw)) = pending_python_frames.last() {
                diagnostics.push(command_run_diagnostic(
                    "error",
                    path,
                    *line_number,
                    1,
                    message,
                    raw,
                ));
                pending_python_frames.clear();
                pending_rust = None;
            }
        }
        if let Some((path, line_number, column)) = parse_rust_location_line(trimmed) {
            if let Some((severity, message, raw)) = pending_rust.take() {
                diagnostics.push(command_run_diagnostic(
                    severity,
                    path,
                    line_number,
                    column,
                    message,
                    raw,
                ));
            }
        } else if let Some(diagnostic) = parse_typescript_diagnostic(trimmed)
            .or_else(|| parse_colon_diagnostic(trimmed))
            .or_else(|| parse_colon_line_diagnostic(trimmed))
        {
            diagnostics.push(diagnostic);
            pending_rust = None;
            pending_python_frames.clear();
            pending_diagnostic_file = None;
        }
        if diagnostics.len() >= max_diagnostics {
            break;
        }
    }
    diagnostics
}

fn parse_rust_severity_message(line: &str) -> Option<(String, String)> {
    for severity in ["error", "warning"] {
        if line == severity
            || line.starts_with(&format!("{severity}:"))
            || line.starts_with(&format!("{severity}["))
        {
            let message = line
                .split_once(':')
                .map(|(_, message)| message.trim())
                .filter(|message| !message.is_empty())
                .unwrap_or(line)
                .to_string();
            return Some((severity.to_string(), message));
        }
    }
    None
}

fn parse_rust_location_line(line: &str) -> Option<(String, u64, u64)> {
    let rest = line.strip_prefix("-->")?.trim();
    parse_location(rest)
}

fn parse_typescript_diagnostic(line: &str) -> Option<Value> {
    let open = line.find('(')?;
    let close = line[open + 1..].find(')')? + open + 1;
    let path = line[..open].trim();
    let mut location = line[open + 1..close].split(',');
    let line_number = location.next()?.trim().parse::<u64>().ok()?;
    let column = location.next()?.trim().parse::<u64>().ok()?;
    let rest = line[close + 1..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let (severity, message) = split_severity_message(rest)?;
    Some(command_run_diagnostic(
        severity,
        path,
        line_number,
        column,
        message,
        line,
    ))
}

fn parse_python_traceback_location(line: &str) -> Option<(String, u64)> {
    let rest = line.strip_prefix("File ")?;
    let rest = rest.strip_prefix('"')?;
    let (path, rest) = rest.split_once('"')?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(", line ")?;
    let line_number = rest
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse::<u64>()
        .ok()?;
    Some((path.to_string(), line_number))
}

fn parse_python_exception_line(line: &str) -> Option<String> {
    if line.is_empty()
        || line == "Traceback (most recent call last):"
        || line.starts_with("File ")
        || line.starts_with("During handling of the above exception")
        || line.starts_with("The above exception was the direct cause")
    {
        return None;
    }
    let (exception, _message) = line.split_once(':').unwrap_or((line, ""));
    let exception = exception.trim();
    let valid_exception_name = exception
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '.')
        && exception
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_uppercase() || character == '_');
    valid_exception_name.then(|| line.to_string())
}

fn parse_colon_diagnostic(line: &str) -> Option<Value> {
    let parts = line.split(':').collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }
    for index in 1..parts.len().saturating_sub(2) {
        let Ok(line_number) = parts[index].trim().parse::<u64>() else {
            continue;
        };
        let Ok(column) = parts[index + 1].trim().parse::<u64>() else {
            continue;
        };
        let rest = parts[index + 2..].join(":");
        let Some((severity, message)) = split_severity_message(rest.trim()) else {
            continue;
        };
        let path = parts[..index].join(":");
        return Some(command_run_diagnostic(
            severity,
            path,
            line_number,
            column,
            message,
            line,
        ));
    }
    None
}

fn parse_colon_line_diagnostic(line: &str) -> Option<Value> {
    let parts = line.split(':').collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    for index in 1..parts.len().saturating_sub(1) {
        let Ok(line_number) = parts[index].trim().parse::<u64>() else {
            continue;
        };
        let rest = parts[index + 1..].join(":");
        let rest = rest.trim();
        if rest.is_empty() {
            continue;
        }
        let path = parts[..index].join(":");
        if path.trim().is_empty() {
            continue;
        }
        let (severity, message) = if let Some((severity, message)) = split_severity_message(rest) {
            (severity, message)
        } else if let Some(message) = parse_python_exception_line(rest) {
            ("error".to_string(), message)
        } else {
            continue;
        };
        return Some(command_run_diagnostic(
            severity,
            path,
            line_number,
            1,
            message,
            line,
        ));
    }
    None
}

fn parse_diagnostic_file_context(line: &str) -> Option<String> {
    let path = line.trim();
    if path.is_empty()
        || path.contains(char::is_whitespace)
        || path.contains(':')
        || !path.contains('.')
        || path.starts_with(">")
    {
        return None;
    }
    Some(path.to_string())
}

fn parse_indented_line_column_diagnostic(line: &str, path: &str) -> Option<Value> {
    if !line.starts_with(char::is_whitespace) {
        return None;
    }
    let trimmed = line.trim_start();
    let (line_text, rest) = trimmed.split_once(':')?;
    let line_number = line_text.trim().parse::<u64>().ok()?;
    let (column_text, rest) = rest.trim_start().split_once(char::is_whitespace)?;
    let column = column_text.trim().parse::<u64>().ok()?;
    let rest = rest.trim_start();
    let (severity, message) = split_severity_message(rest)?;
    Some(command_run_diagnostic(
        severity,
        path,
        line_number,
        column,
        message,
        line.trim(),
    ))
}

fn split_severity_message(text: &str) -> Option<(String, String)> {
    for severity in ["error", "warning"] {
        if text == severity {
            return Some((severity.to_string(), String::new()));
        }
        if let Some(message) = text.strip_prefix(&format!("{severity}:")) {
            return Some((severity.to_string(), message.trim().to_string()));
        }
        if text.starts_with(&format!("{severity} ")) {
            let message = text[severity.len()..].trim_start().to_string();
            return Some((severity.to_string(), message));
        }
    }
    None
}

fn parse_location(text: &str) -> Option<(String, u64, u64)> {
    let parts = text.split(':').collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let column = parts.last()?.trim().parse::<u64>().ok()?;
    let line_number = parts.get(parts.len() - 2)?.trim().parse::<u64>().ok()?;
    let path = parts[..parts.len() - 2].join(":");
    Some((path, line_number, column))
}
