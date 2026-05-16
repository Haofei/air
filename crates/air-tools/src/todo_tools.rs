use air_runtime::RuntimeError;
use serde_json::{json, Value};

fn validate_single_todo_item(name: &str, index: usize, todo: &Value) -> Result<(), RuntimeError> {
    let Some(object) = todo.as_object() else {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.todos[{index}] must be an object"
        )));
    };
    for field in ["content", "status"] {
        if !object.get(field).is_some_and(Value::is_string) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].{field} must be a string"
            )));
        }
    }
    if let Some(priority) = object.get("priority") {
        if !priority.is_string() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].priority must be a string"
            )));
        }
    }
    Ok(())
}

pub(super) fn call_todowrite_tool(name: &str, input: &Value) -> Result<Value, RuntimeError> {
    let todos = input
        .get("todos")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.todos must be an array"))
        })?;
    for (index, todo) in todos.iter().enumerate() {
        validate_single_todo_item(name, index, todo)?;
    }
    Ok(json!({
        "todos": todos,
        "todo_count": todos.len(),
        "artifacts": [{
            "id": "todos:current",
            "kind": "todo_list",
            "title": "todos",
            "uri": "memory://todos/current",
            "content": serde_json::to_string_pretty(todos).unwrap_or_else(|_| "[]".to_string()),
            "metadata": {
                "provider": "todowrite",
                "todo_count": todos.len()
            }
        }]
    }))
}
