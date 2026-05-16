use serde_json::Value;
use std::path::Path;

pub(super) fn render_search_matches(matches: &[Value], base_path: Option<&Path>) -> String {
    let mut rendered = String::new();
    let mut current_path = None;
    for item in matches {
        render_search_match(item, None, base_path, &mut current_path, &mut rendered);
    }
    rendered
}

fn render_search_match(
    value: &Value,
    fallback_path: Option<&str>,
    base_path: Option<&Path>,
    current_path: &mut Option<String>,
    rendered: &mut String,
) {
    let path = value.get("path").and_then(Value::as_str).or(fallback_path);
    if let Some(before) = value.get("before").and_then(Value::as_array) {
        for context in before {
            render_search_line(context, path, base_path, current_path, rendered);
        }
    }
    render_search_line(value, path, base_path, current_path, rendered);
    if let Some(after) = value.get("after").and_then(Value::as_array) {
        for context in after {
            render_search_line(context, path, base_path, current_path, rendered);
        }
    }
}

fn render_search_line(
    value: &Value,
    fallback_path: Option<&str>,
    base_path: Option<&Path>,
    current_path: &mut Option<String>,
    rendered: &mut String,
) {
    let number = value
        .get("line_number")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let line = value
        .get("line")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = value.get("path").and_then(Value::as_str).or(fallback_path);
    if let Some(path) = path {
        render_path_header(path, base_path, current_path, rendered);
        rendered.push_str(&format_search_line(number, line, true));
    } else {
        rendered.push_str(&format_search_line(number, line, false));
    }
}

fn render_path_header(
    path: &str,
    base_path: Option<&Path>,
    current_path: &mut Option<String>,
    rendered: &mut String,
) {
    let display_path = display_search_path(path, base_path);
    if current_path.as_deref() != Some(display_path.as_str()) {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&display_path);
        rendered.push_str(":\n");
        *current_path = Some(display_path);
    }
}

fn format_search_line(number: u64, line: &str, indented: bool) -> String {
    if indented {
        format!("  Line {number}: {line}\n")
    } else {
        format!("Line {number}: {line}\n")
    }
}

fn display_search_path(path: &str, base_path: Option<&Path>) -> String {
    let path_value = Path::new(path);
    if path_value.is_absolute() {
        path.to_string()
    } else if let Some(base_path) = base_path {
        base_path.join(path_value).display().to_string()
    } else {
        path.to_string()
    }
}

pub(super) fn stable_pattern_id(pattern: &str) -> String {
    pattern
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(48)
        .collect::<String>()
}
