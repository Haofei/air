use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::RuntimeError;

pub(crate) fn call_artifact_validate_tool(
    name: &str,
    input: &Value,
    fail_on_missing: bool,
    max_ids: usize,
) -> Result<Value, RuntimeError> {
    let mut registered_ids = BTreeSet::new();
    let mut registered_artifacts = Vec::new();
    if let Some(ids) = input.get("registered_ids") {
        collect_string_values(name, "registered_ids", ids, &mut registered_ids, max_ids)?;
    }
    if let Some(evidence) = input.get("evidence") {
        collect_artifact_ids(
            name,
            evidence,
            &mut registered_ids,
            &mut registered_artifacts,
            max_ids,
        )?;
    }

    let mut cited_ids = BTreeSet::new();
    if let Some(citations) = input.get("citations") {
        collect_citation_ids(name, citations, &mut cited_ids, max_ids)?;
    }
    if let Some(ids) = input.get("source_ids") {
        collect_string_values(name, "source_ids", ids, &mut cited_ids, max_ids)?;
    }
    if let Some(ids) = input.get("artifact_ids") {
        collect_string_values(name, "artifact_ids", ids, &mut cited_ids, max_ids)?;
    }

    let missing_ids = cited_ids
        .difference(&registered_ids)
        .cloned()
        .collect::<Vec<_>>();
    if fail_on_missing && !missing_ids.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} citations reference unregistered artifact ids: {}",
            missing_ids.join(", ")
        )));
    }

    let unused_registered_ids = registered_ids
        .difference(&cited_ids)
        .cloned()
        .collect::<Vec<_>>();
    let registered_ids = registered_ids.into_iter().collect::<Vec<_>>();
    let cited_ids = cited_ids.into_iter().collect::<Vec<_>>();
    let valid = missing_ids.is_empty();
    let content = artifact_validation_summary_content(
        valid,
        registered_ids.len(),
        cited_ids.len(),
        missing_ids.len(),
    );

    Ok(json!({
        "valid": valid,
        "registered_ids": registered_ids,
        "cited_ids": cited_ids,
        "missing_ids": missing_ids,
        "unused_registered_ids": unused_registered_ids,
        "registered_artifacts": registered_artifacts,
        "artifacts": [{
            "id": "artifact-validation:citations",
            "kind": "artifact_validation",
            "title": "citation artifact validation",
            "uri": "air://artifacts/validation/citations",
            "content": content,
            "metadata": {
                "provider": "artifact_validate",
                "valid": valid
            }
        }]
    }))
}

fn collect_artifact_ids(
    tool_name: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    artifacts: &mut Vec<Value>,
    max_ids: usize,
) -> Result<(), RuntimeError> {
    match value {
        Value::Object(object) => {
            if let Some(raw_artifacts) = object.get("artifacts") {
                let raw_artifacts = raw_artifacts.as_array().ok_or_else(|| {
                    RuntimeError::Provider(format!(
                        "tool {tool_name} evidence.artifacts must be an array"
                    ))
                })?;
                for artifact in raw_artifacts {
                    let Some(object) = artifact.as_object() else {
                        continue;
                    };
                    let Some(id) = object.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    insert_non_empty_id(tool_name, "evidence.artifacts.id", id, ids, max_ids)?;
                    artifacts.push(json!({
                        "id": id,
                        "kind": object.get("kind").cloned().unwrap_or(Value::Null),
                        "title": object.get("title").cloned().unwrap_or(Value::Null),
                        "uri": object.get("uri").cloned().unwrap_or(Value::Null)
                    }));
                }
            }
            for child in object.values() {
                collect_artifact_ids(tool_name, child, ids, artifacts, max_ids)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_artifact_ids(tool_name, child, ids, artifacts, max_ids)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn collect_citation_ids(
    tool_name: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    max_ids: usize,
) -> Result<(), RuntimeError> {
    collect_citation_ids_inner(tool_name, value, ids, max_ids, false)
}

fn collect_citation_ids_inner(
    tool_name: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    max_ids: usize,
    collect_strings: bool,
) -> Result<(), RuntimeError> {
    match value {
        Value::String(id) => {
            if collect_strings {
                insert_non_empty_id(tool_name, "citations", id, ids, max_ids)
            } else {
                Ok(())
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_citation_ids_inner(tool_name, child, ids, max_ids, collect_strings)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            for (key, child) in object {
                if is_citation_key(key) {
                    collect_string_values(tool_name, key, child, ids, max_ids)?;
                } else {
                    collect_citation_ids_inner(tool_name, child, ids, max_ids, false)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn is_citation_key(key: &str) -> bool {
    matches!(
        key,
        "source_id"
            | "source_ids"
            | "artifact_id"
            | "artifact_ids"
            | "citation"
            | "citations"
            | "source"
            | "sources"
    )
}

fn collect_string_values(
    tool_name: &str,
    field: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    max_ids: usize,
) -> Result<(), RuntimeError> {
    match value {
        Value::String(id) => insert_non_empty_id(tool_name, field, id, ids, max_ids),
        Value::Array(values) => {
            for child in values {
                collect_string_values(tool_name, field, child, ids, max_ids)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            for child in object.values() {
                collect_string_values(tool_name, field, child, ids, max_ids)?;
            }
            Ok(())
        }
        Value::Null => Ok(()),
        _ => Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must contain strings, arrays, or objects"
        ))),
    }
}

fn insert_non_empty_id(
    tool_name: &str,
    field: &str,
    id: &str,
    ids: &mut BTreeSet<String>,
    max_ids: usize,
) -> Result<(), RuntimeError> {
    let id = parse_artifact_id(tool_name, field, id)?;
    ids.insert(id);
    if ids.len() > max_ids {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must reference at most {max_ids} ids"
        )));
    }
    Ok(())
}

fn parse_artifact_id(tool_name: &str, field: &str, id: &str) -> Result<String, RuntimeError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} entries must not be empty"
        )));
    }
    Ok(id.to_string())
}

fn artifact_validation_summary_content(
    valid: bool,
    registered_count: usize,
    cited_count: usize,
    missing_count: usize,
) -> String {
    format!(
        "valid: {valid}\nregistered_ids: {registered_count}\ncited_ids: {cited_count}\nmissing_ids: {missing_count}"
    )
}
