use crate::file_tools::{read_snapshot, ReadSnapshot};
use air_runtime::RuntimeError;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadObservation {
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) complete: bool,
    pub(crate) snapshot: ReadSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchObservation {
    pub(crate) path: String,
    pub(crate) pattern: String,
    pub(crate) match_count: u64,
    pub(crate) returned_match_count: u64,
}

pub(crate) fn observe_file_read_output(
    read_snapshots: &mut BTreeMap<PathBuf, ReadSnapshot>,
    read_observations: &mut BTreeMap<PathBuf, Vec<ReadObservation>>,
    name: &str,
    output: &Value,
) -> Result<Option<Value>, RuntimeError> {
    let Some(path) = output.get("path").and_then(Value::as_str) else {
        return Ok(None);
    };
    let path = Path::new(path);
    let snapshot = read_snapshot(name, "input.path", path)?;
    let (start_line, end_line) = extract_read_line_range(output);
    if let Some(output) = large_unscoped_read_guard_output(output, start_line, end_line) {
        read_snapshots.insert(path.to_path_buf(), snapshot);
        return Ok(Some(output));
    }
    if let Some(covered_by) =
        covered_read_observation(read_observations, path, snapshot, start_line, end_line)
    {
        read_snapshots.insert(path.to_path_buf(), snapshot);
        return Ok(Some(repeated_read_output(
            output, start_line, end_line, covered_by,
        )));
    }
    let complete = !output
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    remember_read_observation(
        read_snapshots,
        read_observations,
        path,
        snapshot,
        start_line,
        end_line,
        complete,
    )?;
    Ok(None)
}

const SEARCH_COUNT_FIELDS: &[&str] = &["match_count", "file_count", "returned_match_count"];
const SEARCH_ARRAY_FIELDS: &[&str] = &["matches", "files", "symbols", "references", "locations"];

fn search_output_no_matches(output: &Value) -> bool {
    output
        .get("no_matches")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn any_search_count_positive(output: &Value) -> bool {
    SEARCH_COUNT_FIELDS
        .iter()
        .any(|&field| output.get(field).and_then(Value::as_u64).unwrap_or(0) > 0)
}

fn any_search_array_nonempty(output: &Value) -> bool {
    SEARCH_ARRAY_FIELDS.iter().any(|&field| {
        output
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn search_output_has_positive_results(output: &Value) -> bool {
    !search_output_no_matches(output)
        && (any_search_count_positive(output) || any_search_array_nonempty(output))
}

fn insert_output_field(output: &mut Value, key: &str, value: Value) {
    if let Some(object) = output.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

fn append_output_search_hint(output: &mut Value, hint: &str) {
    let Some(object) = output.as_object_mut() else {
        return;
    };
    let existing = object
        .get("search_hint")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let next = if existing.trim().is_empty() {
        hint.to_string()
    } else if existing.contains(hint) {
        existing.to_string()
    } else {
        format!("{existing} {hint}")
    };
    object.insert("search_hint".to_string(), Value::String(next));
}

pub(crate) fn annotate_search_progress(
    search_no_match_streak: &mut u32,
    mut output: Value,
) -> Value {
    if search_output_no_matches(&output) {
        *search_no_match_streak = search_no_match_streak.saturating_add(1);
        insert_output_field(
            &mut output,
            "no_match_streak",
            Value::from(*search_no_match_streak),
        );
        if *search_no_match_streak >= 2 {
            append_output_search_hint(
                &mut output,
                "Several recent searches returned no matches; broaden the search strategy, inspect the project file list, or switch to a more likely source/test extension instead of repeating this direction.",
            );
        }
    } else if search_output_has_positive_results(&output) {
        *search_no_match_streak = 0;
    }
    output
}

pub(crate) fn observed_tool_key(generation: u64, name: &str, input: &Value) -> String {
    let input = serde_json::to_string(input).unwrap_or_else(|_| "<unserializable>".to_string());
    format!("{generation}\n{name}\n{input}")
}

fn remember_read_observation(
    read_snapshots: &mut BTreeMap<PathBuf, ReadSnapshot>,
    read_observations: &mut BTreeMap<PathBuf, Vec<ReadObservation>>,
    path: &Path,
    snapshot: ReadSnapshot,
    start_line: usize,
    end_line: usize,
    complete: bool,
) -> Result<(), RuntimeError> {
    let path = fs::canonicalize(path).map_err(|error| {
        RuntimeError::Provider(format!("tool file snapshot canonicalize path: {error}"))
    })?;
    read_snapshots.insert(path.clone(), snapshot);
    read_observations
        .entry(path)
        .or_default()
        .push(ReadObservation {
            start_line,
            end_line,
            complete,
            snapshot,
        });
    Ok(())
}

fn covered_read_observation(
    read_observations: &BTreeMap<PathBuf, Vec<ReadObservation>>,
    path: &Path,
    snapshot: ReadSnapshot,
    start_line: usize,
    end_line: usize,
) -> Option<ReadObservation> {
    read_observations.get(path)?.iter().copied().find(|seen| {
        seen.complete
            && seen.snapshot == snapshot
            && seen.start_line == start_line
            && seen.end_line == end_line
    })
}

fn large_unscoped_read_guard_output(
    output: &Value,
    start_line: usize,
    end_line: usize,
) -> Option<Value> {
    if output
        .get("range_limited_unscoped_read")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return None;
    }
    if output.get("match_line").and_then(Value::as_u64).is_some() {
        return None;
    }
    Some(json!({
        "path": output.get("path").cloned().unwrap_or(Value::Null),
        "start_line": start_line,
        "end_line": end_line,
        "total_lines": output.get("total_lines").cloned().unwrap_or(Value::Null),
        "allow_whole_file": output.get("allow_whole_file").cloned().unwrap_or(Value::Bool(false)),
        "range_limited_unscoped_read": true,
        "content_skipped": true,
        "message": "Unscoped read skipped. Use grep, read with contains+context_lines, LSP, or a small range around known line numbers before requesting file content. If the whole file is truly needed, retry with allow_whole_file=true after locating or confirming the file is small.",
        "suggested_tools": ["grep", "read.contains", "lsp"],
        "artifacts": [{
            "kind": "tool_notice",
            "title": "read skipped: locate before reading",
            "content": "No file content returned because broad reads require an explicit allow_whole_file opt-in. Locate the relevant symbol or line range first.",
            "metadata": {
                "provider": "file_read",
                "content_skipped": true,
                "range_limited_unscoped_read": true,
                "start_line": start_line,
                "end_line": end_line
            }
        }]
    }))
}

fn extract_read_line_range(output: &Value) -> (usize, usize) {
    let total_lines = output
        .get("total_lines")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let start_line = output
        .get("start_line")
        .and_then(Value::as_u64)
        .map(|line| line as usize)
        .unwrap_or(1);
    let end_line = output
        .get("end_line")
        .and_then(Value::as_u64)
        .map(|line| line as usize)
        .unwrap_or(total_lines);
    (start_line, end_line)
}

pub(crate) fn repeated_read_output(
    previous_output: &Value,
    start_line: usize,
    end_line: usize,
    covered_by: ReadObservation,
) -> Value {
    json!({
        "path": previous_output.get("path").cloned().unwrap_or(Value::Null),
        "start_line": start_line,
        "end_line": end_line,
        "total_lines": previous_output.get("total_lines").cloned().unwrap_or(Value::Null),
        "no_new_information": true,
        "already_read": true,
        "covered_by": {
            "start_line": covered_by.start_line,
            "end_line": covered_by.end_line
        },
        "message": "This line range is already covered by a previous read and the file has not changed. Use the prior context, request a different line range, or edit based on the existing evidence.",
        "artifacts": [{
            "kind": "tool_notice",
            "title": "read skipped: already covered",
            "content": "No new file content returned because this unchanged line range was already read.",
            "metadata": {
                "provider": "file_read",
                "no_new_information": true,
                "already_read": true,
                "covered_start_line": covered_by.start_line,
                "covered_end_line": covered_by.end_line
            }
        }]
    })
}

pub(crate) fn repeated_search_output(previous: &SearchObservation) -> Value {
    json!({
        "path": previous.path.clone(),
        "pattern": previous.pattern.clone(),
        "match_count": previous.match_count,
        "returned_match_count": previous.returned_match_count,
        "no_new_information": true,
        "already_seen": true,
        "message": "This exact search was already run for the current workspace state. Use the previous matches, narrow the query, or edit based on the existing evidence.",
        "artifacts": [{
            "kind": "tool_notice",
            "title": "search skipped: already seen",
            "content": "No match list returned because this exact search was already run.",
            "metadata": {
                "provider": "file_search",
                "no_new_information": true,
                "already_seen": true,
                "match_count": previous.match_count,
                "returned_match_count": previous.returned_match_count
            }
        }]
    })
}

pub(crate) fn search_observation_from_output(output: &Value) -> SearchObservation {
    SearchObservation {
        path: output
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        pattern: output
            .get("pattern")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        match_count: output
            .get("match_count")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        returned_match_count: output
            .get("returned_match_count")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}
