use air_core::{
    path_segments, validate_value_against_type, AirModule, Expr, InputSpec, RetryPolicy,
    StateAction, ToolErrorMode, TypeSpec, Workflow,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};
use thiserror::Error;

mod model_context;
pub use model_context::{
    compact_model_context_string, compact_model_context_value, take_last_within_bytes_value,
    truncate_middle_context_string,
};

pub type State = Map<String, Value>;
type ResolvedToolSelection = Result<(String, Value), RuntimeError>;
type ResolvedToolBatchItem = (Value, ResolvedToolSelection);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub outputs: State,
    pub trace: Vec<TraceEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub agent: String,
    pub step: u32,
    pub rule: String,
    pub action: String,

    #[serde(default)]
    pub input: Option<Value>,

    #[serde(default)]
    pub output: Option<Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,

    pub status: TraceStatus,

    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceWriteOptions {
    pub redact_sensitive: bool,
    pub max_string_chars: Option<usize>,
    pub max_event_bytes: Option<usize>,
}

impl Default for TraceWriteOptions {
    fn default() -> Self {
        Self::redacted()
    }
}

impl TraceWriteOptions {
    pub fn raw() -> Self {
        Self {
            redact_sensitive: false,
            max_string_chars: None,
            max_event_bytes: None,
        }
    }

    pub fn redacted() -> Self {
        Self {
            redact_sensitive: true,
            max_string_chars: Some(4096),
            max_event_bytes: Some(64 * 1024),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    Ok,
    Error,
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

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("air-runtime only supports state_machine workflow execution")]
    UnsupportedWorkflow,

    #[error("no state_machine rule matched at step {step} with phase {phase}")]
    NoMatchingRule { step: u32, phase: String },

    #[error("state_machine exceeded max_steps={0}")]
    StepLimitExceeded(u32),

    #[error("missing input or state field {0}")]
    MissingField(String),

    #[error("unsupported condition {0}")]
    UnsupportedCondition(String),

    #[error("unsupported expression {0}")]
    UnsupportedExpression(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("policy.max_tool_calls exceeded: limit={limit} attempted={attempted}")]
    ToolCallLimitExceeded { limit: u32, attempted: u32 },

    #[error("tool_batch_dispatch max_calls exceeded: limit={limit} attempted={attempted}")]
    ToolBatchDispatchLimitExceeded { limit: u32, attempted: u32 },

    #[error("policy.max_model_calls exceeded: limit={limit} attempted={attempted}")]
    ModelCallLimitExceeded { limit: u32, attempted: u32 },

    #[error("policy.max_repeated_tool_calls exceeded for tool {tool}: limit={limit} attempted={attempted}")]
    RepeatedToolCallLimitExceeded {
        tool: String,
        limit: u32,
        attempted: u32,
    },

    #[error(
        "repeated context tool call requires repeat_reason for tool {tool}: attempted={attempted}"
    )]
    RepeatedContextToolCallRequiresReason { tool: String, attempted: u32 },

    #[error("{action} action exceeded timeout_seconds={timeout_seconds} elapsed_ms={elapsed_ms}")]
    ActionTimeoutExceeded {
        action: String,
        timeout_seconds: u64,
        elapsed_ms: u128,
    },

    #[error("policy.timeout_seconds exceeded: limit={limit} elapsed_ms={elapsed_ms}")]
    ModuleTimeoutExceeded { limit: u64, elapsed_ms: u128 },

    #[error("schema violation: {0}")]
    SchemaViolation(String),

    #[error("append target {0} is not an array")]
    AppendTargetNotArray(String),

    #[error("{action} action cannot write state.phase; use an explicit set action for state_machine transitions")]
    ControlFieldWrite { action: String },

    #[error("tool {tool} is not declared by module {module}")]
    UndeclaredTool { module: String, tool: String },

    #[error("tool {tool} provider capability {provider_capability} does not match module-declared capability {module_capability}")]
    ToolCapabilityMismatch {
        tool: String,
        provider_capability: String,
        module_capability: String,
    },

    #[error("tool {tool} requires capability {capability}, but module {module} does not declare it in requires.capabilities")]
    ToolCapabilityNotRequired {
        module: String,
        tool: String,
        capability: String,
    },

    #[error("approval required for module {module} capabilities {capabilities:?}, but no approval provider approved it")]
    ApprovalRequired {
        module: String,
        capabilities: Vec<String>,
    },

    #[error("approval denied for module {module} capabilities {capabilities:?}: {reason}")]
    ApprovalDenied {
        module: String,
        capabilities: Vec<String>,
        reason: String,
    },

    #[error("approval action in module {module} references capability {capability}, but policy.require_approval does not require it")]
    ApprovalCapabilityNotRequired { module: String, capability: String },

    #[error("trace IO error: {0}")]
    TraceIo(String),

    #[error("trace JSON error: {0}")]
    TraceJson(String),

    #[error("trace does not contain replayable output")]
    TraceMissingOutput,

    #[error("unknown citation id {citation} at {path}; known artifact ids: {known:?}")]
    UnknownCitation {
        path: String,
        citation: String,
        known: Vec<String>,
    },
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
        module: &AirModule,
        approval_for: &[String],
        _state: &Value,
    ) -> Result<ApprovalDecision, RuntimeError> {
        Err(RuntimeError::ApprovalRequired {
            module: module.agent.name.clone(),
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

pub fn system_return_event(outputs: State) -> TraceEvent {
    TraceEvent {
        agent: "$system".to_string(),
        step: 0,
        rule: "system".to_string(),
        action: "return".to_string(),
        input: None,
        output: Some(Value::Object(outputs)),
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    }
}

pub fn write_trace_jsonl(
    path: impl AsRef<Path>,
    events: &[TraceEvent],
) -> Result<(), RuntimeError> {
    write_trace_jsonl_with_options(path, events, &TraceWriteOptions::default())
}

pub fn write_trace_jsonl_with_options(
    path: impl AsRef<Path>,
    events: &[TraceEvent],
    options: &TraceWriteOptions,
) -> Result<(), RuntimeError> {
    let mut file = File::create(path).map_err(|error| RuntimeError::TraceIo(error.to_string()))?;
    for event in events {
        let event = sanitize_trace_event(event, options);
        let mut line = serde_json::to_string(&event)
            .map_err(|error| RuntimeError::TraceJson(error.to_string()))?;
        if let Some(max_bytes) = options.max_event_bytes {
            if line.len() > max_bytes {
                line = serde_json::to_string(&compact_trace_event(&event))
                    .map_err(|error| RuntimeError::TraceJson(error.to_string()))?;
            }
        }
        writeln!(file, "{line}").map_err(|error| RuntimeError::TraceIo(error.to_string()))?;
    }
    Ok(())
}

pub fn read_trace_jsonl(path: impl AsRef<Path>) -> Result<Vec<TraceEvent>, RuntimeError> {
    let file = File::open(path).map_err(|error| RuntimeError::TraceIo(error.to_string()))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for line in reader.lines() {
        let line = line.map_err(|error| RuntimeError::TraceIo(error.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str(&line)
            .map_err(|error| RuntimeError::TraceJson(error.to_string()))?;
        events.push(event);
    }

    Ok(events)
}

pub fn replay_outputs(events: &[TraceEvent]) -> Result<Value, RuntimeError> {
    events
        .iter()
        .rev()
        .find(|event| {
            event.status == TraceStatus::Ok
                && event.action == "return"
                && (event.agent == "$system" || event.output.is_some())
        })
        .and_then(|event| event.output.clone())
        .ok_or(RuntimeError::TraceMissingOutput)
}

pub fn sanitize_trace_event(event: &TraceEvent, options: &TraceWriteOptions) -> TraceEvent {
    let mut sanitized = event.clone();
    sanitized.input = sanitized
        .input
        .as_ref()
        .map(|value| sanitize_trace_value(value, options));
    sanitized.output = sanitized
        .output
        .as_ref()
        .map(|value| sanitize_trace_value(value, options));
    sanitized.meta = sanitized
        .meta
        .as_ref()
        .map(|value| sanitize_trace_value(value, options));
    if let Some(error) = &sanitized.error {
        sanitized.error = Some(sanitize_trace_text(error, options));
    }
    sanitized
}

fn sanitize_trace_value(value: &Value, options: &TraceWriteOptions) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let value = if options.redact_sensitive && is_sensitive_key(key) {
                        Value::String("[AIR_REDACTED]".to_string())
                    } else {
                        sanitize_trace_value(value, options)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| sanitize_trace_value(value, options))
                .collect(),
        ),
        Value::String(text) => Value::String(sanitize_trace_text(text, options)),
        _ => value.clone(),
    }
}

pub fn sanitize_trace_text(text: &str, options: &TraceWriteOptions) -> String {
    let mut text = if options.redact_sensitive {
        mask_sensitive_text(text)
    } else {
        text.to_string()
    };
    if let Some(max_chars) = options.max_string_chars {
        if text.chars().count() > max_chars {
            let truncated = truncate_middle_context_string(&text, max_chars, "AIR_TRUNCATED");
            text = if truncated.contains("[AIR_TRUNCATED]") {
                truncated
            } else {
                "[AIR_TRUNCATED]".to_string()
            };
        }
    }
    text
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "apikey"
            | "apiaccesskey"
            | "authorization"
            | "cookie"
            | "setcookie"
            | "password"
            | "passwd"
            | "pwd"
            | "secret"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "clientsecret"
    )
}

fn mask_sensitive_text(text: &str) -> String {
    let mut output = text.to_string();
    for marker in [
        "Bearer ",
        "authorization: ",
        "Authorization: ",
        "api_key=",
        "api-key=",
        "apikey=",
        "password=",
        "secret=",
        "token=",
    ] {
        output = mask_after_marker(&output, marker);
    }
    output
}

fn mask_after_marker(text: &str, marker: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        output.push_str(&rest[..index + marker.len()]);
        output.push_str("[AIR_REDACTED]");
        let value_start = index + marker.len();
        let value = &rest[value_start..];
        let value_end = value
            .find(|ch: char| ch.is_whitespace() || ch == '&' || ch == '"' || ch == '\'')
            .unwrap_or(value.len());
        rest = &value[value_end..];
    }
    output.push_str(rest);
    output
}

fn compact_trace_event(event: &TraceEvent) -> TraceEvent {
    let mut compact = event.clone();
    compact.input = compact.input.as_ref().map(compact_trace_payload);
    compact.output = compact.output.as_ref().map(compact_trace_payload);
    compact.meta = compact.meta.as_ref().map(compact_trace_meta);
    compact.error = compact
        .error
        .as_ref()
        .map(|_| "[AIR_TRUNCATED]".to_string());
    compact
}

fn compact_trace_payload(payload: &Value) -> Value {
    let Value::Object(object) = payload else {
        return json!({"_air_truncated": true});
    };
    let mut compact = Map::new();
    compact.insert("_air_truncated".to_string(), Value::Bool(true));
    for (key, value) in object {
        let Some(value) = compact_trace_meta_value(value) else {
            compact.insert(key.clone(), json!({"_air_truncated": true}));
            continue;
        };
        compact.insert(key.clone(), value);
    }
    Value::Object(compact)
}

fn compact_trace_meta(meta: &Value) -> Value {
    let Value::Object(object) = meta else {
        return json!({"_air_truncated": true});
    };
    let mut compact = Map::new();
    compact.insert("_air_truncated".to_string(), Value::Bool(true));
    for (key, value) in object {
        let Some(value) = compact_trace_meta_value(value) else {
            continue;
        };
        compact.insert(key.clone(), value);
    }
    Value::Object(compact)
}

fn compact_trace_meta_value(value: &Value) -> Option<Value> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Some(value.clone()),
        Value::Array(values) => {
            let mut compact = Vec::new();
            for value in values.iter().take(32) {
                let Some(value) = compact_trace_meta_value(value) else {
                    return Some(json!({"_air_truncated": true}));
                };
                compact.push(value);
            }
            if values.len() > compact.len() {
                compact.push(json!({"_air_truncated": true}));
            }
            Some(Value::Array(compact))
        }
        Value::Object(_) => None,
    }
}

pub struct Vm<T, M> {
    pub tools: T,
    pub models: M,
}

impl<T, M> Vm<T, M>
where
    T: ToolProvider,
    M: ModelProvider,
{
    pub fn run(&mut self, module: &AirModule, inputs: State) -> Result<RunResult, RuntimeError> {
        self.run_with_observer(module, inputs, |_| {})
    }

    pub fn run_with_observer<F>(
        &mut self,
        module: &AirModule,
        inputs: State,
        mut observer: F,
    ) -> Result<RunResult, RuntimeError>
    where
        F: FnMut(&TraceEvent),
    {
        let Workflow::StateMachine(workflow) = &module.workflow else {
            return Err(RuntimeError::UnsupportedWorkflow);
        };

        let mut state = inputs;
        state.insert("phase".to_string(), Value::String(workflow.initial.clone()));
        let mut outputs = State::new();
        let mut trace = Vec::new();
        let mut model_calls = 0u32;
        let mut tool_calls = 0u32;
        let mut tool_history = Vec::new();
        let mut artifact_registry = collect_initial_artifact_ids(&Value::Object(state.clone()));
        let module_started_at = Instant::now();

        for step in 0..workflow.max_steps {
            state.insert(
                "_air".to_string(),
                runtime_context(step, workflow.max_steps, model_calls, tool_calls),
            );
            let phase = state
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            let mut matched = false;
            for rule in &workflow.rules {
                if condition_matches(&state, &outputs, &rule.when)? {
                    matched = true;
                    let mut did_return = false;
                    for action in &rule.actions {
                        if did_return && !matches!(action, StateAction::Return { .. }) {
                            continue;
                        }
                        let mut context = ExecutionContext {
                            module,
                            state: &mut state,
                            outputs: &mut outputs,
                            trace: &mut trace,
                            observer: &mut observer,
                            model_calls: &mut model_calls,
                            tool_calls: &mut tool_calls,
                            tool_history: &mut tool_history,
                            artifact_registry: &mut artifact_registry,
                            module_started_at,
                            step,
                            rule: &rule.id,
                        };
                        self.execute_action(&mut context, action)?;
                        if matches!(action, StateAction::Return { .. }) {
                            did_return = true;
                        }
                    }
                    if did_return {
                        return Ok(RunResult { outputs, trace });
                    }
                    if let Some(return_rule) = workflow.rules.iter().find(|candidate| {
                        candidate.id != rule.id
                            && candidate
                                .actions
                                .iter()
                                .any(|action| matches!(action, StateAction::Return { .. }))
                            && condition_matches(&state, &outputs, &candidate.when).unwrap_or(false)
                    }) {
                        for action in &return_rule.actions {
                            if !matches!(action, StateAction::Return { .. }) {
                                continue;
                            }
                            let mut context = ExecutionContext {
                                module,
                                state: &mut state,
                                outputs: &mut outputs,
                                trace: &mut trace,
                                observer: &mut observer,
                                model_calls: &mut model_calls,
                                tool_calls: &mut tool_calls,
                                tool_history: &mut tool_history,
                                artifact_registry: &mut artifact_registry,
                                module_started_at,
                                step,
                                rule: &return_rule.id,
                            };
                            self.execute_action(&mut context, action)?;
                        }
                        return Ok(RunResult { outputs, trace });
                    }
                    break;
                }
            }

            if !matched {
                return Err(RuntimeError::NoMatchingRule { step, phase });
            }
        }

        Err(RuntimeError::StepLimitExceeded(workflow.max_steps))
    }

    fn execute_action(
        &mut self,
        context: &mut ExecutionContext<'_>,
        action: &StateAction,
    ) -> Result<(), RuntimeError> {
        check_module_timeout(context)?;
        match action {
            StateAction::Set { values } => {
                let mut resolved_values = Map::new();
                for (key, value) in values {
                    let resolved = resolve_set_value(context.state, context.outputs, value)?;
                    if let Err(error) = validate_state_value(context.module, key, &resolved) {
                        context.push_event(
                            "set",
                            None,
                            Some(Value::Object(resolved_values.clone())),
                            Err(error.to_string()),
                        );
                        return Err(error);
                    }
                    resolved_values.insert(key.clone(), resolved.clone());
                    context.state.insert(key.clone(), resolved);
                }
                context.push_event("set", None, Some(Value::Object(resolved_values)), Ok(()));
            }
            StateAction::Append { target, value } => {
                reject_control_field_write("append", target)?;
                let value = resolve_input(context.state, context.outputs, value)?;
                let mut values = context
                    .state
                    .get(target)
                    .map(|existing| {
                        existing
                            .as_array()
                            .cloned()
                            .ok_or_else(|| RuntimeError::AppendTargetNotArray(target.clone()))
                    })
                    .transpose()?
                    .unwrap_or_default();
                values.push(value.clone());
                let next = Value::Array(values);
                if let Err(error) = validate_output(context.module, target, &next) {
                    context.push_event("append", Some(value), Some(next), Err(error.to_string()));
                    return Err(error);
                }
                context.state.insert(target.clone(), next);
                context.push_event(
                    "append",
                    Some(value),
                    context.state.get(target).cloned(),
                    Ok(()),
                );
            }
            StateAction::ModelCall {
                model,
                input,
                output,
                timeout_seconds,
                retry,
                ..
            } => {
                reject_control_field_write("model_call", output)?;
                let input = resolve_input(context.state, context.outputs, input)?;
                let schema = output_spec(context.module, output);
                let max_attempts = retry
                    .as_ref()
                    .map(|retry| retry.max_attempts)
                    .unwrap_or(1)
                    .max(1);
                let mut retry_error: Option<RetryError> = None;
                for attempt in 1..=max_attempts {
                    let attempt_input = model_input_for_attempt(
                        &input,
                        attempt,
                        retry_error.as_ref(),
                        schema,
                        retry.as_ref(),
                    );
                    let attempt_input_bytes = json_value_size_bytes(&attempt_input);
                    let model_meta_args = ModelCallMetaArgs {
                        model,
                        attempt,
                        max_attempts,
                        output,
                        timeout_seconds: *timeout_seconds,
                        input_bytes: attempt_input_bytes,
                    };
                    if let Some(limit) = context.module.policy.max_model_calls {
                        let attempted = *context.model_calls + 1;
                        if attempted > limit {
                            let error = RuntimeError::ModelCallLimitExceeded { limit, attempted };
                            context.push_event_with_meta(
                                "model_call",
                                Some(attempt_input),
                                None,
                                Some(model_call_result_meta(&model_meta_args, 0, false)),
                                Err(error.to_string()),
                            );
                            return Err(error);
                        }
                    }
                    *context.model_calls += 1;
                    context.push_event_with_meta(
                        "model_call_start",
                        Some(attempt_input.clone()),
                        None,
                        Some(model_call_meta(&model_meta_args)),
                        Ok(()),
                    );
                    let attempt_started_at = Instant::now();
                    let result = match self.models.call_model_with_timeout(
                        model,
                        &attempt_input,
                        Duration::from_secs(*timeout_seconds),
                    ) {
                        Ok(result) => {
                            let provider_stats = self.models.take_last_request_stats();
                            (result, provider_stats)
                        }
                        Err(error) => {
                            let provider_stats = self.models.take_last_request_stats();
                            let is_final_attempt = attempt == max_attempts;
                            let error_message = error.to_string();
                            context.push_event_with_meta(
                                "model_call",
                                Some(attempt_input),
                                None,
                                Some(model_call_result_meta_with_provider(
                                    &model_meta_args,
                                    attempt_started_at.elapsed().as_millis(),
                                    !is_final_attempt,
                                    provider_stats.as_ref(),
                                )),
                                Err(error_message.clone()),
                            );
                            if is_final_attempt {
                                return Err(error);
                            }
                            retry_error = Some(RetryError::from_runtime_error(&error));
                            continue;
                        }
                    };
                    if let Some(error) =
                        action_timeout_error("model_call", *timeout_seconds, attempt_started_at)
                    {
                        let (result, provider_stats) = result;
                        let is_final_attempt = attempt == max_attempts;
                        let error_message = error.to_string();
                        context.push_event_with_meta(
                            "model_call",
                            Some(attempt_input),
                            Some(result),
                            Some(model_call_result_meta_with_provider(
                                &model_meta_args,
                                attempt_started_at.elapsed().as_millis(),
                                !is_final_attempt,
                                provider_stats.as_ref(),
                            )),
                            Err(error_message.clone()),
                        );
                        if is_final_attempt {
                            return Err(error);
                        }
                        retry_error = Some(RetryError::Message(error_message));
                        continue;
                    }
                    let (result, provider_stats) = result;
                    let result =
                        match model_result_for_output_schema(context.module, output, result) {
                            Ok(result) => result,
                            Err(error) => {
                                let is_final_attempt = attempt == max_attempts;
                                let error_message = error.error.to_string();
                                context.push_event_with_meta(
                                    "model_call",
                                    Some(attempt_input),
                                    Some(error.value),
                                    Some(model_call_result_meta_with_provider(
                                        &model_meta_args,
                                        attempt_started_at.elapsed().as_millis(),
                                        !is_final_attempt,
                                        provider_stats.as_ref(),
                                    )),
                                    Err(error_message.clone()),
                                );
                                if is_final_attempt {
                                    return Err(error.error);
                                }
                                retry_error = Some(RetryError::Message(error_message));
                                continue;
                            }
                        };
                    if let Err(error) = validate_citations_against_registry(
                        output,
                        &result,
                        context.artifact_registry,
                    ) {
                        let is_final_attempt = attempt == max_attempts;
                        let error_message = error.to_string();
                        context.push_event_with_meta(
                            "model_call",
                            Some(attempt_input),
                            Some(result),
                            Some(model_call_result_meta_with_provider(
                                &model_meta_args,
                                attempt_started_at.elapsed().as_millis(),
                                !is_final_attempt,
                                provider_stats.as_ref(),
                            )),
                            Err(error_message.clone()),
                        );
                        if is_final_attempt {
                            return Err(error);
                        }
                        retry_error = Some(RetryError::Message(error_message));
                        continue;
                    }
                    context.state.insert(output.clone(), result);
                    context.push_event_with_meta(
                        "model_call",
                        Some(attempt_input),
                        context.state.get(output).cloned(),
                        Some(model_call_result_meta_with_provider(
                            &model_meta_args,
                            attempt_started_at.elapsed().as_millis(),
                            false,
                            provider_stats.as_ref(),
                        )),
                        Ok(()),
                    );
                    return Ok(());
                }
            }
            StateAction::ToolCall {
                tool,
                input,
                output,
                timeout_seconds,
                retry,
                ..
            } => {
                let input = resolve_input(context.state, context.outputs, input)?;
                self.execute_tool_call(
                    context,
                    ToolExecution {
                        action_name: "tool_call",
                        tool,
                        requested_tool: None,
                        input,
                        output,
                        timeout_seconds: *timeout_seconds,
                        retry,
                    },
                )?;
            }
            StateAction::ToolBatchDispatch {
                input,
                output,
                timeout_seconds,
                max_calls,
                retry,
                on_error,
            } => {
                let batch = resolve_input(context.state, context.outputs, input)?;
                self.execute_tool_batch_dispatch(
                    context,
                    ToolBatchExecution {
                        input: batch,
                        output,
                        timeout_seconds: *timeout_seconds,
                        max_calls: *max_calls,
                        retry,
                        on_error: *on_error,
                    },
                )?;
            }
            StateAction::Approval { approval_for } => {
                if let Err(error) = validate_approval_capabilities(context.module, approval_for) {
                    context.push_event_with_meta(
                        "approval",
                        Some(Value::Array(
                            approval_for.iter().cloned().map(Value::String).collect(),
                        )),
                        None,
                        Some(json!({"approval_for": approval_for})),
                        Err(error.to_string()),
                    );
                    return Err(error);
                }

                let state_snapshot = Value::Object(context.state.clone());
                let decision =
                    match self
                        .tools
                        .request_approval(context.module, approval_for, &state_snapshot)
                    {
                        Ok(decision) => decision,
                        Err(error) => {
                            context.push_event_with_meta(
                                "approval",
                                Some(Value::Array(
                                    approval_for.iter().cloned().map(Value::String).collect(),
                                )),
                                None,
                                Some(json!({"approval_for": approval_for})),
                                Err(error.to_string()),
                            );
                            return Err(error);
                        }
                    };
                let output = serde_json::to_value(&decision).unwrap_or(Value::Null);
                let meta = approval_meta(approval_for, &decision);
                if !decision.approved {
                    let error = RuntimeError::ApprovalDenied {
                        module: context.module.agent.name.clone(),
                        capabilities: approval_for.clone(),
                        reason: decision
                            .reason
                            .clone()
                            .unwrap_or_else(|| "approval provider denied the request".to_string()),
                    };
                    context.push_event_with_meta(
                        "approval",
                        Some(Value::Array(
                            approval_for.iter().cloned().map(Value::String).collect(),
                        )),
                        Some(output),
                        Some(meta),
                        Err(error.to_string()),
                    );
                    return Err(error);
                }
                context.push_event_with_meta(
                    "approval",
                    Some(Value::Array(
                        approval_for.iter().cloned().map(Value::String).collect(),
                    )),
                    Some(output),
                    Some(meta),
                    Ok(()),
                );
            }
            StateAction::Return { output } => {
                let value = read_field(context.state, context.outputs, output)?.clone();
                if let Err(error) = validate_return_output(context.module, output, &value) {
                    context.push_event("return", None, Some(value), Err(error.to_string()));
                    return Err(error);
                }
                if let Err(error) =
                    validate_citations_against_registry(output, &value, context.artifact_registry)
                {
                    context.push_event("return", None, Some(value), Err(error.to_string()));
                    return Err(error);
                }
                context.outputs.insert(output.clone(), value);
                context.push_event("return", None, context.outputs.get(output).cloned(), Ok(()));
            }
        }

        Ok(())
    }

    fn execute_tool_call(
        &mut self,
        context: &mut ExecutionContext<'_>,
        call: ToolExecution<'_>,
    ) -> Result<(), RuntimeError> {
        let ToolExecution {
            action_name,
            tool,
            requested_tool,
            input,
            output,
            timeout_seconds,
            retry,
        } = call;
        reject_control_field_write(action_name, output)?;
        if let Err(error) = validate_tool_capability(context.module, tool, &self.tools) {
            context.push_event_with_meta(
                action_name,
                None,
                None,
                Some(tool_error_meta(tool, requested_tool)),
                Err(error.to_string()),
            );
            return Err(error);
        }
        enforce_repeated_tool_policy(context, action_name, tool, &input)?;
        let max_attempts = retry
            .as_ref()
            .map(|retry| retry.max_attempts)
            .unwrap_or(1)
            .max(1);
        for attempt in 1..=max_attempts {
            let meta_args = ToolCallMeta {
                tool,
                requested_tool,
                attempt,
                max_attempts,
                output,
                timeout_seconds,
            };
            if let Some(limit) = context.module.policy.max_tool_calls {
                let attempted = *context.tool_calls + 1;
                if attempted > limit {
                    let error = RuntimeError::ToolCallLimitExceeded { limit, attempted };
                    context.push_event_with_meta(
                        action_name,
                        Some(input),
                        None,
                        Some(tool_call_result_meta(meta_args, 0, false)),
                        Err(error.to_string()),
                    );
                    return Err(error);
                }
            }
            *context.tool_calls += 1;
            context.push_event_with_meta(
                &format!("{action_name}_start"),
                Some(input.clone()),
                None,
                Some(tool_call_meta(meta_args)),
                Ok(()),
            );
            let attempt_started_at = Instant::now();
            let result = match self.tools.call_tool_with_timeout(
                tool,
                &input,
                Duration::from_secs(timeout_seconds),
            ) {
                Ok(result) => result,
                Err(error) => {
                    let is_final_attempt = attempt == max_attempts;
                    context.push_event_with_meta(
                        action_name,
                        Some(input.clone()),
                        None,
                        Some(tool_call_result_meta(
                            meta_args,
                            attempt_started_at.elapsed().as_millis(),
                            !is_final_attempt,
                        )),
                        Err(error.to_string()),
                    );
                    if is_final_attempt {
                        return Err(error);
                    }
                    continue;
                }
            };
            if let Some(error) =
                action_timeout_error(action_name, timeout_seconds, attempt_started_at)
            {
                let is_final_attempt = attempt == max_attempts;
                context.push_event_with_meta(
                    action_name,
                    Some(input.clone()),
                    Some(result),
                    Some(tool_call_result_meta(
                        meta_args,
                        attempt_started_at.elapsed().as_millis(),
                        !is_final_attempt,
                    )),
                    Err(error.to_string()),
                );
                if is_final_attempt {
                    return Err(error);
                }
                continue;
            }
            if let Err(error) = validate_output(context.module, output, &result) {
                let is_final_attempt = attempt == max_attempts;
                context.push_event_with_meta(
                    action_name,
                    Some(input.clone()),
                    Some(result),
                    Some(tool_call_result_meta(
                        meta_args,
                        attempt_started_at.elapsed().as_millis(),
                        !is_final_attempt,
                    )),
                    Err(error.to_string()),
                );
                if is_final_attempt {
                    return Err(error);
                }
                continue;
            }
            let artifact_ids = register_artifacts(context.artifact_registry, &result);
            context.state.insert(output.to_string(), result);
            let mut meta =
                tool_call_result_meta(meta_args, attempt_started_at.elapsed().as_millis(), false);
            insert_artifact_result_meta(&mut meta, &artifact_ids);
            context.push_event_with_meta(
                action_name,
                Some(input),
                context.state.get(output).cloned(),
                Some(meta),
                Ok(()),
            );
            return Ok(());
        }
        Ok(())
    }

    fn execute_tool_batch_dispatch(
        &mut self,
        context: &mut ExecutionContext<'_>,
        batch: ToolBatchExecution<'_>,
    ) -> Result<(), RuntimeError> {
        let ToolBatchExecution {
            input,
            output,
            timeout_seconds,
            max_calls,
            retry,
            on_error,
        } = batch;
        reject_control_field_write("tool_batch_dispatch", output)?;
        let dispatch_items = resolve_tool_batch_dispatch_items(&input)?;
        let attempted = dispatch_items.len() as u32;
        if attempted > max_calls {
            let error = RuntimeError::ToolBatchDispatchLimitExceeded {
                limit: max_calls,
                attempted,
            };
            if on_error == ToolErrorMode::Observe {
                observe_tool_batch_dispatch_error(
                    context,
                    input,
                    output,
                    attempted,
                    max_calls,
                    &error.to_string(),
                )?;
                return Ok(());
            }
            context.push_event_with_meta(
                "tool_batch_dispatch",
                Some(input),
                None,
                Some(json!({"max_calls": max_calls, "attempted": attempted})),
                Err(error.to_string()),
            );
            return Err(error);
        }
        let max_attempts = retry
            .as_ref()
            .map(|retry| retry.max_attempts)
            .unwrap_or(1)
            .max(1);
        let mut results = Vec::new();
        for (index, (raw_dispatch, dispatch)) in dispatch_items.into_iter().enumerate() {
            let (tool, tool_input, requested_tool) = match dispatch {
                Ok((tool, input)) => {
                    let (tool, requested_tool) =
                        normalize_model_selected_tool_name(context.module, tool);
                    (tool, input, requested_tool)
                }
                Err(error) => {
                    context.push_event_with_meta(
                        "tool_batch_dispatch_item",
                        Some(raw_dispatch.clone()),
                        None,
                        Some(json!({"tool": invalid_dispatch_tool_name(&raw_dispatch), "index": index})),
                        Err(error.to_string()),
                    );
                    if on_error == ToolErrorMode::Observe {
                        results.push(tool_batch_malformed_observation(
                            &raw_dispatch,
                            &error.to_string(),
                        ));
                        continue;
                    }
                    return Err(error);
                }
            };
            if let Err(error) = validate_tool_capability(context.module, &tool, &self.tools) {
                let mut meta = tool_error_meta(&tool, requested_tool.as_deref());
                insert_batch_item_meta(&mut meta, index);
                context.push_event_with_meta(
                    "tool_batch_dispatch_item",
                    Some(tool_input.clone()),
                    None,
                    Some(meta),
                    Err(error.to_string()),
                );
                if on_error == ToolErrorMode::Observe
                    && matches!(error, RuntimeError::UndeclaredTool { .. })
                {
                    results.push(tool_batch_error_observation(
                        &tool,
                        &tool_input,
                        &error.to_string(),
                    ));
                    continue;
                }
                return Err(error);
            }
            if let Err(error) = enforce_repeated_tool_policy(
                context,
                "tool_batch_dispatch_item",
                &tool,
                &tool_input,
            ) {
                if on_error == ToolErrorMode::Observe {
                    results.push(tool_batch_error_observation_for_runtime_error(
                        &tool,
                        &tool_input,
                        &error,
                    ));
                    continue;
                }
                return Err(error);
            }
            let mut item_output = None;
            for attempt in 1..=max_attempts {
                let meta_args = ToolCallMeta {
                    tool: &tool,
                    requested_tool: requested_tool.as_deref(),
                    attempt,
                    max_attempts,
                    output,
                    timeout_seconds,
                };
                if let Some(limit) = context.module.policy.max_tool_calls {
                    let attempted = *context.tool_calls + 1;
                    if attempted > limit {
                        let error = RuntimeError::ToolCallLimitExceeded { limit, attempted };
                        let mut meta = tool_call_result_meta(meta_args, 0, false);
                        insert_batch_item_meta(&mut meta, index);
                        context.push_event_with_meta(
                            "tool_batch_dispatch_item",
                            Some(tool_input.clone()),
                            None,
                            Some(meta),
                            Err(error.to_string()),
                        );
                        return Err(error);
                    }
                }
                *context.tool_calls += 1;
                let mut start_meta = tool_call_meta(meta_args);
                insert_batch_item_meta(&mut start_meta, index);
                context.push_event_with_meta(
                    "tool_batch_dispatch_item_start",
                    Some(tool_input.clone()),
                    None,
                    Some(start_meta),
                    Ok(()),
                );
                let attempt_started_at = Instant::now();
                let result = match self.tools.call_tool_with_timeout(
                    &tool,
                    &tool_input,
                    Duration::from_secs(timeout_seconds),
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        let is_final_attempt = attempt == max_attempts;
                        let mut meta = tool_call_result_meta(
                            meta_args,
                            attempt_started_at.elapsed().as_millis(),
                            !is_final_attempt,
                        );
                        insert_batch_item_meta(&mut meta, index);
                        context.push_event_with_meta(
                            "tool_batch_dispatch_item",
                            Some(tool_input.clone()),
                            None,
                            Some(meta),
                            Err(error.to_string()),
                        );
                        if is_final_attempt {
                            if on_error == ToolErrorMode::Observe {
                                item_output = Some(tool_batch_error_observation(
                                    &tool,
                                    &tool_input,
                                    &error.to_string(),
                                ));
                                break;
                            }
                            return Err(error);
                        }
                        continue;
                    }
                };
                if let Some(error) = action_timeout_error(
                    "tool_batch_dispatch_item",
                    timeout_seconds,
                    attempt_started_at,
                ) {
                    let is_final_attempt = attempt == max_attempts;
                    let mut meta = tool_call_result_meta(
                        meta_args,
                        attempt_started_at.elapsed().as_millis(),
                        !is_final_attempt,
                    );
                    insert_batch_item_meta(&mut meta, index);
                    context.push_event_with_meta(
                        "tool_batch_dispatch_item",
                        Some(tool_input.clone()),
                        Some(result),
                        Some(meta),
                        Err(error.to_string()),
                    );
                    if is_final_attempt {
                        if on_error == ToolErrorMode::Observe {
                            item_output = Some(tool_batch_error_observation(
                                &tool,
                                &tool_input,
                                &error.to_string(),
                            ));
                            break;
                        }
                        return Err(error);
                    }
                    continue;
                }

                let artifact_ids = register_artifacts(context.artifact_registry, &result);
                let mut meta = tool_call_result_meta(
                    meta_args,
                    attempt_started_at.elapsed().as_millis(),
                    false,
                );
                insert_batch_item_meta(&mut meta, index);
                insert_artifact_result_meta(&mut meta, &artifact_ids);
                context.push_event_with_meta(
                    "tool_batch_dispatch_item",
                    Some(tool_input.clone()),
                    Some(result.clone()),
                    Some(meta),
                    Ok(()),
                );
                item_output = Some(json!({
                    "tool": tool,
                    "input": tool_input,
                    "status": "ok",
                    "error_code": null,
                    "output": result,
                }));
                if let Some(output_object) = item_output.as_mut().and_then(Value::as_object_mut) {
                    copy_tool_selection_metadata(&raw_dispatch, output_object);
                }
                break;
            }
            if let Some(item_output) = item_output {
                results.push(item_output);
            }
        }

        let summary = tool_batch_summary(&results);
        let output_value = Value::Array(results);
        if let Err(error) = validate_output(context.module, output, &output_value) {
            context.push_event_with_meta(
                "tool_batch_dispatch",
                Some(input),
                Some(output_value),
                Some(json!({"count": attempted, "max_calls": max_calls})),
                Err(error.to_string()),
            );
            return Err(error);
        }
        context.state.insert(output.to_string(), output_value);
        context
            .state
            .insert(format!("{output}_summary"), summary.clone());
        context.push_event_with_meta(
            "tool_batch_dispatch",
            Some(input),
            context.state.get(output).cloned(),
            Some(json!({
                "count": attempted,
                "max_calls": max_calls,
                "summary": summary,
                "error_count": context
                    .state
                    .get(output)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|item| item.get("status").and_then(Value::as_str) == Some("error"))
                    .count()
            })),
            Ok(()),
        );
        Ok(())
    }
}

fn validate_output(module: &AirModule, output: &str, value: &Value) -> Result<(), RuntimeError> {
    validate_named_value(output, value, output_spec(module, output))
}

fn observe_tool_batch_dispatch_error(
    context: &mut ExecutionContext<'_>,
    input: Value,
    output: &str,
    attempted: u32,
    max_calls: u32,
    error: &str,
) -> Result<(), RuntimeError> {
    let output_value = Value::Array(vec![tool_batch_level_error_observation(&input, error)]);
    if let Err(error) = validate_output(context.module, output, &output_value) {
        context.push_event_with_meta(
            "tool_batch_dispatch",
            Some(input),
            Some(output_value),
            Some(json!({"count": attempted, "max_calls": max_calls, "error_count": 1})),
            Err(error.to_string()),
        );
        return Err(error);
    }
    let summary = output_value
        .as_array()
        .map(|results| tool_batch_summary(results))
        .unwrap_or_else(|| tool_batch_summary(&[]));
    context.state.insert(output.to_string(), output_value);
    context
        .state
        .insert(format!("{output}_summary"), summary.clone());
    context.push_event_with_meta(
        "tool_batch_dispatch",
        Some(input),
        context.state.get(output).cloned(),
        Some(json!({
            "count": attempted,
            "max_calls": max_calls,
            "error_count": 1,
            "summary": summary
        })),
        Ok(()),
    );
    Ok(())
}

fn tool_batch_summary(results: &[Value]) -> Value {
    let mut any_workspace_change = false;
    let mut any_verification_passed = false;
    let mut any_verification_failed = false;
    let mut verification_status_update = "unchanged";
    let mut workspace_change_tools = Vec::new();
    let mut verification_tools = Vec::new();
    let mut error_count = 0usize;

    for item in results {
        if item.get("status").and_then(Value::as_str) == Some("error") {
            error_count += 1;
        }
        let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
        let output = item.get("output").unwrap_or(&Value::Null);

        if output
            .get("workspace_changed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            any_workspace_change = true;
            workspace_change_tools.push(Value::String(tool.to_string()));
            verification_status_update = "unknown";
        }

        if output
            .get("verification")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            verification_tools.push(Value::String(tool.to_string()));
            if output
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                any_verification_passed = true;
                verification_status_update = "passed";
            } else {
                any_verification_failed = true;
                verification_status_update = "failed";
            }
        }
    }

    json!({
        "tool_count": results.len(),
        "error_count": error_count,
        "any_workspace_change": any_workspace_change,
        "workspace_change_tools": workspace_change_tools,
        "any_verification_passed": any_verification_passed,
        "any_verification_failed": any_verification_failed,
        "verification_tools": verification_tools,
        "verification_status_update": verification_status_update
    })
}

struct ModelOutputSchemaError {
    value: Value,
    error: RuntimeError,
}

fn model_result_for_output_schema(
    module: &AirModule,
    output: &str,
    value: Value,
) -> Result<Value, ModelOutputSchemaError> {
    match validate_output(module, output, &value) {
        Ok(()) => Ok(value),
        Err(error) => {
            for parsed in model_output_schema_candidates(&value).into_iter().rev() {
                if validate_output(module, output, &parsed).is_ok() {
                    return Ok(parsed);
                }
            }
            let error = provider_content_wrapper_schema_error(&value, error);
            Err(ModelOutputSchemaError { value, error })
        }
    }
}

fn model_output_schema_candidates(value: &Value) -> Vec<Value> {
    let mut candidates = model_content_json_candidates(value);
    candidates.extend(single_field_model_wrapper_candidates(value));
    candidates
}

fn single_field_model_wrapper_candidates(value: &Value) -> Vec<Value> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    if object.len() != 1 {
        return Vec::new();
    }
    let Some((key, wrapped)) = object.iter().next() else {
        return Vec::new();
    };
    if !matches!(key.as_str(), "answer" | "result" | "output") {
        return Vec::new();
    }
    if wrapped.is_string() {
        let wrapper = json!({"content": wrapped});
        model_content_json_candidates(&wrapper)
    } else {
        vec![wrapped.clone()]
    }
}

fn provider_content_wrapper_schema_error(value: &Value, error: RuntimeError) -> RuntimeError {
    let Some(content) = value
        .as_object()
        .and_then(|object| object.get("content"))
        .and_then(Value::as_str)
    else {
        return error;
    };
    let preview = content.chars().take(160).collect::<String>();
    RuntimeError::SchemaViolation(format!(
        "{error}; invalid structured model output: content wrappers are not part of the declared AIR output interface. Return the required JSON object directly. If more information is needed, encode that need using the declared fields instead of prose. content_preview={preview:?}"
    ))
}

fn model_content_json_candidates(value: &Value) -> Vec<Value> {
    let Some(content) = value
        .as_object()
        .and_then(|object| object.get("content"))
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    let normalized = strip_model_content_code_fence(content).unwrap_or(content);
    let mut values = Vec::new();
    if let Ok(value) = serde_json::from_str(normalized) {
        push_unique_json_candidate(&mut values, value);
    }
    for slice in json_object_slices(normalized) {
        if let Ok(value) = serde_json::from_str(slice) {
            push_unique_json_candidate(&mut values, value);
        }
    }
    values
}

fn push_unique_json_candidate(values: &mut Vec<Value>, value: Value) {
    if !values.iter().any(|candidate| candidate == &value) {
        values.push(value);
    }
}

fn json_object_slices(input: &str) -> Vec<&str> {
    let mut slices = Vec::new();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, character) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start_index) = start.take() {
                        slices.push(&input[start_index..index + character.len_utf8()]);
                    }
                }
            }
            _ => {}
        }
    }

    slices
}

fn strip_model_content_code_fence(content: &str) -> Option<&str> {
    let trimmed = content.trim();
    let after_open = trimmed.strip_prefix("```")?;
    let close_index = after_open.rfind("```")?;
    let inner = &after_open[..close_index];
    let inner = inner.trim_start();
    let inner = match inner.find('\n') {
        Some(index) => {
            let language = inner[..index].trim();
            if language.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            }) {
                &inner[index + 1..]
            } else {
                inner
            }
        }
        None => inner,
    };
    Some(inner.trim())
}

fn reject_control_field_write(action: &str, field: &str) -> Result<(), RuntimeError> {
    if field == "phase" {
        return Err(RuntimeError::ControlFieldWrite {
            action: action.to_string(),
        });
    }
    Ok(())
}

fn validate_state_value(
    module: &AirModule,
    field: &str,
    value: &Value,
) -> Result<(), RuntimeError> {
    validate_named_value(field, value, output_spec(module, field))
}

fn validate_return_output(
    module: &AirModule,
    output: &str,
    value: &Value,
) -> Result<(), RuntimeError> {
    validate_named_value(output, value, module.outputs.get(output))
}

fn validate_named_value(
    name: &str,
    value: &Value,
    spec: Option<&TypeSpec>,
) -> Result<(), RuntimeError> {
    let Some(spec) = spec else {
        return Ok(());
    };
    let errors = validate_value_against_type(name, value, spec);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(RuntimeError::SchemaViolation(errors.join("; ")))
    }
}

fn collect_initial_artifact_ids(value: &Value) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    collect_explicit_artifact_ids(value, &mut ids);
    collect_citation_ids(value, &mut ids);
    ids
}

fn register_artifacts(registry: &mut BTreeSet<String>, value: &Value) -> Vec<String> {
    let mut discovered = BTreeSet::new();
    collect_explicit_artifact_ids(value, &mut discovered);
    discovered
        .into_iter()
        .filter(|id| registry.insert(id.clone()))
        .collect()
}

fn collect_explicit_artifact_ids(value: &Value, ids: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            collect_artifact_array_ids(object.get("artifacts"), ids);
            collect_artifact_array_ids(object.get("documents"), ids);
            for value in object.values() {
                collect_explicit_artifact_ids(value, ids);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_explicit_artifact_ids(value, ids);
            }
        }
        _ => {}
    }
}

fn collect_artifact_array_ids(value: Option<&Value>, ids: &mut BTreeSet<String>) {
    let Some(Value::Array(items)) = value else {
        return;
    };
    for item in items {
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            if !id.trim().is_empty() {
                ids.insert(id.to_string());
            }
        }
    }
}

fn collect_citation_ids(value: &Value, ids: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if is_citation_field(key) {
                    collect_string_array_items(value, ids);
                }
                collect_citation_ids(value, ids);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_citation_ids(value, ids);
            }
        }
        _ => {}
    }
}

fn validate_citations_against_registry(
    root_path: &str,
    value: &Value,
    registry: &BTreeSet<String>,
) -> Result<(), RuntimeError> {
    if registry.is_empty() {
        return Ok(());
    }
    validate_citations_at_path(root_path, value, registry)
}

fn validate_citations_at_path(
    path: &str,
    value: &Value,
    registry: &BTreeSet<String>,
) -> Result<(), RuntimeError> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let child_path = format!("{path}.{key}");
                if is_citation_field(key) {
                    validate_citation_value(&child_path, value, registry)?;
                }
                validate_citations_at_path(&child_path, value, registry)?;
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_citations_at_path(&format!("{path}[{index}]"), value, registry)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_citation_value(
    path: &str,
    value: &Value,
    registry: &BTreeSet<String>,
) -> Result<(), RuntimeError> {
    let Value::Array(items) = value else {
        return Ok(());
    };
    for (index, item) in items.iter().enumerate() {
        let Some(citation) = item.as_str() else {
            continue;
        };
        if citation.trim().is_empty() || registry.contains(citation) {
            continue;
        }
        return Err(RuntimeError::UnknownCitation {
            path: format!("{path}[{index}]"),
            citation: citation.to_string(),
            known: registry.iter().take(50).cloned().collect(),
        });
    }
    Ok(())
}

fn collect_string_array_items(value: &Value, ids: &mut BTreeSet<String>) {
    let Value::Array(items) = value else {
        return;
    };
    for item in items {
        if let Some(id) = item.as_str() {
            if !id.trim().is_empty() {
                ids.insert(id.to_string());
            }
        }
    }
}

fn is_citation_field(field: &str) -> bool {
    matches!(field, "sources" | "citations" | "source_ids")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RetryError {
    Message(String),
    Provider(String),
    TokenLimit(String),
}

impl RetryError {
    fn from_runtime_error(error: &RuntimeError) -> Self {
        match error {
            RuntimeError::Provider(message) if is_token_limit_error(message) => {
                RetryError::TokenLimit(message.clone())
            }
            RuntimeError::Provider(message) => RetryError::Provider(message.clone()),
            other => RetryError::Message(other.to_string()),
        }
    }

    fn message(&self) -> &str {
        match self {
            RetryError::Message(message)
            | RetryError::Provider(message)
            | RetryError::TokenLimit(message) => message,
        }
    }

    fn reason(&self) -> &'static str {
        match self {
            RetryError::Message(_) => "previous_error",
            RetryError::Provider(_) => "provider_error",
            RetryError::TokenLimit(_) => "token_limit",
        }
    }

    fn instruction(&self) -> &'static str {
        match self {
            RetryError::Message(_) => {
                "The previous model response failed AIR runtime schema validation. Return only a corrected JSON object matching required_output_schema exactly."
            }
            RetryError::Provider(_) => {
                "The previous provider call failed before AIR accepted a model response. Retry the task and return only a JSON object matching required_output_schema exactly."
            }
            RetryError::TokenLimit(_) => {
                "The previous provider call exceeded the model context limit. Retry with the compacted input and return only a JSON object matching required_output_schema exactly."
            }
        }
    }
}

fn model_input_for_attempt(
    input: &Value,
    attempt: u32,
    retry_error: Option<&RetryError>,
    schema: Option<&TypeSpec>,
    retry: Option<&air_core::RetryPolicy>,
) -> Value {
    let Some(error) = retry_error else {
        return input.clone();
    };
    let max_input_chars = match error {
        RetryError::TokenLimit(_) => retry
            .and_then(|retry| retry.on_provider_error.as_ref())
            .and_then(|policy| policy.token_limit.as_ref())
            .and_then(|policy| {
                policy
                    .max_input_chars
                    .get(attempt.saturating_sub(2) as usize)
                    .copied()
                    .or_else(|| policy.max_input_chars.last().copied())
            }),
        RetryError::Message(_) | RetryError::Provider(_) => None,
    };
    let input = max_input_chars
        .map(|max_chars| compact_json_value(input, max_chars))
        .unwrap_or_else(|| input.clone());

    let mut retry_context = Map::new();
    retry_context.insert("attempt".to_string(), Value::Number(attempt.into()));
    retry_context.insert(
        "previous_error".to_string(),
        Value::String(error.message().to_string()),
    );
    retry_context.insert(
        "reason".to_string(),
        Value::String(error.reason().to_string()),
    );
    if let Some(max_input_chars) = max_input_chars {
        retry_context.insert(
            "max_input_chars".to_string(),
            Value::Number(max_input_chars.into()),
        );
        retry_context.insert(
            "compaction".to_string(),
            Value::String("input was truncated after a provider token/context limit error".into()),
        );
    }
    if let Some(schema) = schema {
        retry_context.insert(
            "required_output_schema".to_string(),
            serde_json::to_value(schema).unwrap_or(Value::Null),
        );
    }
    retry_context.insert(
        "instruction".to_string(),
        Value::String(error.instruction().to_string()),
    );

    match input {
        Value::Object(object) => {
            let mut object = object;
            object.insert("_air_retry".to_string(), Value::Object(retry_context));
            Value::Object(object)
        }
        other => {
            let mut object = Map::new();
            object.insert("input".to_string(), other);
            object.insert("_air_retry".to_string(), Value::Object(retry_context));
            Value::Object(object)
        }
    }
}

fn is_token_limit_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "context_length_exceeded",
        "context length",
        "maximum context",
        "max context",
        "token limit",
        "too many tokens",
        "input is too long",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn compact_json_value(value: &Value, max_chars: usize) -> Value {
    let mut remaining = max_chars;
    compact_json_value_with_budget(value, &mut remaining)
}

fn compact_json_value_with_budget(value: &Value, remaining: &mut usize) -> Value {
    match value {
        Value::String(text) => {
            if *remaining == 0 {
                return Value::String(String::new());
            }
            let char_count = text.chars().count();
            if char_count <= *remaining {
                *remaining -= char_count;
                return Value::String(text.clone());
            }
            let truncated = truncate_middle_context_string(text, *remaining, "air:truncated");
            *remaining = 0;
            Value::String(truncated)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| compact_json_value_with_budget(value, remaining))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        compact_json_value_with_budget(value, remaining),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn output_spec<'a>(module: &'a AirModule, output: &str) -> Option<&'a TypeSpec> {
    module
        .state
        .get(output)
        .or_else(|| module.outputs.get(output))
        .or_else(|| module.inputs.get(output))
}

fn validate_tool_capability<T: ToolProvider>(
    module: &AirModule,
    tool: &str,
    provider: &T,
) -> Result<(), RuntimeError> {
    let Some(tool_spec) = module.tools.iter().find(|candidate| candidate.name == tool) else {
        return Err(RuntimeError::UndeclaredTool {
            module: module.agent.name.clone(),
            tool: tool.to_string(),
        });
    };
    if let Some(capability) = tool_spec.capability.as_ref() {
        if !module.requires.capabilities.contains(capability) {
            return Err(RuntimeError::ToolCapabilityNotRequired {
                module: module.agent.name.clone(),
                tool: tool.to_string(),
                capability: capability.clone(),
            });
        }
    }
    if let Some(provider_capability) = provider.tool_capability(tool) {
        match tool_spec.capability.as_deref() {
            Some(module_capability) if module_capability == provider_capability => {}
            Some(module_capability) => {
                return Err(RuntimeError::ToolCapabilityMismatch {
                    tool: tool.to_string(),
                    provider_capability: provider_capability.to_string(),
                    module_capability: module_capability.to_string(),
                });
            }
            None => {
                return Err(RuntimeError::ToolCapabilityNotRequired {
                    module: module.agent.name.clone(),
                    tool: tool.to_string(),
                    capability: provider_capability.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn resolve_tool_selection(selection: &Value) -> Result<(String, Value), RuntimeError> {
    let Some(object) = selection.as_object() else {
        return Err(RuntimeError::SchemaViolation(
            "tool_batch_dispatch item must be an object with fields tool and input".to_string(),
        ));
    };
    if let Some(tool) = object.get("tool").and_then(Value::as_str) {
        let input = object
            .get("input")
            .or_else(|| object.get("args"))
            .or_else(|| object.get("arguments"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        return Ok((tool.to_string(), input));
    }
    let Some(tool) = object.get("name").and_then(Value::as_str) else {
        return Err(RuntimeError::SchemaViolation(
            "tool_batch_dispatch item.tool must be a string".to_string(),
        ));
    };
    let input = object
        .get("args")
        .or_else(|| object.get("arguments"))
        .or_else(|| object.get("input"))
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    Ok((tool.to_string(), input))
}

fn normalize_model_selected_tool_name(
    module: &AirModule,
    tool: String,
) -> (String, Option<String>) {
    if module.tools.iter().any(|candidate| candidate.name == tool) {
        return (tool, None);
    }
    let normalized = tool.to_ascii_lowercase();
    if normalized != tool
        && module
            .tools
            .iter()
            .any(|candidate| candidate.name == normalized)
    {
        return (normalized, Some(tool));
    }
    (tool, None)
}

fn resolve_tool_batch_dispatch_items(
    batch: &Value,
) -> Result<Vec<ResolvedToolBatchItem>, RuntimeError> {
    let Some(items) = batch.as_array() else {
        return Err(RuntimeError::SchemaViolation(
            "tool_batch_dispatch input must be an array of objects with fields tool and input"
                .to_string(),
        ));
    };
    Ok(items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let resolved = resolve_tool_selection(item).map_err(|error| match error {
                RuntimeError::SchemaViolation(message) => RuntimeError::SchemaViolation(format!(
                    "tool_batch_dispatch input[{index}] invalid: {message}"
                )),
                other => other,
            });
            (item.clone(), resolved)
        })
        .collect())
}

fn copy_tool_selection_metadata(raw_selection: &Value, output_object: &mut Map<String, Value>) {
    let Some(object) = raw_selection.as_object() else {
        return;
    };
    for field in ["_air_tool_call_id", "_air_tool_name"] {
        if let Some(value) = object.get(field) {
            output_object.insert(field.to_string(), value.clone());
        }
    }
}

fn tool_batch_error_observation(tool: &str, input: &Value, error: &str) -> Value {
    json!({
        "tool": tool,
        "input": input,
        "status": "error",
        "error_code": "tool_error",
        "error": error,
        "output": {
            "status": "error",
            "error_code": "tool_error",
            "error": error,
        }
    })
}

fn tool_batch_error_observation_for_runtime_error(
    tool: &str,
    input: &Value,
    error: &RuntimeError,
) -> Value {
    let mut observation = tool_batch_error_observation(tool, input, &error.to_string());
    if let RuntimeError::RepeatedToolCallLimitExceeded {
        tool,
        limit,
        attempted,
    } = error
    {
        let policy = json!({
            "error_code": "doom_loop",
            "permission": "doom_loop",
            "message": "The same tool call repeated with identical semantic input. Use existing observations or choose a different next action instead of retrying the same call.",
            "tool": tool,
            "limit": limit,
            "attempted": attempted,
        });
        if let Some(object) = observation.as_object_mut() {
            for (key, value) in policy.as_object().unwrap() {
                object.insert(key.clone(), value.clone());
            }
            if let Some(output) = object.get_mut("output").and_then(Value::as_object_mut) {
                for (key, value) in policy.as_object().unwrap() {
                    output.insert(key.clone(), value.clone());
                }
            }
        }
    } else if let RuntimeError::RepeatedContextToolCallRequiresReason { tool, attempted } = error {
        let policy = json!({
            "error_code": "repeat_reason_required",
            "permission": "repeat_reason_required",
            "message": "This read/search/diagnostic tool call repeats a previous semantic input. Reissue it only if you include repeat_reason naming the exact missing fact; otherwise use existing observations and edit, verify, complete, or abort.",
            "tool": tool,
            "attempted": attempted,
        });
        if let Some(object) = observation.as_object_mut() {
            for (key, value) in policy.as_object().unwrap() {
                object.insert(key.clone(), value.clone());
            }
            if let Some(output) = object.get_mut("output").and_then(Value::as_object_mut) {
                for (key, value) in policy.as_object().unwrap() {
                    output.insert(key.clone(), value.clone());
                }
            }
        }
    }
    observation
}

fn tool_batch_level_error_observation(input: &Value, error: &str) -> Value {
    json!({
        "tool": "<batch>",
        "input": {
            "requested": input,
        },
        "status": "error",
        "error_code": "tool_error",
        "error": error,
        "output": {
            "status": "error",
            "error_code": "tool_error",
            "error": error,
            "requested": input,
        }
    })
}

fn tool_batch_malformed_observation(dispatch: &Value, error: &str) -> Value {
    let tool = invalid_dispatch_tool_name(dispatch);
    json!({
        "tool": tool,
        "input": {},
        "status": "error",
        "error_code": "tool_error",
        "error": error,
        "output": {
            "status": "error",
            "error_code": "tool_error",
            "error": error,
            "requested": dispatch,
        }
    })
}

fn invalid_dispatch_tool_name(dispatch: &Value) -> String {
    dispatch
        .as_object()
        .and_then(|object| object.get("tool").or_else(|| object.get("name")))
        .and_then(Value::as_str)
        .unwrap_or("<invalid>")
        .to_string()
}

fn validate_approval_capabilities(
    module: &AirModule,
    approval_for: &[String],
) -> Result<(), RuntimeError> {
    for capability in approval_for {
        if !module.policy.require_approval.contains(capability) {
            return Err(RuntimeError::ApprovalCapabilityNotRequired {
                module: module.agent.name.clone(),
                capability: capability.clone(),
            });
        }
    }
    Ok(())
}

fn enforce_repeated_tool_policy(
    context: &mut ExecutionContext<'_>,
    action_name: &str,
    tool: &str,
    input: &Value,
) -> Result<(), RuntimeError> {
    let Some(limit) = context.module.policy.max_repeated_tool_calls else {
        return Ok(());
    };
    let normalized_input = repeated_tool_input_key(tool, input);

    let repeated = recent_repeated_tool_call_count(context.tool_history, tool, &normalized_input);
    let attempted = repeated + 1;
    if repeated > 0 && context_tool_requires_repeat_reason(tool) && !has_repeat_reason(input) {
        let error = RuntimeError::RepeatedContextToolCallRequiresReason {
            tool: tool.to_string(),
            attempted,
        };
        context.push_event_with_meta(
            action_name,
            Some(input.clone()),
            None,
            Some(json!({
                "tool": tool,
                "attempted": attempted,
                "error_code": "repeat_reason_required",
            })),
            Err(error.to_string()),
        );
        return Err(error);
    }
    if attempted > limit {
        let error = RuntimeError::RepeatedToolCallLimitExceeded {
            tool: tool.to_string(),
            limit,
            attempted,
        };
        context.push_event_with_meta(
            action_name,
            Some(input.clone()),
            None,
            Some(json!({
                "tool": tool,
                "limit": limit,
                "attempted": attempted,
            })),
            Err(error.to_string()),
        );
        return Err(error);
    }

    context.tool_history.push(ToolCallRecord {
        tool: tool.to_string(),
        input: normalized_input,
    });
    Ok(())
}

fn context_tool_requires_repeat_reason(tool: &str) -> bool {
    matches!(
        tool,
        "read"
            | "file.read"
            | "read_many"
            | "file.read_many"
            | "grep"
            | "file.search"
            | "glob"
            | "repo.files"
            | "repo.search"
            | "repo.context"
            | "repo.symbols"
            | "repo.references"
            | "lsp"
    )
}

fn has_repeat_reason(input: &Value) -> bool {
    input
        .as_object()
        .and_then(|object| object.get("repeat_reason"))
        .and_then(Value::as_str)
        .is_some_and(|reason| !reason.trim().is_empty())
}

fn recent_repeated_tool_call_count(
    history: &[ToolCallRecord],
    tool: &str,
    normalized_input: &Value,
) -> u32 {
    history
        .iter()
        .rev()
        .take(12)
        .filter(|record| record.tool == tool && record.input == *normalized_input)
        .count() as u32
}

fn repeated_tool_input_key(tool: &str, input: &Value) -> Value {
    match tool {
        "read" | "file.read" => pick_tool_input_fields(
            input,
            &[
                "path",
                "filePath",
                "offset",
                "limit",
                "start_line",
                "end_line",
                "lines",
                "contains",
                "occurrence",
            ],
        ),
        "read_many" | "file.read_many" => pick_tool_input_fields(input, &["files"]),
        "grep" | "file.search" => {
            pick_tool_input_fields(input, &["path", "include", "pattern", "query"])
        }
        "glob" | "repo.files" => {
            pick_tool_input_fields(input, &["pattern", "query", "path", "include_all"])
        }
        "repo.search" | "repo.context" => pick_tool_input_fields(
            input,
            &["path", "file_glob", "glob", "query", "pattern", "mode"],
        ),
        "repo.symbols" => pick_tool_input_fields(input, &["path", "query", "names"]),
        "repo.references" => pick_tool_input_fields(input, &["path", "symbol"]),
        "lsp" => pick_tool_input_fields(
            input,
            &["command", "method", "path", "symbol", "line", "character"],
        ),
        _ => strip_tool_reason_fields(input),
    }
}

fn pick_tool_input_fields(input: &Value, fields: &[&str]) -> Value {
    let Some(object) = input.as_object() else {
        return input.clone();
    };
    let mut key = Map::new();
    for field in fields {
        if let Some(value) = object.get(*field) {
            key.insert((*field).to_string(), value.clone());
        }
    }
    Value::Object(key)
}

fn strip_tool_reason_fields(input: &Value) -> Value {
    let Some(object) = input.as_object() else {
        return input.clone();
    };
    let mut key = object.clone();
    for field in [
        "reason",
        "repeat_reason",
        "reread_reason",
        "search_reason",
        "why",
    ] {
        key.remove(field);
    }
    Value::Object(key)
}

struct ExecutionContext<'a> {
    module: &'a AirModule,
    state: &'a mut State,
    outputs: &'a mut State,
    trace: &'a mut Vec<TraceEvent>,
    observer: &'a mut dyn FnMut(&TraceEvent),
    model_calls: &'a mut u32,
    tool_calls: &'a mut u32,
    tool_history: &'a mut Vec<ToolCallRecord>,
    artifact_registry: &'a mut BTreeSet<String>,
    module_started_at: Instant,
    step: u32,
    rule: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolCallRecord {
    tool: String,
    input: Value,
}

struct ToolExecution<'a> {
    action_name: &'a str,
    tool: &'a str,
    requested_tool: Option<&'a str>,
    input: Value,
    output: &'a str,
    timeout_seconds: u64,
    retry: &'a Option<RetryPolicy>,
}

struct ToolBatchExecution<'a> {
    input: Value,
    output: &'a str,
    timeout_seconds: u64,
    max_calls: u32,
    retry: &'a Option<RetryPolicy>,
    on_error: ToolErrorMode,
}

#[derive(Clone, Copy)]
struct ToolCallMeta<'a> {
    tool: &'a str,
    requested_tool: Option<&'a str>,
    attempt: u32,
    max_attempts: u32,
    output: &'a str,
    timeout_seconds: u64,
}

impl ExecutionContext<'_> {
    fn push_event(
        &mut self,
        action: &str,
        input: Option<Value>,
        output: Option<Value>,
        status: Result<(), String>,
    ) {
        self.push_event_with_meta(action, input, output, None, status);
    }

    fn push_event_with_meta(
        &mut self,
        action: &str,
        input: Option<Value>,
        output: Option<Value>,
        meta: Option<Value>,
        status: Result<(), String>,
    ) {
        let event = event(
            self.module,
            self.step,
            self.rule,
            action,
            EventPayload {
                input,
                output,
                meta,
            },
            &status,
        );
        (self.observer)(&event);
        self.trace.push(event);
    }
}

fn check_module_timeout(context: &mut ExecutionContext<'_>) -> Result<(), RuntimeError> {
    let Some(limit) = context.module.policy.timeout_seconds else {
        return Ok(());
    };
    let elapsed = context.module_started_at.elapsed();
    if elapsed < Duration::from_secs(limit) {
        return Ok(());
    }

    let error = RuntimeError::ModuleTimeoutExceeded {
        limit,
        elapsed_ms: elapsed.as_millis(),
    };
    context.push_event("module_timeout", None, None, Err(error.to_string()));
    Err(error)
}

fn action_timeout_error(
    action: &str,
    timeout_seconds: u64,
    started_at: Instant,
) -> Option<RuntimeError> {
    let elapsed = started_at.elapsed();
    if elapsed < Duration::from_secs(timeout_seconds) {
        return None;
    }

    Some(RuntimeError::ActionTimeoutExceeded {
        action: action.to_string(),
        timeout_seconds,
        elapsed_ms: elapsed.as_millis(),
    })
}

struct ModelCallMetaArgs<'a> {
    model: &'a str,
    attempt: u32,
    max_attempts: u32,
    output: &'a str,
    timeout_seconds: u64,
    input_bytes: usize,
}

fn model_call_meta(args: &ModelCallMetaArgs<'_>) -> Value {
    serde_json::json!({
        "model": args.model,
        "attempt": args.attempt,
        "max_attempts": args.max_attempts,
        "output": args.output,
        "timeout_seconds": args.timeout_seconds,
        "input_bytes": args.input_bytes,
    })
}

fn model_call_result_meta(
    args: &ModelCallMetaArgs<'_>,
    elapsed_ms: u128,
    will_retry: bool,
) -> Value {
    model_call_result_meta_with_provider(args, elapsed_ms, will_retry, None)
}

fn model_call_result_meta_with_provider(
    args: &ModelCallMetaArgs<'_>,
    elapsed_ms: u128,
    will_retry: bool,
    provider_stats: Option<&ModelRequestStats>,
) -> Value {
    let mut meta = model_call_meta(args);
    insert_result_meta(&mut meta, elapsed_ms, will_retry);
    if let Some(stats) = provider_stats {
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(
                "provider_request_bytes".to_string(),
                json!(stats.provider_request_bytes),
            );
            meta.insert(
                "provider_user_content_bytes".to_string(),
                json!(stats.provider_user_content_bytes),
            );
            meta.insert(
                "provider_tools_bytes".to_string(),
                json!(stats.provider_tools_bytes),
            );
            if let Some(request) = &stats.provider_request {
                meta.insert("provider_request".to_string(), request.clone());
            }
            if let Some(response) = &stats.provider_response {
                meta.insert("provider_response".to_string(), response.clone());
            }
        }
    }
    meta
}

fn json_value_size_bytes(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|value| value.len())
        .unwrap_or(0)
}

fn tool_call_meta(args: ToolCallMeta<'_>) -> Value {
    let mut meta = serde_json::json!({
        "tool": args.tool,
        "attempt": args.attempt,
        "max_attempts": args.max_attempts,
        "output": args.output,
        "timeout_seconds": args.timeout_seconds,
    });
    insert_requested_tool_meta(&mut meta, args.requested_tool);
    meta
}

fn tool_call_result_meta(args: ToolCallMeta<'_>, elapsed_ms: u128, will_retry: bool) -> Value {
    let mut meta = tool_call_meta(args);
    insert_result_meta(&mut meta, elapsed_ms, will_retry);
    meta
}

fn tool_error_meta(tool: &str, requested_tool: Option<&str>) -> Value {
    let mut meta = json!({ "tool": tool });
    insert_requested_tool_meta(&mut meta, requested_tool);
    meta
}

fn insert_requested_tool_meta(meta: &mut Value, requested_tool: Option<&str>) {
    let Some(requested_tool) = requested_tool else {
        return;
    };
    if let Value::Object(object) = meta {
        object.insert(
            "requested_tool".to_string(),
            Value::String(requested_tool.to_string()),
        );
    }
}

fn approval_meta(approval_for: &[String], decision: &ApprovalDecision) -> Value {
    let mut meta = json!({
        "approval_for": approval_for,
        "approved": decision.approved,
    });
    if let Value::Object(object) = &mut meta {
        if let Some(approver) = &decision.approver {
            object.insert("approver".to_string(), Value::String(approver.clone()));
        }
        if let Some(reason) = &decision.reason {
            object.insert("reason".to_string(), Value::String(reason.clone()));
        }
        if let Some(metadata) = &decision.metadata {
            object.insert("metadata".to_string(), metadata.clone());
        }
    }
    meta
}

fn insert_result_meta(meta: &mut Value, elapsed_ms: u128, will_retry: bool) {
    if let Value::Object(object) = meta {
        object.insert(
            "elapsed_ms".to_string(),
            serde_json::to_value(elapsed_ms).unwrap_or(Value::Null),
        );
        object.insert("will_retry".to_string(), Value::Bool(will_retry));
    }
}

fn insert_batch_item_meta(meta: &mut Value, index: usize) {
    if let Value::Object(object) = meta {
        object.insert("index".to_string(), Value::Number(index.into()));
    }
}

fn insert_artifact_result_meta(meta: &mut Value, artifact_ids: &[String]) {
    if artifact_ids.is_empty() {
        return;
    }
    if let Value::Object(object) = meta {
        object.insert("artifact_ids".to_string(), json!(artifact_ids));
        object.insert(
            "artifact_count".to_string(),
            Value::Number(artifact_ids.len().into()),
        );
    }
}

fn read_field<'a>(
    state: &'a State,
    outputs: &'a State,
    field: &str,
) -> Result<&'a Value, RuntimeError> {
    state
        .get(field)
        .or_else(|| outputs.get(field))
        .ok_or_else(|| RuntimeError::MissingField(field.to_string()))
}

fn runtime_context(step: u32, max_steps: u32, model_calls: u32, tool_calls: u32) -> Value {
    let remaining_steps = max_steps.saturating_sub(step);
    json!({
        "step": step,
        "step_number": step.saturating_add(1),
        "max_steps": max_steps,
        "remaining_steps": remaining_steps,
        "model_calls": model_calls,
        "tool_calls": tool_calls,
        "is_last_step": remaining_steps <= 1,
        "is_last_action_step": remaining_steps <= 2,
    })
}

fn resolve_input(state: &State, outputs: &State, input: &InputSpec) -> Result<Value, RuntimeError> {
    match input {
        InputSpec::Field(field) => Ok(read_field(state, outputs, field)?.clone()),
        InputSpec::Fields { fields } => {
            let mut object = Map::new();
            for (alias, field) in fields {
                object.insert(alias.clone(), read_field(state, outputs, field)?.clone());
            }
            Ok(Value::Object(object))
        }
        InputSpec::Expr(expr) => eval_expr(state, outputs, expr),
    }
}

fn resolve_set_value(state: &State, outputs: &State, value: &Value) -> Result<Value, RuntimeError> {
    if looks_like_expr(value) {
        let expr = serde_json::from_value::<Expr>(value.clone()).map_err(|error| {
            RuntimeError::SchemaViolation(format!("set expression is invalid: {error}"))
        })?;
        eval_expr(state, outputs, &expr)
    } else {
        Ok(value.clone())
    }
}

fn looks_like_expr(value: &Value) -> bool {
    let Value::Object(object) = value else {
        return false;
    };
    if object.len() != 1 {
        return false;
    }
    object.keys().next().is_some_and(|key| {
        matches!(
            key.as_str(),
            "ref"
                | "path"
                | "literal"
                | "object"
                | "array"
                | "template"
                | "truncate"
                | "take_last"
                | "take_last_within_bytes"
                | "split_lines"
                | "equals"
                | "is_empty"
                | "not"
        )
    })
}

fn eval_expr(state: &State, outputs: &State, expr: &Expr) -> Result<Value, RuntimeError> {
    match expr {
        Expr::Ref { reference } => read_path(state, outputs, reference).cloned(),
        Expr::Path { path } => read_path(state, outputs, path).cloned(),
        Expr::Literal { literal } => Ok(literal.clone()),
        Expr::Object { object } => {
            let mut value = Map::new();
            for (key, expr) in object {
                value.insert(key.clone(), eval_expr(state, outputs, expr)?);
            }
            Ok(Value::Object(value))
        }
        Expr::Array { array } => {
            let mut values = Vec::with_capacity(array.len());
            for expr in array {
                values.push(eval_expr(state, outputs, expr)?);
            }
            Ok(Value::Array(values))
        }
        Expr::Template { template } => render_template(state, outputs, template).map(Value::String),
        Expr::Truncate {
            truncate,
            max_chars,
        } => Ok(Value::String(truncate_value(
            &eval_expr(state, outputs, truncate)?,
            *max_chars,
        ))),
        Expr::TakeLast {
            take_last,
            max_items,
        } => take_last_value(&eval_expr(state, outputs, take_last)?, *max_items),
        Expr::TakeLastWithinBytes {
            take_last_within_bytes,
            max_bytes,
        } => take_last_within_bytes_value(
            &eval_expr(state, outputs, take_last_within_bytes)?,
            *max_bytes,
        ),
        Expr::SplitLines { split_lines } => {
            let value = eval_expr(state, outputs, split_lines)?;
            let Some(text) = value.as_str() else {
                return Err(RuntimeError::UnsupportedExpression(
                    "split_lines expects a string operand".to_string(),
                ));
            };
            Ok(Value::Array(
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| Value::String(line.to_string()))
                    .collect(),
            ))
        }
        Expr::Equals { equals } => {
            let [left, right] = equals.as_slice() else {
                return Err(RuntimeError::UnsupportedExpression(
                    "equals expects exactly two operands".to_string(),
                ));
            };
            Ok(Value::Bool(
                eval_expr(state, outputs, left)? == eval_expr(state, outputs, right)?,
            ))
        }
        Expr::IsEmpty { is_empty } => {
            let value = eval_expr(state, outputs, is_empty)?;
            Ok(Value::Bool(value_is_empty(&value)))
        }
        Expr::Not { not } => {
            let value = eval_expr(state, outputs, not)?;
            let Some(value) = value.as_bool() else {
                return Err(RuntimeError::UnsupportedExpression(
                    "not expects a boolean operand".to_string(),
                ));
            };
            Ok(Value::Bool(!value))
        }
    }
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

fn read_path<'a>(
    state: &'a State,
    outputs: &'a State,
    path: &str,
) -> Result<&'a Value, RuntimeError> {
    let segments = path_segments(path);
    let Some(first) = segments.first().filter(|segment| !segment.is_empty()) else {
        return Err(RuntimeError::MissingField(path.to_string()));
    };

    let mut value = read_field(state, outputs, first)?;
    let missing = || RuntimeError::MissingField(path.to_string());
    for segment in segments.iter().skip(1) {
        value = match value {
            Value::Object(object) => object.get(segment).ok_or_else(missing)?,
            Value::Array(array) => {
                let index = segment.parse::<usize>().map_err(|_| missing())?;
                array.get(index).ok_or_else(missing)?
            }
            _ => return Err(missing()),
        };
    }

    Ok(value)
}

fn render_template(state: &State, outputs: &State, template: &str) -> Result<String, RuntimeError> {
    let mut rendered = String::new();
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        let (before, after_start) = rest.split_at(start);
        rendered.push_str(before);
        let after_start = &after_start[2..];
        let Some(end) = after_start.find("}}") else {
            rendered.push_str("{{");
            rendered.push_str(after_start);
            return Ok(rendered);
        };

        let (path, after_end) = after_start.split_at(end);
        let value = read_path(state, outputs, path.trim())?;
        rendered.push_str(&value_to_template_string(value));
        rest = &after_end[2..];
    }

    rendered.push_str(rest);
    Ok(rendered)
}

fn value_to_template_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn truncate_value(value: &Value, max_chars: usize) -> String {
    let raw = value_to_template_string(value);
    raw.chars().take(max_chars).collect()
}

fn take_last_value(value: &Value, max_items: usize) -> Result<Value, RuntimeError> {
    let Some(values) = value.as_array() else {
        return Err(RuntimeError::Provider(
            "take_last expression expected an array value".to_string(),
        ));
    };
    let start = values.len().saturating_sub(max_items);
    Ok(Value::Array(values[start..].to_vec()))
}

fn event(
    module: &AirModule,
    step: u32,
    rule: &str,
    action: &str,
    payload: EventPayload,
    status: &Result<(), String>,
) -> TraceEvent {
    TraceEvent {
        agent: module.agent.name.clone(),
        step,
        rule: rule.to_string(),
        action: action.to_string(),
        input: payload.input,
        output: payload.output,
        meta: payload.meta,
        status: match status {
            Ok(()) => TraceStatus::Ok,
            Err(_) => TraceStatus::Error,
        },
        error: status.as_ref().err().cloned(),
    }
}

struct EventPayload {
    input: Option<Value>,
    output: Option<Value>,
    meta: Option<Value>,
}

fn condition_matches(
    state: &State,
    outputs: &State,
    condition: &str,
) -> Result<bool, RuntimeError> {
    for group in condition.split("||") {
        let mut group_matches = true;
        for clause in group.split("&&") {
            if !condition_clause_matches(state, outputs, clause.trim())? {
                group_matches = false;
                break;
            }
        }
        if group_matches {
            return Ok(true);
        }
    }
    Ok(false)
}

fn condition_clause_matches(
    state: &State,
    outputs: &State,
    clause: &str,
) -> Result<bool, RuntimeError> {
    let Some((left, operator, right)) = split_condition_clause(clause) else {
        return Err(RuntimeError::UnsupportedCondition(clause.to_string()));
    };

    let actual = match read_path(state, outputs, left.trim()) {
        Ok(value) => value,
        Err(RuntimeError::MissingField(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    let expected = parse_condition_literal(right.trim())?;
    match operator {
        "==" => Ok(actual == &expected),
        "!=" => Ok(actual != &expected),
        ">" | ">=" | "<" | "<=" => {
            let Some(actual) = actual.as_f64() else {
                return Err(RuntimeError::UnsupportedCondition(clause.to_string()));
            };
            let Some(expected) = expected.as_f64() else {
                return Err(RuntimeError::UnsupportedCondition(clause.to_string()));
            };
            Ok(match operator {
                ">" => actual > expected,
                ">=" => actual >= expected,
                "<" => actual < expected,
                "<=" => actual <= expected,
                _ => unreachable!(),
            })
        }
        _ => Err(RuntimeError::UnsupportedCondition(clause.to_string())),
    }
}

fn split_condition_clause(clause: &str) -> Option<(&str, &'static str, &str)> {
    for operator in [">=", "<=", "==", "!=", ">", "<"] {
        if let Some((left, right)) = clause.split_once(operator) {
            return Some((left, operator, right));
        }
    }
    None
}

fn parse_condition_literal(raw: &str) -> Result<Value, RuntimeError> {
    if raw.is_empty() {
        return Err(RuntimeError::UnsupportedCondition(raw.to_string()));
    }
    if (raw.starts_with('"') && raw.ends_with('"'))
        || (raw.starts_with('\'') && raw.ends_with('\''))
    {
        return Ok(Value::String(raw[1..raw.len() - 1].to_string()));
    }
    serde_json::from_str(raw).map_err(|_| RuntimeError::UnsupportedCondition(raw.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_candidates_from_model_content_with_revision() {
        let value = json!({
            "content": "first {\"patch\":\"bad\",\"rationale\":\"old\"}\n\
                Thinking aloud.\n\
                {\"patch\":\"good { still inside string }\",\"rationale\":\"new\"}"
        });

        let candidates = model_content_json_candidates(&value);

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0]["patch"], json!("bad"));
        assert_eq!(
            candidates[1]["patch"],
            json!("good { still inside string }")
        );
        assert_eq!(candidates[1]["rationale"], json!("new"));
    }

    #[test]
    fn extracts_json_candidate_from_fenced_model_content() {
        let value = json!({
            "content": "```json\n{\"patch\":\"diff\",\"rationale\":\"ok\"}\n```"
        });

        let candidates = model_content_json_candidates(&value);

        assert!(!candidates.is_empty());
        assert_eq!(candidates[0]["patch"], json!("diff"));
    }

    #[test]
    fn extracts_schema_candidates_from_answer_wrapper() {
        let value = json!({
            "answer": {
                "complete": true,
                "tool_calls": [],
                "rationale": "done"
            }
        });

        let candidates = model_output_schema_candidates(&value);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["complete"], json!(true));
        assert_eq!(candidates[0]["rationale"], json!("done"));
    }

    #[test]
    fn extracts_schema_candidates_from_string_answer_wrapper() {
        let value = json!({
            "answer": "{\"complete\":true,\"tool_calls\":[],\"rationale\":\"done\"}"
        });

        let candidates = model_output_schema_candidates(&value);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["complete"], json!(true));
        assert_eq!(candidates[0]["rationale"], json!("done"));
    }

    #[test]
    fn content_wrapper_schema_error_gives_structured_retry_feedback() {
        let error = provider_content_wrapper_schema_error(
            &json!({"content": "I need to read more files before deciding."}),
            RuntimeError::SchemaViolation("decision.complete missing required field".to_string()),
        );
        let message = error.to_string();

        assert!(message.contains("invalid structured model output"));
        assert!(message.contains("declared AIR output interface"));
        assert!(message.contains("Return the required JSON object directly"));
        assert!(message.contains("content_preview="));
    }

    #[test]
    fn read_path_supports_array_index_segments() {
        let mut state = State::new();
        state.insert(
            "observation".to_string(),
            json!([{"tool": "edit", "status": "ok"}]),
        );
        let outputs = State::new();

        assert_eq!(
            read_path(&state, &outputs, "observation[0].tool").unwrap(),
            &json!("edit")
        );
        assert_eq!(
            read_path(&state, &outputs, "observation.0.status").unwrap(),
            &json!("ok")
        );
    }

    #[test]
    fn condition_matches_numeric_runtime_context_comparisons() {
        let mut state = State::new();
        state.insert(
            "_air".to_string(),
            json!({
                "model_calls": 8,
                "tool_calls": 12,
            }),
        );
        let outputs = State::new();

        assert!(condition_matches(&state, &outputs, "_air.model_calls >= 8").unwrap());
        assert!(condition_matches(&state, &outputs, "_air.tool_calls < 20").unwrap());
        assert!(!condition_matches(&state, &outputs, "_air.model_calls > 8").unwrap());
    }

    #[test]
    fn condition_matches_treats_missing_nested_path_as_false() {
        let mut state = State::new();
        state.insert("phase".to_string(), json!("post_act"));
        state.insert(
            "observation".to_string(),
            json!([{
                "tool": "format",
                "status": "ok",
                "output": {"success": true}
            }]),
        );
        let outputs = State::new();

        assert!(!condition_matches(
            &state,
            &outputs,
            "phase == \"post_act\" && observation[1].tool == \"test\"",
        )
        .unwrap());
    }

    #[test]
    fn eval_expr_supports_boolean_composition_helpers() {
        let mut state = State::new();
        state.insert("verification_status".to_string(), json!("passed"));
        state.insert("changed_files".to_string(), json!(["src/lib.rs"]));
        let outputs = State::new();

        let final_success = eval_expr(
            &state,
            &outputs,
            &Expr::Equals {
                equals: vec![
                    Expr::Ref {
                        reference: "verification_status".to_string(),
                    },
                    Expr::Literal {
                        literal: json!("passed"),
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(final_success, json!(true));

        let patch_applied = eval_expr(
            &state,
            &outputs,
            &Expr::Not {
                not: Box::new(Expr::IsEmpty {
                    is_empty: Box::new(Expr::Ref {
                        reference: "changed_files".to_string(),
                    }),
                }),
            },
        )
        .unwrap();
        assert_eq!(patch_applied, json!(true));
    }

    #[test]
    fn repeated_tool_key_ignores_non_semantic_read_and_search_options() {
        assert_eq!(
            repeated_tool_input_key(
                "grep",
                &json!({
                    "path": "src/lib.rs",
                    "pattern": "truncat",
                    "context_lines": 3,
                    "max_matches": 20,
                    "repeat_reason": "need a wider preview"
                })
            ),
            repeated_tool_input_key(
                "grep",
                &json!({
                    "path": "src/lib.rs",
                    "pattern": "truncat",
                    "context_lines": 5,
                    "max_matches": 80,
                    "repeat_reason": "checking whether the same match appears"
                })
            )
        );
        assert_eq!(
            repeated_tool_input_key(
                "read",
                &json!({
                    "filePath": "src/lib.rs",
                    "start_line": 10,
                    "end_line": 20,
                    "line_numbers": true,
                    "max_bytes": 4096,
                    "repeat_reason": "confirming same range"
                })
            ),
            repeated_tool_input_key(
                "read",
                &json!({
                    "filePath": "src/lib.rs",
                    "start_line": 10,
                    "end_line": 20,
                    "line_numbers": false,
                    "max_bytes": 65536,
                    "repeat_reason": "different explanation should not reset repeat policy"
                })
            )
        );
        assert_eq!(
            repeated_tool_input_key(
                "custom.tool",
                &json!({
                    "path": "src/lib.rs",
                    "reason": "first explanation",
                    "repeat_reason": "read again"
                })
            ),
            repeated_tool_input_key(
                "custom.tool",
                &json!({
                    "path": "src/lib.rs",
                    "reason": "second explanation",
                    "repeat_reason": "still read again"
                })
            )
        );
    }

    #[test]
    fn repeated_tool_count_uses_recent_window_instead_of_only_consecutive_calls() {
        let file_search = repeated_tool_input_key(
            "file.search",
            &json!({"path": "src/lib.rs", "pattern": "truncat", "context_lines": 3}),
        );
        let history = vec![
            ToolCallRecord {
                tool: "file.search".to_string(),
                input: file_search.clone(),
            },
            ToolCallRecord {
                tool: "repo.symbols".to_string(),
                input: json!({"path": "src/lib.rs", "query": "truncat"}),
            },
            ToolCallRecord {
                tool: "file.search".to_string(),
                input: repeated_tool_input_key(
                    "file.search",
                    &json!({"path": "src/lib.rs", "pattern": "truncat", "context_lines": 5}),
                ),
            },
        ];

        assert_eq!(
            recent_repeated_tool_call_count(&history, "file.search", &file_search),
            2
        );
    }

    #[test]
    fn repeated_context_tools_require_model_visible_reason() {
        assert!(context_tool_requires_repeat_reason("read"));
        assert!(context_tool_requires_repeat_reason("grep"));
        assert!(context_tool_requires_repeat_reason("repo.symbols"));
        assert!(context_tool_requires_repeat_reason("lsp"));
        assert!(!context_tool_requires_repeat_reason("edit"));
        assert!(!context_tool_requires_repeat_reason("format"));

        assert!(has_repeat_reason(&json!({
            "path": "src/lib.rs",
            "repeat_reason": "need the import block after locating the moved function"
        })));
        assert!(!has_repeat_reason(&json!({
            "path": "src/lib.rs",
            "repeat_reason": "   "
        })));
        assert!(!has_repeat_reason(&json!({"path": "src/lib.rs"})));

        let error = RuntimeError::RepeatedContextToolCallRequiresReason {
            tool: "read".to_string(),
            attempted: 2,
        };
        let observation = tool_batch_error_observation_for_runtime_error(
            "read",
            &json!({"path": "src/lib.rs"}),
            &error,
        );
        assert_eq!(observation["error_code"], json!("repeat_reason_required"));
        assert_eq!(
            observation["output"]["error_code"],
            json!("repeat_reason_required")
        );
        assert!(observation["output"]["message"]
            .as_str()
            .unwrap()
            .contains("include repeat_reason"));
    }

    #[test]
    fn take_last_within_bytes_preserves_read_many_file_context_when_compacting() {
        let large_content = format!(
            "{}\nfn target_helper() {{\n    println!(\"keep this\");\n}}\n{}",
            "prefix\n".repeat(2_000),
            "suffix\n".repeat(2_000)
        );
        let evidence = json!([{
            "action": "file_read_many_evidence",
            "rationale": "preserve source context",
            "requested": [{
                "tool": "read_many",
                "input": {
                    "files": ["src/lib.rs", "src/tools.rs"],
                    "line_numbers": true
                }
            }],
            "result": [{
                "tool": "read_many",
                "status": "ok",
                "input": {
                    "files": ["src/lib.rs", "src/tools.rs"],
                    "line_numbers": true
                },
                "output": {
                    "file_count": 2,
                    "bytes": 120000,
                    "truncated": true,
                    "files": [{
                        "path": "src/lib.rs",
                        "content": large_content,
                        "start_line": null,
                        "end_line": null,
                        "truncated": true,
                        "truncation_hint": "Use a narrower range"
                    }, {
                        "path": "src/tools.rs",
                        "content": "fn other_helper() {}",
                        "start_line": 1,
                        "end_line": 1,
                        "truncated": false
                    }]
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 18_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(rendered.contains("\"files\""), "{rendered}");
        assert!(rendered.contains("src/lib.rs"), "{rendered}");
        assert!(rendered.contains("\"content\""), "{rendered}");
        assert!(rendered.contains("Use a narrower range"), "{rendered}");
        assert!(
            !rendered.contains("\"result\":{\"_air_truncated\":true}"),
            "{rendered}"
        );
        assert!(rendered.len() <= 18_000, "compacted evidence is too large");
    }

    #[test]
    fn take_last_within_bytes_preserves_glob_file_paths_when_compacting() {
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "glob",
                "status": "ok",
                "input": {"pattern": "crates/air-tools/src/**/*.rs"},
                "output": {
                    "glob": "crates/air-tools/src/**/*.rs",
                    "mode": "fixed",
                    "files": [
                        "crates/air-tools/src/lib.rs",
                        "crates/air-tools/src/file_tools.rs"
                    ],
                    "truncated": false
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 20_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(
            rendered.contains("crates/air-tools/src/lib.rs"),
            "{rendered}"
        );
        assert!(
            rendered.contains("crates/air-tools/src/file_tools.rs"),
            "{rendered}"
        );
        assert!(rendered.contains("\"glob\""), "{rendered}");
    }

    #[test]
    fn take_last_within_bytes_has_no_item_count_cap() {
        let evidence = Value::Array(
            (0..20)
                .map(|index| {
                    json!({
                        "action": "tool_result",
                        "result": [{
                            "tool": "read",
                            "status": "ok",
                            "output": {
                                "path": format!("src/{index}.rs"),
                                "content": format!("note-{index}")
                            }
                        }]
                    })
                })
                .collect(),
        );

        let compacted = take_last_within_bytes_value(&evidence, 100_000).unwrap();
        let items = compacted.as_array().unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert_eq!(items.len(), 20, "{rendered}");
        assert!(rendered.contains("note-0"), "{rendered}");
        assert!(rendered.contains("note-19"), "{rendered}");
    }

    #[test]
    fn take_last_within_bytes_preserves_no_match_tool_hints_when_compacting() {
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "glob",
                "status": "ok",
                "input": {"pattern": "**/*edit*loop*test*"},
                "output": {
                    "glob": "**/*edit*loop*test*",
                    "file_count": 0,
                    "files": [],
                    "no_matches": true,
                    "no_match_streak": 2,
                    "search_hint": "No files matched; try a different glob."
                }
            }, {
                "tool": "grep",
                "status": "ok",
                "input": {"pattern": "does_not_exist"},
                "output": {
                    "path": ".",
                    "pattern": "does_not_exist",
                    "match_count": 0,
                    "returned_match_count": 0,
                    "matches": [],
                    "no_matches": true,
                    "search_hint": "No matches in the current directory scope. broaden the pattern/path."
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 20_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(rendered.contains("\"no_matches\":true"), "{rendered}");
        assert!(rendered.contains("\"no_match_streak\":2"), "{rendered}");
        assert!(rendered.contains("try a different glob"), "{rendered}");
        assert!(rendered.contains("broaden the pattern/path"), "{rendered}");
    }

    #[test]
    fn take_last_within_bytes_preserves_command_output_log_when_compacting() {
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "bash",
                "status": "ok",
                "input": {
                    "command": "rg -n \"file_edit\" crates/air-tools/src/tests.rs"
                },
                "output": {
                    "success": true,
                    "status": 0,
                    "bytes": 128,
                    "truncated": false,
                    "log": "12:file_edit applies exact replacement\n44:file_edit rejects missing oldString"
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 20_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(rendered.contains("\"log\""), "{rendered}");
        assert!(
            rendered.contains("file_edit applies exact replacement"),
            "{rendered}"
        );
        assert!(rendered.contains("\"status\":0"), "{rendered}");
    }

    #[test]
    fn take_last_within_bytes_preserves_subagent_output_when_compacting() {
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "task",
                "status": "ok",
                "_air_tool_name": "task",
                "_air_tool_call_id": "call_task",
                "input": {
                    "description": "Explore transcript rendering",
                    "subagent_type": "explore"
                },
                "output": {
                    "success": true,
                    "status": 0,
                    "bytes": 7104,
                    "raw_output_bytes": 7104,
                    "subagent_type": "explore",
                    "output": "Findings:\n- crates/air-backend-openai/src/lib.rs:2147-2169 `render_tool_output_transcript` - dispatches tool transcript rendering\nNext action:\n- read crates/air-backend-openai/src/lib.rs lines 2121-2169"
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 20_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(rendered.contains("\"output\""), "{rendered}");
        assert!(
            rendered.contains("render_tool_output_transcript"),
            "{rendered}"
        );
        assert!(rendered.contains("Next action"), "{rendered}");
        assert!(
            rendered.contains("\"subagent_type\":\"explore\""),
            "{rendered}"
        );
        assert!(
            rendered.contains("\"_air_tool_call_id\":\"call_task\""),
            "{rendered}"
        );
    }

    #[test]
    fn take_last_within_bytes_updates_visible_file_end_line_after_compaction() {
        let content = (1..=2_000)
            .map(|line| format!("{line:05}| line {line:04} {}", "x".repeat(40)))
            .collect::<Vec<_>>()
            .join("\n");
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "read",
                "status": "ok",
                "input": {"filePath": "src/lib.rs"},
                "output": {
                    "path": "src/lib.rs",
                    "content": content,
                    "content_format": "line_numbered",
                    "start_line": 1,
                    "end_line": 2_000,
                    "total_lines": 2_400,
                    "max_bytes": 65_536,
                    "truncated": true
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 200_000).unwrap();
        let output = &compacted[0]["result"][0]["output"];
        let visible_end = output["end_line"].as_u64().unwrap();

        assert!(visible_end < 2_000, "{output}");
        assert!(visible_end > 100, "{output}");
        assert_eq!(output["next_offset"], json!(visible_end));
        assert_eq!(output["max_bytes"], json!(60 * 1024));
    }

    #[test]
    fn take_last_within_bytes_compacts_tool_observations_before_budgeting() {
        let evidence = json!([{
            "action": "tool_result",
            "rationale": "read target function",
            "assistant": {
                "_air_assistant": {
                    "content": "I will read the narrow target function."
                },
                "complete": false,
                "tool_calls": [{
                    "tool": "read",
                    "input": {"filePath": "src/lib.rs", "start_line": 10, "end_line": 12}
                }]
            },
            "result": [{
                "tool": "read",
                "status": "ok",
                "input": {
                    "filePath": "src/lib.rs",
                    "start_line": 10,
                    "end_line": 12
                },
                "output": {
                    "path": "src/lib.rs",
                    "content": "00010| fn target() {\n00011|     work();\n00012| }",
                    "content_format": "line_numbered",
                    "start_line": 10,
                    "end_line": 12,
                    "artifacts": [{
                        "id": "file:src/lib.rs",
                        "kind": "file_span",
                        "content": "large duplicate artifact payload"
                    }]
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 20_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(
            rendered.contains("I will read the narrow target function"),
            "{rendered}"
        );
        assert!(rendered.contains("\"content\""), "{rendered}");
        assert!(rendered.contains("fn target"), "{rendered}");
        assert!(
            !rendered.contains("large duplicate artifact payload"),
            "{rendered}"
        );
        assert!(!rendered.contains("\"artifacts\""), "{rendered}");
    }

    #[test]
    fn take_last_within_bytes_preserves_long_assistant_reasoning_context() {
        let reasoning = format!(
            "I have enough context to edit now. {}",
            "keep this plan visible. ".repeat(500)
        );
        let evidence = json!([{
            "action": "tool_result",
            "assistant": {
                "_air_assistant": {
                    "reasoning": reasoning,
                    "content": "Now I have a complete picture. Let me implement."
                },
                "tool_calls": [{
                    "tool": "todowrite",
                    "input": {"todos": []}
                }]
            },
            "requested": [{
                "tool": "todowrite",
                "input": {"todos": []}
            }],
            "result": [{
                "tool": "todowrite",
                "status": "ok",
                "output": {"todos": []}
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 80_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(
            rendered.contains("Now I have a complete picture. Let me implement."),
            "{rendered}"
        );
        assert!(
            rendered.contains("keep this plan visible. keep this plan visible."),
            "{rendered}"
        );
        assert!(
            rendered.len() > 10_000,
            "assistant reasoning should not be compacted to the generic 2KB string limit"
        );
    }

    #[test]
    fn take_last_within_bytes_preserves_opencode_sized_read_context() {
        let content = "x".repeat(60_000);
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "read",
                "status": "ok",
                "_air_tool_name": "read",
                "_air_tool_call_id": "call_read",
                "input": {
                    "filePath": "src/lib.rs"
                },
                "output": {
                    "path": "src/lib.rs",
                    "content": content,
                    "content_format": "line_numbered",
                    "bytes": 51200,
                    "artifacts": [{
                        "id": "file:src/lib.rs",
                        "kind": "file_span",
                        "content": content
                    }]
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 90_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(rendered.contains("\"content\""), "{rendered}");
        assert!(!rendered.contains("[AIR_COMPACTED]"), "{rendered}");
        assert!(
            rendered.len() > 50_000,
            "read observations should preserve roughly OpenCode's 50KB context budget, got {} bytes",
            rendered.len()
        );
        assert!(
            rendered.len() < 90_000,
            "compacted evidence should respect the requested model window"
        );
        assert!(!rendered.contains("\"artifacts\""), "{rendered}");
        assert!(!rendered.contains("file_span"), "{rendered}");
    }

    #[test]
    fn model_context_compaction_preserves_native_tool_history_metadata() {
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "repo.search",
                "status": "ok",
                "_air_tool_name": "repo_search",
                "_air_tool_call_id": "call_123",
                "input": {"query": "target"},
                "output": {
                    "matches": [{
                        "path": "src/lib.rs",
                        "line_number": 42,
                        "line": "fn target() {}"
                    }]
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 20_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(
            rendered.contains("\"_air_tool_name\":\"repo_search\""),
            "{rendered}"
        );
        assert!(
            rendered.contains("\"_air_tool_call_id\":\"call_123\""),
            "{rendered}"
        );
        assert!(rendered.contains("fn target"), "{rendered}");
    }

    #[test]
    fn model_context_compaction_preserves_artifact_refs_not_bodies() {
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "docs.search",
                "status": "ok",
                "output": {
                    "matches": [{
                        "title": "AIR docs",
                        "snippet": "Use source doc-1"
                    }],
                    "artifacts": [{
                        "id": "doc-1",
                        "kind": "document",
                        "title": "AIR docs",
                        "uri": "air://docs/doc-1",
                        "content": "large artifact body should stay out of model context"
                    }]
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 20_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(rendered.contains("\"artifact_refs\""), "{rendered}");
        assert!(rendered.contains("\"id\":\"doc-1\""), "{rendered}");
        assert!(
            rendered.contains("\"uri\":\"air://docs/doc-1\""),
            "{rendered}"
        );
        assert!(!rendered.contains("large artifact body"), "{rendered}");
    }

    #[test]
    fn model_context_compaction_preserves_post_edit_snippets() {
        let evidence = json!([{
            "action": "tool_result",
            "result": [{
                "tool": "edit",
                "status": "ok",
                "output": {
                    "path": "src/lib.rs",
                    "applied": true,
                    "checked": true,
                    "replacements": 1,
                    "edit_count": 1,
                    "files": [{"path": "src/lib.rs"}],
                    "post_edit_snippets": [{
                        "path": "src/lib.rs",
                        "start_line": 10,
                        "end_line": 14,
                        "content_format": "line_numbered",
                        "content": "00010| fn helper() {}\n00011| "
                    }],
                    "diff": "large diff body should not be required for next model step"
                }
            }]
        }]);

        let compacted = take_last_within_bytes_value(&evidence, 20_000).unwrap();
        let rendered = serde_json::to_string(&compacted).unwrap();

        assert!(rendered.contains("\"post_edit_snippets\""), "{rendered}");
        assert!(rendered.contains("fn helper"), "{rendered}");
        assert!(rendered.contains("\"applied\":true"), "{rendered}");
        assert!(rendered.contains("\"path\":\"src/lib.rs\""), "{rendered}");
        assert!(rendered.contains("\"replacements\":1"), "{rendered}");
        assert!(!rendered.contains("large diff body"), "{rendered}");
    }

    #[test]
    fn tool_batch_summary_resets_stale_verification_after_workspace_change() {
        let summary = tool_batch_summary(&[
            json!({
                "tool": "bash",
                "status": "ok",
                "output": {
                    "verification": true,
                    "success": true
                }
            }),
            json!({
                "tool": "edit",
                "status": "ok",
                "output": {
                    "workspace_changed": true,
                    "success": true
                }
            }),
        ]);

        assert_eq!(summary["any_verification_passed"], json!(true));
        assert_eq!(summary["any_workspace_change"], json!(true));
        assert_eq!(summary["verification_status_update"], json!("unknown"));
    }

    #[test]
    fn tool_batch_summary_allows_verification_after_workspace_change() {
        let summary = tool_batch_summary(&[
            json!({
                "tool": "edit",
                "status": "ok",
                "output": {
                    "workspace_changed": true,
                    "success": true
                }
            }),
            json!({
                "tool": "bash",
                "status": "ok",
                "output": {
                    "verification": true,
                    "success": true
                }
            }),
        ]);

        assert_eq!(summary["any_workspace_change"], json!(true));
        assert_eq!(summary["any_verification_passed"], json!(true));
        assert_eq!(summary["verification_status_update"], json!("passed"));
    }

    #[test]
    fn model_context_truncation_preserves_head_and_tail() {
        let text = "prefix-".to_string() + &"x".repeat(128) + "-suffix";

        let truncated = compact_model_context_string(&text, 48);

        assert_eq!(truncated.chars().count(), 48);
        assert!(truncated.starts_with("prefix"), "{truncated}");
        assert!(truncated.ends_with("suffix"), "{truncated}");
        assert!(truncated.contains("[AIR_COMPACTED]"), "{truncated}");
        assert!(truncated.contains("chars omitted"), "{truncated}");
    }

    #[test]
    fn compact_trace_event_preserves_control_metadata() {
        let event = TraceEvent {
            agent: "agent".to_string(),
            step: 1,
            rule: "rule".to_string(),
            action: "model_call_start".to_string(),
            input: Some(json!({
                "task": "continue\n\nAIR project memory from previous tasks:\n...",
                "large": {"nested": "omitted"}
            })),
            output: None,
            meta: Some(json!({
                "model": "code_edit_decider",
                "output": "decision",
                "attempt": 1,
                "approval_for": ["file.write"],
                "nested": {"large": "omitted"}
            })),
            status: TraceStatus::Ok,
            error: None,
        };

        let compact = compact_trace_event(&event);

        assert_eq!(
            compact.input.as_ref().unwrap()["task"],
            json!("continue\n\nAIR project memory from previous tasks:\n...")
        );
        assert_eq!(
            compact.input.as_ref().unwrap()["_air_truncated"],
            json!(true)
        );
        assert_eq!(
            compact.input.as_ref().unwrap()["large"],
            json!({"_air_truncated": true})
        );
        assert_eq!(
            compact.meta.as_ref().unwrap()["model"],
            json!("code_edit_decider")
        );
        assert_eq!(
            compact.meta.as_ref().unwrap()["approval_for"],
            json!(["file.write"])
        );
        assert_eq!(
            compact.meta.as_ref().unwrap()["_air_truncated"],
            json!(true)
        );
        assert!(compact.meta.as_ref().unwrap().get("nested").is_none());
    }
}
