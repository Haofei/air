use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CodeBudgetLimits {
    pub(crate) max_estimated_model_calls: Option<usize>,
    pub(crate) max_estimated_tool_calls: Option<usize>,
}

fn build_budget_map(
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
    budget_limits: CodeBudgetLimits,
    model_exceeded: bool,
    tool_exceeded: bool,
) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    map.insert(
        "max_estimated_model_calls".to_string(),
        json!(budget_limits.max_estimated_model_calls),
    );
    map.insert(
        "max_estimated_tool_calls".to_string(),
        json!(budget_limits.max_estimated_tool_calls),
    );
    map.insert(
        "attempted_model_calls".to_string(),
        json!(max_estimated_model_calls),
    );
    map.insert(
        "attempted_tool_calls".to_string(),
        json!(max_estimated_tool_calls),
    );
    map.insert("model_exceeded".to_string(), Value::Bool(model_exceeded));
    map.insert("tool_exceeded".to_string(), Value::Bool(tool_exceeded));
    map
}

pub(crate) fn code_budget_limit_status(
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
    budget_limits: CodeBudgetLimits,
) -> Value {
    let model_exceeded = budget_limits
        .max_estimated_model_calls
        .map(|limit| max_estimated_model_calls > limit)
        .unwrap_or(false);
    let tool_exceeded = budget_limits
        .max_estimated_tool_calls
        .map(|limit| max_estimated_tool_calls > limit)
        .unwrap_or(false);
    let exceeded = model_exceeded || tool_exceeded;
    let mut map = build_budget_map(
        max_estimated_model_calls,
        max_estimated_tool_calls,
        budget_limits,
        model_exceeded,
        tool_exceeded,
    );
    map.insert("exceeded".to_string(), Value::Bool(exceeded));
    Value::Object(map)
}

pub(crate) fn code_budget_limit_violation(
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
    budget_limits: CodeBudgetLimits,
) -> Option<Value> {
    let model_exceeded = budget_limits
        .max_estimated_model_calls
        .map(|limit| max_estimated_model_calls > limit)
        .unwrap_or(false);
    let tool_exceeded = budget_limits
        .max_estimated_tool_calls
        .map(|limit| max_estimated_tool_calls > limit)
        .unwrap_or(false);
    if !model_exceeded && !tool_exceeded {
        return None;
    }
    Some(Value::Object(build_budget_map(
        max_estimated_model_calls,
        max_estimated_tool_calls,
        budget_limits,
        model_exceeded,
        tool_exceeded,
    )))
}

pub(crate) fn code_budget_value_counts(budget: &Value) -> (usize, usize) {
    let model_calls = budget
        .get("max_estimated_model_calls")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    let tool_calls = budget
        .get("max_estimated_tool_calls")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    (model_calls, tool_calls)
}
