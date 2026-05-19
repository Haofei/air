use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Duration;

mod errors;
mod model_context;
mod trace;

pub use errors::RuntimeError;
pub use model_context::{
    compact_model_context_string, compact_model_context_value, take_last_within_bytes_value,
    truncate_middle_context_string,
};
pub use trace::{
    read_trace_jsonl, replay_outputs, sanitize_trace_event, sanitize_trace_text,
    system_return_event, write_trace_jsonl, write_trace_jsonl_with_options, TraceEvent,
    TraceStatus, TraceWriteOptions,
};

pub type State = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub outputs: State,
    pub trace: Vec<TraceEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDecision {
    pub approved: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approver: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ApprovalDecision {
    pub fn approved(approver: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            approved: true,
            approver: Some(approver.into()),
            reason: Some(reason.into()),
            metadata: None,
        }
    }

    pub fn denied(reason: impl Into<String>) -> Self {
        Self {
            approved: false,
            approver: None,
            reason: Some(reason.into()),
            metadata: None,
        }
    }
}

pub trait ToolProvider {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError>;

    fn call_tool_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        _timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        self.call_tool(name, input)
    }

    fn tool_capability(&self, _name: &str) -> Option<&str> {
        None
    }

    fn request_approval(
        &mut self,
        module: &str,
        approval_for: &[String],
        _state: &Value,
    ) -> Result<ApprovalDecision, RuntimeError> {
        Err(RuntimeError::ApprovalRequired {
            module: module.to_string(),
            capabilities: approval_for.to_vec(),
        })
    }
}

pub trait ModelProvider {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError>;

    fn call_model_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        _timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        self.call_model(name, input)
    }

    fn take_last_request_stats(&mut self) -> Option<ModelRequestStats> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequestStats {
    pub provider_request_bytes: usize,
    pub provider_user_content_bytes: usize,
    pub provider_tools_bytes: usize,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request: Option<Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_response: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(action: &str, output: Option<Value>) -> TraceEvent {
        TraceEvent {
            agent: "$system".to_string(),
            step: 0,
            rule: "test".to_string(),
            action: action.to_string(),
            input: None,
            output,
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        }
    }

    #[test]
    fn replay_outputs_returns_last_system_return() {
        let events = vec![
            event("return", Some(json!({"answer": "first"}))),
            event("return", Some(json!({"answer": "final"}))),
        ];

        assert_eq!(replay_outputs(&events).unwrap(), json!({"answer": "final"}));
    }

    #[test]
    fn trace_redaction_hides_sensitive_keys() {
        let event = event(
            "model_call",
            Some(json!({
                "api_key": "secret",
                "safe": "value"
            })),
        );

        let sanitized = sanitize_trace_event(&event, &TraceWriteOptions::redacted());

        assert_eq!(
            sanitized.output.unwrap(),
            json!({
                "api_key": "[AIR_REDACTED]",
                "safe": "value"
            })
        );
    }
}
