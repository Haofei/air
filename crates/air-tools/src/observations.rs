use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub(crate) struct SearchObservation {
    pub(crate) path: String,
    pub(crate) pattern: String,
    pub(crate) match_count: u64,
    pub(crate) returned_match_count: u64,
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
