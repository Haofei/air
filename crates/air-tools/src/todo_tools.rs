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
    let mut counts = TodoCounts::default();

    for (index, todo) in todos.iter().enumerate() {
        let result =
            validate_and_normalize_todo(name, todo, index, max_content_chars, &mut seen_ids)?;
        counts.record(result.status);
        normalized.push(result.normalized);
    }

    if counts.in_progress > 1 {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.todos must contain at most one in_progress item"
        )));
    }

    Ok(todo_list_output(name, "todo_write", normalized, counts))
}

struct ValidatedTodo {
    normalized: Value,
    status: TodoStatusKind,
}

#[derive(Debug, Clone, Copy)]
enum TodoStatusKind {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatusKind {
    fn parse(name: &str, status: &str, index: usize) -> Result<Self, RuntimeError> {
        match status {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].status must be pending, in_progress, completed, or cancelled"
            ))),
        }
    }
}

fn validate_and_normalize_todo(
    name: &str,
    todo: &Value,
    index: usize,
    max_content_chars: usize,
    seen_ids: &mut BTreeSet<String>,
) -> Result<ValidatedTodo, RuntimeError> {
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
    let status_kind = TodoStatusKind::parse(name, status, index)?;
    validate_todo_priority(name, priority, index)?;

    let normalized = json!({
        "id": id,
        "content": content,
        "status": status,
        "priority": priority,
    });
    Ok(ValidatedTodo {
        normalized,
        status: status_kind,
    })
}

fn validate_todo_priority(name: &str, priority: &str, index: usize) -> Result<(), RuntimeError> {
    if !matches!(priority, "high" | "medium" | "low") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.todos[{index}].priority must be high, medium, or low"
        )));
    }
    Ok(())
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

#[derive(Debug, Clone, Copy, Default)]
struct TodoCounts {
    pending: usize,
    in_progress: usize,
    completed: usize,
    cancelled: usize,
}

impl TodoCounts {
    fn record(&mut self, status: TodoStatusKind) {
        match status {
            TodoStatusKind::Pending => self.pending += 1,
            TodoStatusKind::InProgress => self.in_progress += 1,
            TodoStatusKind::Completed => self.completed += 1,
            TodoStatusKind::Cancelled => self.cancelled += 1,
        }
    }
}

fn todo_status_kind_from_value(value: &Value) -> Option<TodoStatusKind> {
    match value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "pending" => Some(TodoStatusKind::Pending),
        "in_progress" => Some(TodoStatusKind::InProgress),
        "completed" => Some(TodoStatusKind::Completed),
        "cancelled" => Some(TodoStatusKind::Cancelled),
        _ => None,
    }
}

fn todo_counts(todos: &[Value]) -> TodoCounts {
    let mut counts = TodoCounts::default();
    for todo in todos {
        if let Some(status) = todo_status_kind_from_value(todo) {
            counts.record(status);
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
