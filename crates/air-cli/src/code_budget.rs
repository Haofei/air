use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CodeBudgetLimits {
    pub(crate) max_estimated_model_calls: Option<usize>,
    pub(crate) max_estimated_tool_calls: Option<usize>,
}

pub(crate) fn code_budget_limit_status(
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
    budget_limits: CodeBudgetLimits,
) -> Value {
    if let Some(Value::Object(mut violation)) = code_budget_limit_violation(
        max_estimated_model_calls,
        max_estimated_tool_calls,
        budget_limits,
    ) {
        violation.insert("exceeded".to_string(), Value::Bool(true));
        return Value::Object(violation);
    }
    json!({
        "max_estimated_model_calls": budget_limits.max_estimated_model_calls,
        "max_estimated_tool_calls": budget_limits.max_estimated_tool_calls,
        "attempted_model_calls": max_estimated_model_calls,
        "attempted_tool_calls": max_estimated_tool_calls,
        "model_exceeded": false,
        "tool_exceeded": false,
        "exceeded": false,
    })
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
    Some(json!({
        "max_estimated_model_calls": budget_limits.max_estimated_model_calls,
        "max_estimated_tool_calls": budget_limits.max_estimated_tool_calls,
        "attempted_model_calls": max_estimated_model_calls,
        "attempted_tool_calls": max_estimated_tool_calls,
        "model_exceeded": model_exceeded,
        "tool_exceeded": tool_exceeded,
    }))
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
