use crate::RuntimeError;
use serde_json::{json, Map, Value};

const OPENCODE_READ_CONTEXT_CHARS: usize = 50 * 1024;

pub fn take_last_within_bytes_value(
    value: &Value,
    max_items: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    if max_items == 0 {
        return Err(RuntimeError::Provider(
            "take_last_within_bytes max_items must be at least 1".to_string(),
        ));
    }
    if max_bytes == 0 {
        return Err(RuntimeError::Provider(
            "take_last_within_bytes max_bytes must be at least 1".to_string(),
        ));
    }
    let Some(values) = value.as_array() else {
        return Err(RuntimeError::Provider(
            "take_last_within_bytes expression expected an array value".to_string(),
        ));
    };
    let mut selected = Vec::new();
    let mut selected_bytes = 0usize;
    for value in values.iter().rev().take(max_items) {
        let mut candidate = compact_model_context_payload(value, max_bytes);
        let mut candidate_bytes = json_value_size_bytes(&candidate);
        if selected_bytes.saturating_add(candidate_bytes) > max_bytes {
            candidate = compact_json_value(&candidate, max_bytes);
            candidate_bytes = json_value_size_bytes(&candidate);
        }
        if selected_bytes.saturating_add(candidate_bytes) > max_bytes {
            continue;
        }
        selected_bytes = selected_bytes.saturating_add(candidate_bytes);
        selected.push(candidate);
    }
    selected.reverse();
    Ok(Value::Array(selected))
}

fn compact_model_context_payload(value: &Value, max_bytes: usize) -> Value {
    let compact = compact_model_context_value(value);
    if json_value_size_bytes(&compact) <= max_bytes {
        return compact;
    }
    compact_json_value(&compact, max_bytes)
}

pub fn compact_model_context_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(16)
                .map(compact_model_context_value)
                .collect(),
        ),
        Value::Object(object) if looks_like_observation(object) => {
            compact_observation_context(object)
        }
        Value::Object(object) if looks_like_tool_result(object) => {
            compact_tool_result_context(object)
        }
        Value::Object(object) if object.get("files").and_then(Value::as_array).is_some() => {
            compact_file_collection_context(object)
        }
        Value::Object(object)
            if object.get("content").is_some() || object.get("content_preview").is_some() =>
        {
            compact_file_like_context(object, OPENCODE_READ_CONTEXT_CHARS)
        }
        Value::Object(object) => {
            let mut compact = Map::new();
            for (key, value) in object.iter().take(16) {
                compact.insert(key.clone(), compact_model_context_value(value));
            }
            if object.len() > compact.len() {
                compact.insert(
                    "_air_compacted".to_string(),
                    json!({"omitted_fields": object.len() - compact.len()}),
                );
            }
            Value::Object(compact)
        }
        Value::String(text) => Value::String(compact_model_context_string(text, 2_000)),
        other => other.clone(),
    }
}

fn looks_like_observation(object: &Map<String, Value>) -> bool {
    object.contains_key("action")
        && (object.contains_key("requested") || object.contains_key("result"))
}

fn looks_like_tool_result(object: &Map<String, Value>) -> bool {
    object.contains_key("tool") && (object.contains_key("output") || object.contains_key("error"))
}

fn compact_observation_context(object: &Map<String, Value>) -> Value {
    let mut compact = Map::new();
    for field in ["action", "rationale"] {
        copy_context_field(&mut compact, object, field);
    }
    if let Some(assistant) = object.get("assistant") {
        compact.insert(
            "assistant".to_string(),
            compact_model_context_value(assistant),
        );
    }
    if let Some(requested) = object.get("requested") {
        compact.insert(
            "requested".to_string(),
            compact_model_context_value(requested),
        );
    }
    if let Some(result) = object.get("result") {
        compact.insert("result".to_string(), compact_model_context_value(result));
    }
    Value::Object(compact)
}

fn compact_tool_result_context(object: &Map<String, Value>) -> Value {
    let mut compact = Map::new();
    for field in [
        "tool",
        "status",
        "error_code",
        "permission",
        "message",
        "_air_tool_call_id",
        "_air_tool_name",
    ] {
        copy_context_field(&mut compact, object, field);
    }
    if let Some(input) = object.get("input") {
        compact.insert("input".to_string(), compact_json_value(input, 1_500));
    }
    if let Some(output) = object.get("output").and_then(Value::as_object) {
        compact.insert("output".to_string(), compact_tool_output_context(output));
    }
    if let Some(error) = object.get("error") {
        compact.insert("error".to_string(), compact_model_context_value(error));
    }
    Value::Object(compact)
}

fn compact_tool_output_context(object: &Map<String, Value>) -> Value {
    if looks_like_edit_output(object) {
        return compact_file_edit_output_context(object);
    }
    if object.get("files").and_then(Value::as_array).is_some() {
        return compact_file_collection_context(object);
    }
    if object.get("content").is_some() || object.get("content_preview").is_some() {
        return compact_file_like_context(object, OPENCODE_READ_CONTEXT_CHARS);
    }

    let mut compact = Map::new();
    for field in [
        "success",
        "status",
        "error",
        "error_code",
        "permission",
        "message",
        "applied",
        "passed",
        "failed_count",
        "replacements",
        "edit_count",
        "match_strategy",
        "match_strategies",
        "bytes",
        "path",
        "repo",
        "query",
        "effective_query",
        "mode",
        "names",
        "match_count",
        "returned_match_count",
        "no_matches",
        "no_match_streak",
        "search_hint",
        "start_line",
        "end_line",
        "truncated",
        "full_log_path",
        "full_output_path",
        "truncation_hint",
        "content_format",
        "unscoped_read",
        "changed_files",
        "diagnostics",
        "post_edit_snippets",
        "todos",
        "matches",
        "symbols",
        "references",
        "locations",
    ] {
        if let Some(value) = object.get(field) {
            compact.insert(field.to_string(), compact_model_context_value(value));
        }
    }
    if let Some(artifacts) = object.get("artifacts") {
        compact.insert(
            "artifact_refs".to_string(),
            compact_artifact_refs(artifacts),
        );
    }
    if compact.is_empty() {
        compact_model_context_value(&Value::Object(object.clone()))
    } else {
        Value::Object(compact)
    }
}

fn looks_like_edit_output(object: &Map<String, Value>) -> bool {
    object.contains_key("applied")
        || object.contains_key("replacements")
        || object.contains_key("edit_count")
        || object.contains_key("post_edit_snippets")
}

fn compact_file_edit_output_context(object: &Map<String, Value>) -> Value {
    let mut compact = Map::new();
    for field in [
        "path",
        "applied",
        "checked",
        "replacements",
        "edit_count",
        "match_strategy",
        "match_strategies",
        "bytes",
        "diagnostics",
        "post_edit_snippets",
        "files",
    ] {
        copy_context_field(&mut compact, object, field);
    }
    if let Some(artifacts) = object.get("artifacts") {
        compact.insert(
            "artifact_refs".to_string(),
            compact_artifact_refs(artifacts),
        );
    }
    if !compact.contains_key("post_edit_snippets") {
        if let Some(diff) = object.get("diff").and_then(Value::as_str) {
            compact.insert(
                "diff".to_string(),
                Value::String(compact_model_context_string(diff, 2_000)),
            );
        }
    }
    Value::Object(compact)
}

fn compact_artifact_refs(value: &Value) -> Value {
    let Some(items) = value.as_array() else {
        return compact_model_context_value(value);
    };
    Value::Array(
        items
            .iter()
            .take(8)
            .filter_map(|item| {
                let object = item.as_object()?;
                let mut compact = Map::new();
                for field in ["id", "kind", "title", "uri"] {
                    if let Some(value) = object.get(field) {
                        compact.insert(field.to_string(), compact_model_context_value(value));
                    }
                }
                if let Some(metadata) = object.get("metadata") {
                    compact.insert(
                        "metadata".to_string(),
                        compact_model_context_value(metadata),
                    );
                }
                Some(Value::Object(compact))
            })
            .collect(),
    )
}

fn compact_file_collection_context(object: &Map<String, Value>) -> Value {
    let mut compact = Map::new();
    for field in [
        "file_count",
        "bytes",
        "glob",
        "glob_source",
        "query",
        "effective_query",
        "mode",
        "no_matches",
        "no_match_streak",
        "search_hint",
        "max_bytes_per_file",
        "truncated",
        "truncation_hint",
    ] {
        copy_context_field(&mut compact, object, field);
    }
    if let Some(files) = object.get("files").and_then(Value::as_array) {
        compact.insert(
            "files".to_string(),
            Value::Array(
                files
                    .iter()
                    .take(40)
                    .map(|file| match file {
                        Value::String(path) => Value::String(path.clone()),
                        Value::Object(file) => compact_file_like_context(file, 8_000),
                        other => compact_model_context_value(other),
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(compact)
}

fn compact_file_like_context(object: &Map<String, Value>, max_content_chars: usize) -> Value {
    let mut compact = Map::new();
    for field in [
        "path",
        "bytes",
        "source_bytes",
        "start_line",
        "end_line",
        "match_line",
        "contains",
        "context_lines",
        "total_lines",
        "truncated",
        "full_output_path",
        "truncation_hint",
        "content_format",
        "unscoped_read",
        "line_numbers",
    ] {
        copy_context_field(&mut compact, object, field);
    }
    if let Some(content) = object.get("content").and_then(Value::as_str) {
        compact.insert(
            "content".to_string(),
            Value::String(compact_model_context_string(content, max_content_chars)),
        );
    } else if let Some(content) = object.get("content_preview").and_then(Value::as_str) {
        compact.insert(
            "content_preview".to_string(),
            Value::String(compact_model_context_string(content, max_content_chars)),
        );
    }
    Value::Object(compact)
}

fn copy_context_field(compact: &mut Map<String, Value>, object: &Map<String, Value>, field: &str) {
    if let Some(value) = object.get(field) {
        compact.insert(field.to_string(), compact_model_context_value(value));
    }
}

pub fn compact_model_context_string(text: &str, max_chars: usize) -> String {
    truncate_middle_context_string(text, max_chars, "AIR_COMPACTED")
}

pub fn truncate_middle_context_string(text: &str, max_chars: usize, marker_label: &str) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return format!("[{marker_label}]");
    }

    let mut marker = format!("\n[{marker_label}] {} chars omitted\n", total_chars);
    for _ in 0..4 {
        let marker_chars = marker.chars().count();
        if marker_chars >= max_chars {
            return marker_label_fallback(marker_label, max_chars);
        }
        let keep_chars = max_chars - marker_chars;
        let prefix_chars = keep_chars.div_ceil(2);
        let suffix_chars = keep_chars / 2;
        let omitted_chars = total_chars.saturating_sub(prefix_chars + suffix_chars);
        let next_marker = format!("\n[{marker_label}] {omitted_chars} chars omitted\n");
        if next_marker == marker {
            return middle_truncated_text(text, prefix_chars, suffix_chars, &marker);
        }
        marker = next_marker;
    }

    let marker_chars = marker.chars().count();
    if marker_chars >= max_chars {
        return marker_label_fallback(marker_label, max_chars);
    }
    let keep_chars = max_chars - marker_chars;
    middle_truncated_text(text, keep_chars.div_ceil(2), keep_chars / 2, &marker)
}

fn marker_label_fallback(marker_label: &str, max_chars: usize) -> String {
    format!("[{marker_label}]")
        .chars()
        .take(max_chars)
        .collect()
}

fn middle_truncated_text(
    text: &str,
    prefix_chars: usize,
    suffix_chars: usize,
    marker: &str,
) -> String {
    let total_chars = text.chars().count();
    let prefix = text.chars().take(prefix_chars).collect::<String>();
    let suffix = if suffix_chars == 0 {
        String::new()
    } else {
        text.chars()
            .skip(total_chars.saturating_sub(suffix_chars))
            .collect::<String>()
    };
    format!("{prefix}{marker}{suffix}")
}

fn compact_json_value(value: &Value, max_chars: usize) -> Value {
    let mut remaining = max_chars;
    compact_json_value_with_budget(value, &mut remaining)
}

fn compact_json_value_with_budget(value: &Value, remaining: &mut usize) -> Value {
    match value {
        Value::String(text) => {
            if *remaining == 0 {
                return Value::String(String::new());
            }
            let char_count = text.chars().count();
            if char_count <= *remaining {
                *remaining -= char_count;
                return Value::String(text.clone());
            }
            let truncated = truncate_middle_context_string(text, *remaining, "air:truncated");
            *remaining = 0;
            Value::String(truncated)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| compact_json_value_with_budget(value, remaining))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        compact_json_value_with_budget(value, remaining),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn json_value_size_bytes(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|value| value.len())
        .unwrap_or(0)
}
