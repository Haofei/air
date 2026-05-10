use serde_json::{json, Map, Value};
use std::collections::HashSet;

pub(crate) fn code_project_memory(executions: &[Value]) -> Value {
    let tasks = executions
        .iter()
        .map(code_project_execution_memory)
        .collect::<Vec<_>>();
    let mut changed_files = Vec::new();
    let mut artifact_ids = Vec::new();
    let mut artifact_kinds = Vec::new();
    for task in &tasks {
        collect_string_array_field(task.get("changed_files"), &mut changed_files);
        collect_string_array_field(task.get("artifact_ids"), &mut artifact_ids);
        collect_string_array_field(task.get("artifact_kinds"), &mut artifact_kinds);
    }
    changed_files.sort();
    changed_files.dedup();
    artifact_ids.sort();
    artifact_ids.dedup();
    artifact_kinds.sort();
    artifact_kinds.dedup();

    json!({
        "task_count": executions.len(),
        "completed_task_count": executions
            .iter()
            .filter(|execution| execution.get("completed").and_then(Value::as_bool).unwrap_or(false))
            .count(),
        "failed_task_count": executions
            .iter()
            .filter(|execution| !execution.get("completed").and_then(Value::as_bool).unwrap_or(false))
            .count(),
        "changed_files": changed_files,
        "artifact_ids": artifact_ids,
        "artifact_kinds": artifact_kinds,
        "tasks": tasks,
    })
}

pub(crate) fn code_project_artifacts(trace_files: &[String], executions: &[Value]) -> Value {
    let mut artifacts = Vec::new();
    let mut seen = HashSet::new();
    for trace_file in trace_files {
        push_project_artifact(
            &mut artifacts,
            &mut seen,
            json!({
                "id": format!("trace:{trace_file}"),
                "kind": "trace_jsonl",
                "path": trace_file,
            }),
        );
    }
    for execution in executions {
        let task_id = execution
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if let Some(outputs) = execution.get("outputs") {
            collect_project_output_artifacts(
                outputs,
                task_id,
                "$.outputs",
                &mut artifacts,
                &mut seen,
            );
            for path in code_project_changed_files(outputs) {
                push_project_artifact(
                    &mut artifacts,
                    &mut seen,
                    json!({
                        "id": format!("changed-file:{path}"),
                        "kind": "changed_file",
                        "path": path,
                        "task_id": task_id,
                    }),
                );
            }
        }
    }
    Value::Array(artifacts)
}

pub(crate) fn push_project_artifact(
    artifacts: &mut Vec<Value>,
    seen: &mut HashSet<String>,
    artifact: Value,
) {
    let Some(id) = artifact.get("id").and_then(Value::as_str) else {
        return;
    };
    if seen.insert(id.to_string()) {
        artifacts.push(artifact);
    }
}

fn collect_project_output_artifacts(
    value: &Value,
    task_id: &str,
    source: &str,
    artifacts: &mut Vec<Value>,
    seen: &mut HashSet<String>,
) {
    match value {
        Value::Object(object) => {
            if let Some(values) = object.get("artifacts").and_then(Value::as_array) {
                for (index, artifact) in values.iter().enumerate() {
                    if let Some(mut artifact) = normalize_project_artifact(artifact) {
                        if let Some(object) = artifact.as_object_mut() {
                            object
                                .insert("task_id".to_string(), Value::String(task_id.to_string()));
                            object.insert(
                                "source".to_string(),
                                Value::String(format!("{source}.artifacts[{index}]")),
                            );
                        }
                        push_project_artifact(artifacts, seen, artifact);
                    }
                }
            }
            for (key, value) in object {
                collect_project_output_artifacts(
                    value,
                    task_id,
                    &format!("{source}.{key}"),
                    artifacts,
                    seen,
                );
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_project_output_artifacts(
                    value,
                    task_id,
                    &format!("{source}[{index}]"),
                    artifacts,
                    seen,
                );
            }
        }
        _ => {}
    }
}

fn normalize_project_artifact(artifact: &Value) -> Option<Value> {
    let object = artifact.as_object()?;
    let id = object.get("id").and_then(Value::as_str)?;
    let mut normalized = object.clone();
    normalized
        .entry("kind".to_string())
        .or_insert_with(|| Value::String("artifact".to_string()));
    normalized.insert("id".to_string(), Value::String(id.to_string()));
    Some(Value::Object(normalized))
}

fn code_project_execution_memory(execution: &Value) -> Value {
    let mut memory = Map::new();
    for key in [
        "task_id",
        "title",
        "recipe",
        "depends_on",
        "completed",
        "error",
    ] {
        if let Some(value) = execution.get(key) {
            memory.insert(key.to_string(), value.clone());
        }
    }
    if let Some(acceptance) = execution.get("acceptance").and_then(Value::as_array) {
        memory.insert(
            "acceptance".to_string(),
            Value::Array(
                acceptance
                    .iter()
                    .map(code_project_acceptance_memory)
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if let Some(outputs) = execution.get("outputs") {
        memory.insert(
            "output_keys".to_string(),
            Value::Array(
                value_keys(Some(outputs))
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        memory.insert("outputs".to_string(), code_project_outputs_memory(outputs));
        let changed_files = code_project_changed_files(outputs);
        if !changed_files.is_empty() {
            memory.insert(
                "changed_files".to_string(),
                Value::Array(changed_files.into_iter().map(Value::String).collect()),
            );
        }
        let artifact_ids = collect_artifact_field_values(outputs, "id");
        if !artifact_ids.is_empty() {
            memory.insert(
                "artifact_ids".to_string(),
                Value::Array(artifact_ids.into_iter().map(Value::String).collect()),
            );
        }
        let artifact_kinds = collect_artifact_field_values(outputs, "kind");
        if !artifact_kinds.is_empty() {
            memory.insert(
                "artifact_kinds".to_string(),
                Value::Array(artifact_kinds.into_iter().map(Value::String).collect()),
            );
        }
    }
    Value::Object(memory)
}

fn code_project_acceptance_memory(acceptance: &Value) -> Value {
    let mut memory = Map::new();
    for key in ["id", "scope", "command", "expected", "success", "error"] {
        if let Some(value) = acceptance.get(key) {
            memory.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(memory)
}

fn code_project_outputs_memory(outputs: &Value) -> Value {
    let mut memory = Map::new();
    if let Some(exploration) = outputs.get("exploration") {
        copy_selected_output_fields(
            &mut memory,
            "exploration",
            exploration,
            &[
                "summary",
                "findings",
                "relevant_files",
                "source_ids",
                "next_steps",
            ],
        );
    }
    if let Some(review) = outputs.get("review") {
        copy_selected_output_fields(
            &mut memory,
            "review",
            review,
            &[
                "summary",
                "findings",
                "search_quality",
                "source_ids",
                "recommended_actions",
            ],
        );
    }
    if let Some(edit) = outputs.get("edit") {
        copy_selected_output_fields(
            &mut memory,
            "edit",
            edit,
            &[
                "target_path",
                "initial_success",
                "final_success",
                "patch_applied",
                "changed_files",
                "workspace_changed_files",
                "preexisting_changed_files",
                "diagnostics",
                "summary",
            ],
        );
        if let Some(diff) = edit.get("workspace_diff") {
            copy_selected_output_fields(
                &mut memory,
                "edit_workspace_diff",
                diff,
                &["repo", "bytes", "truncated", "artifacts"],
            );
        }
    }
    if memory.is_empty() {
        collect_summary_fields(outputs, "$", &mut memory);
    }
    Value::Object(memory)
}

fn copy_selected_output_fields(
    memory: &mut Map<String, Value>,
    namespace: &str,
    value: &Value,
    fields: &[&str],
) {
    let mut object = Map::new();
    for field in fields {
        if let Some(value) = value.get(*field) {
            object.insert((*field).to_string(), value.clone());
        }
    }
    if !object.is_empty() {
        memory.insert(namespace.to_string(), Value::Object(object));
    }
}

fn collect_summary_fields(value: &Value, path: &str, output: &mut Map<String, Value>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let next = if path == "$" {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                if matches!(key.as_str(), "summary" | "reason" | "recommended_action") {
                    output.insert(next.clone(), value.clone());
                }
                collect_summary_fields(value, &next, output);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_summary_fields(value, &format!("{path}[{index}]"), output);
            }
        }
        _ => {}
    }
}

fn code_project_changed_files(outputs: &Value) -> Vec<String> {
    let mut files = Vec::new();
    collect_path_values(
        outputs
            .get("edit")
            .and_then(|edit| edit.get("changed_files")),
        &mut files,
    );
    collect_path_values(
        outputs
            .get("edit")
            .and_then(|edit| edit.get("workspace_changed_files")),
        &mut files,
    );
    collect_path_values(
        outputs
            .get("edit")
            .and_then(|edit| edit.get("workspace_diff"))
            .and_then(|diff| diff.get("artifacts")),
        &mut files,
    );
    files.sort();
    files.dedup();
    files
}

fn collect_path_values(value: Option<&Value>, output: &mut Vec<String>) {
    match value {
        Some(Value::Array(values)) => {
            for value in values {
                collect_path_values(Some(value), output);
            }
        }
        Some(Value::Object(object)) => {
            if let Some(path) = object.get("path").and_then(Value::as_str) {
                output.push(path.to_string());
            }
        }
        Some(Value::String(path)) => output.push(path.clone()),
        _ => {}
    }
}

fn collect_artifact_field_values(value: &Value, field: &str) -> Vec<String> {
    let mut values = Vec::new();
    collect_artifact_field_values_into(value, field, &mut values);
    values.sort();
    values.dedup();
    values
}

fn collect_artifact_field_values_into(value: &Value, field: &str, output: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(artifacts) = object.get("artifacts").and_then(Value::as_array) {
                for artifact in artifacts {
                    if let Some(value) = artifact.get(field).and_then(Value::as_str) {
                        output.push(value.to_string());
                    }
                }
            }
            for value in object.values() {
                collect_artifact_field_values_into(value, field, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_artifact_field_values_into(value, field, output);
            }
        }
        _ => {}
    }
}

fn collect_string_array_field(value: Option<&Value>, output: &mut Vec<String>) {
    let Some(Value::Array(values)) = value else {
        return;
    };
    output.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
}

fn value_keys(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Object(object)) => object.keys().cloned().collect(),
        _ => Vec::new(),
    }
}
