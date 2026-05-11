use crate::RuntimeError;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

pub(crate) fn call_todo_write_tool(
    name: &str,
    input: &Value,
    max_items: usize,
    max_content_chars: usize,
) -> Result<Value, RuntimeError> {
    let todos = input
        .get("todos")
        .or_else(|| input.get("items"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {name} input.todos (or input.items) must be an array"
            ))
        })?;
    if todos.len() > max_items {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.todos must contain at most {max_items} items"
        )));
    }

    let mut seen_ids = BTreeSet::new();
    let mut normalized = Vec::new();
    let mut pending_count = 0usize;
    let mut in_progress_count = 0usize;
    let mut completed_count = 0usize;
    let mut cancelled_count = 0usize;

    for (index, todo) in todos.iter().enumerate() {
        let object = todo.as_object().ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}] must be an object"
            ))
        })?;
        let id = required_todo_id(name, object, index)?;
        let content = required_object_string(name, object, &format!("todos[{index}].content"))?;
        let status = required_object_string(name, object, &format!("todos[{index}].status"))?;
        let priority = required_object_string(name, object, &format!("todos[{index}].priority"))?;
        if id.trim().is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].id must not be empty"
            )));
        }
        if !seen_ids.insert(id.to_string()) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].id must be unique"
            )));
        }
        if content.trim().is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].content must not be empty"
            )));
        }
        if content.chars().count() > max_content_chars {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].content must contain at most {max_content_chars} characters"
            )));
        }
        match status {
            "pending" => pending_count += 1,
            "in_progress" => in_progress_count += 1,
            "completed" => completed_count += 1,
            "cancelled" => cancelled_count += 1,
            _ => {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} input.todos[{index}].status must be pending, in_progress, completed, or cancelled"
                )));
            }
        }
        if !matches!(priority, "high" | "medium" | "low") {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].priority must be high, medium, or low"
            )));
        }

        normalized.push(json!({
            "id": id,
            "content": content,
            "status": status,
            "priority": priority,
        }));
    }

    if in_progress_count > 1 {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.todos must contain at most one in_progress item"
        )));
    }

    Ok(todo_list_output(
        name,
        "todo_write",
        normalized,
        TodoCounts {
            pending: pending_count,
            in_progress: in_progress_count,
            completed: completed_count,
            cancelled: cancelled_count,
        },
    ))
}

fn required_todo_id(
    name: &str,
    object: &serde_json::Map<String, Value>,
    index: usize,
) -> Result<String, RuntimeError> {
    let field = format!("todos[{index}].id");
    let value = object
        .get("id")
        .ok_or_else(|| RuntimeError::Provider(format!("tool {name} input.{field} is required")))?;
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err(RuntimeError::Provider(format!(
            "tool {name} input.{field} must be a string or number"
        ))),
    }
}

pub(crate) fn call_todo_read_tool(name: &str, current_todos: &[Value]) -> Value {
    let counts = todo_counts(current_todos);
    todo_list_output(name, "todo_read", current_todos.to_vec(), counts)
}

#[derive(Debug, Clone, Copy)]
struct TodoCounts {
    pending: usize,
    in_progress: usize,
    completed: usize,
    cancelled: usize,
}

fn todo_counts(todos: &[Value]) -> TodoCounts {
    let mut counts = TodoCounts {
        pending: 0,
        in_progress: 0,
        completed: 0,
        cancelled: 0,
    };
    for todo in todos {
        match todo
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "pending" => counts.pending += 1,
            "in_progress" => counts.in_progress += 1,
            "completed" => counts.completed += 1,
            "cancelled" => counts.cancelled += 1,
            _ => {}
        }
    }
    counts
}

fn todo_list_output(name: &str, provider: &str, todos: Vec<Value>, counts: TodoCounts) -> Value {
    let open_count = counts.pending + counts.in_progress;
    let content = serde_json::to_string_pretty(&todos).unwrap_or_else(|_| "[]".to_string());
    json!({
        "todos": todos,
        "total": todos.len(),
        "open_count": open_count,
        "pending_count": counts.pending,
        "in_progress_count": counts.in_progress,
        "completed_count": counts.completed,
        "cancelled_count": counts.cancelled,
        "artifacts": [{
            "id": "todo:list",
            "kind": "todo_list",
            "title": format!("{open_count} open todos"),
            "uri": "air://todos/current",
            "content": content,
            "metadata": {
                "provider": provider,
                "tool": name,
                "total": todos.len(),
                "open_count": open_count,
                "pending_count": counts.pending,
                "in_progress_count": counts.in_progress,
                "completed_count": counts.completed,
                "cancelled_count": counts.cancelled
            }
        }],
    })
}

fn required_object_string<'a>(
    tool_name: &str,
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, RuntimeError> {
    object
        .get(field.rsplit('.').next().unwrap_or(field))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a string"))
        })
}
