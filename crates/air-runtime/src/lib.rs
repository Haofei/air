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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRuntimeInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    pub permission_profile: String,
    pub mutates_workspace: bool,
    pub network_access: bool,
    pub accepts_empty_input: bool,
    pub parallel_safe: bool,
    pub output_policy: String,
}

impl ToolRuntimeInfo {
    pub fn generic(capability: Option<&str>) -> Self {
        Self {
            capability: capability.map(str::to_string),
            permission_profile: capability
                .map(permission_profile_from_capability)
                .unwrap_or("unknown")
                .to_string(),
            mutates_workspace: capability
                .is_some_and(|value| value.contains("write") || value.contains("edit")),
            network_access: capability.is_some_and(|value| value.contains("network")),
            accepts_empty_input: false,
            parallel_safe: false,
            output_policy: "compact_for_model".to_string(),
        }
    }
}

fn permission_profile_from_capability(capability: &str) -> &'static str {
    if capability.contains("write") || capability.contains("edit") {
        "workspace_write"
    } else if capability.contains("test") || capability.contains("exec") {
        "command_exec"
    } else if capability.contains("network") || capability.contains("web") {
        "network"
    } else if capability.contains("read") {
        "read_only"
    } else {
        "custom"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRuntimeInfo {
    pub provider: String,
    pub native_tool_calls: bool,
    pub strict_tool_schema: bool,
    pub parallel_tool_calls: bool,
    pub stable_stream_tool_indices: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_bytes: Option<usize>,
}

impl Default for ModelRuntimeInfo {
    fn default() -> Self {
        Self {
            provider: "unknown".to_string(),
            native_tool_calls: false,
            strict_tool_schema: false,
            parallel_tool_calls: false,
            stable_stream_tool_indices: true,
            max_context_bytes: None,
        }
    }
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

    fn tool_runtime_info(&self, name: &str) -> Option<ToolRuntimeInfo> {
        self.tool_capability(name)
            .map(|capability| ToolRuntimeInfo::generic(Some(capability)))
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

    fn model_runtime_info(&self, _name: &str) -> Option<ModelRuntimeInfo> {
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
