use air_runtime::RuntimeError;
use serde_json::{json, Value};

use crate::{json_char_count, optional_bounded_usize_input, optional_threshold_percent_input};

pub(crate) fn call_context_measure_tool(
    name: &str,
    input: &Value,
    max_context_chars: usize,
    threshold_percent: u64,
) -> Result<Value, RuntimeError> {
    if threshold_percent == 0 || threshold_percent > 100 {
        return Err(RuntimeError::Provider(format!(
            "tool {name} threshold_percent must be between 1 and 100"
        )));
    }
    let effective_max_context_chars =
        optional_bounded_usize_input(name, input, "max_context_chars", max_context_chars)?
            .unwrap_or(max_context_chars);
    let effective_threshold_percent =
        optional_threshold_percent_input(name, input, "threshold_percent")?
            .unwrap_or(threshold_percent);
    let payload = input.get("payload").unwrap_or(input);
    let chars = json_char_count(payload);
    let threshold_chars =
        effective_max_context_chars.saturating_mul(effective_threshold_percent as usize) / 100;
    let should_compact = chars >= threshold_chars;
    let fields = payload
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(field, value)| {
                    json!({
                        "field": field,
                        "chars": json_char_count(value)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let content = format!(
        "chars: {chars}\nmax_context_chars: {effective_max_context_chars}\nthreshold_percent: {effective_threshold_percent}\nthreshold_chars: {threshold_chars}\nshould_compact: {should_compact}"
    );
    Ok(json!({
        "chars": chars,
        "max_context_chars": effective_max_context_chars,
        "threshold_percent": effective_threshold_percent,
        "threshold_chars": threshold_chars,
        "usage_ratio": chars as f64 / effective_max_context_chars.max(1) as f64,
        "should_compact": should_compact,
        "fields": fields,
        "artifacts": [{
            "id": format!("context-measure:{chars}:{threshold_chars}"),
            "kind": "context_measure",
            "title": "context measure",
            "uri": "air://context/measure",
            "content": content,
            "metadata": {
                "provider": "context_measure",
                "chars": chars,
                "max_context_chars": effective_max_context_chars,
                "threshold_percent": effective_threshold_percent,
                "threshold_chars": threshold_chars,
                "should_compact": should_compact
            }
        }]
    }))
}
