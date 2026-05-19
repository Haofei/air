use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

pub(crate) fn read_json_object(
    path: PathBuf,
    label: &str,
) -> Result<serde_json::Map<String, Value>> {
    let source = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {label} JSON file {}", path.display()))?;
    match serde_json::from_str::<Value>(&source)
        .with_context(|| format!("failed to parse {label} JSON file {}", path.display()))?
    {
        Value::Object(object) => Ok(object),
        other => anyhow::bail!(
            "{label} JSON file {} must contain a JSON object, got {}",
            path.display(),
            json_kind(&other)
        ),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
