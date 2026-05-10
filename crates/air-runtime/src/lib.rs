use air_core::{
    validate_value_against_type, AirModule, Expr, InputSpec, StateAction, TypeSpec, Workflow,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};
use thiserror::Error;

pub type State = Map<String, Value>;

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

    #[error("provider error: {0}")]
    Provider(String),

    #[error("policy.max_tool_calls exceeded: limit={limit} attempted={attempted}")]
    ToolCallLimitExceeded { limit: u32, attempted: u32 },

    #[error("policy.max_model_calls exceeded: limit={limit} attempted={attempted}")]
    ModelCallLimitExceeded { limit: u32, attempted: u32 },

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
            text = text.chars().take(max_chars).collect::<String>();
            text.push_str("[AIR_TRUNCATED]");
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
    compact.input = compact
        .input
        .as_ref()
        .map(|_| json!({"_air_truncated": true}));
    compact.output = compact
        .output
        .as_ref()
        .map(|_| json!({"_air_truncated": true}));
    compact.meta = compact
        .meta
        .as_ref()
        .map(|_| json!({"_air_truncated": true}));
    compact.error = compact
        .error
        .as_ref()
        .map(|_| "[AIR_TRUNCATED]".to_string());
    compact
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
        let mut artifact_registry = collect_initial_artifact_ids(&Value::Object(state.clone()));
        let module_started_at = Instant::now();

        for step in 0..workflow.max_steps {
            let phase = state
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            if workflow.terminal.iter().any(|terminal| terminal == &phase) {
                if let Some(rule) = workflow
                    .rules
                    .iter()
                    .find(|rule| condition_matches(&state, &outputs, &rule.when).unwrap_or(false))
                {
                    for action in &rule.actions {
                        let mut context = ExecutionContext {
                            module,
                            state: &mut state,
                            outputs: &mut outputs,
                            trace: &mut trace,
                            observer: &mut observer,
                            model_calls: &mut model_calls,
                            tool_calls: &mut tool_calls,
                            artifact_registry: &mut artifact_registry,
                            module_started_at,
                            step,
                            rule: &rule.id,
                        };
                        self.execute_action(&mut context, action)?;
                    }
                }
                return Ok(RunResult { outputs, trace });
            }

            let mut matched = false;
            for rule in &workflow.rules {
                if condition_matches(&state, &outputs, &rule.when)? {
                    matched = true;
                    for action in &rule.actions {
                        let mut context = ExecutionContext {
                            module,
                            state: &mut state,
                            outputs: &mut outputs,
                            trace: &mut trace,
                            observer: &mut observer,
                            model_calls: &mut model_calls,
                            tool_calls: &mut tool_calls,
                            artifact_registry: &mut artifact_registry,
                            module_started_at,
                            step,
                            rule: &rule.id,
                        };
                        self.execute_action(&mut context, action)?;
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
                    if let Some(limit) = context.module.policy.max_model_calls {
                        let attempted = *context.model_calls + 1;
                        if attempted > limit {
                            let error = RuntimeError::ModelCallLimitExceeded { limit, attempted };
                            context.push_event_with_meta(
                                "model_call",
                                Some(attempt_input),
                                None,
                                Some(model_call_result_meta(
                                    model,
                                    attempt,
                                    max_attempts,
                                    output,
                                    *timeout_seconds,
                                    0,
                                    false,
                                )),
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
                        Some(model_call_meta(
                            model,
                            attempt,
                            max_attempts,
                            output,
                            *timeout_seconds,
                        )),
                        Ok(()),
                    );
                    let attempt_started_at = Instant::now();
                    let result = match self.models.call_model_with_timeout(
                        model,
                        &attempt_input,
                        Duration::from_secs(*timeout_seconds),
                    ) {
                        Ok(result) => result,
                        Err(error) => {
                            let is_final_attempt = attempt == max_attempts;
                            let error_message = error.to_string();
                            context.push_event_with_meta(
                                "model_call",
                                Some(attempt_input),
                                None,
                                Some(model_call_result_meta(
                                    model,
                                    attempt,
                                    max_attempts,
                                    output,
                                    *timeout_seconds,
                                    attempt_started_at.elapsed().as_millis(),
                                    !is_final_attempt,
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
                        let is_final_attempt = attempt == max_attempts;
                        let error_message = error.to_string();
                        context.push_event_with_meta(
                            "model_call",
                            Some(attempt_input),
                            Some(result),
                            Some(model_call_result_meta(
                                model,
                                attempt,
                                max_attempts,
                                output,
                                *timeout_seconds,
                                attempt_started_at.elapsed().as_millis(),
                                !is_final_attempt,
                            )),
                            Err(error_message.clone()),
                        );
                        if is_final_attempt {
                            return Err(error);
                        }
                        retry_error = Some(RetryError::Message(error_message));
                        continue;
                    }
                    if let Err(error) = validate_output(context.module, output, &result) {
                        let is_final_attempt = attempt == max_attempts;
                        let error_message = error.to_string();
                        context.push_event_with_meta(
                            "model_call",
                            Some(attempt_input),
                            Some(result),
                            Some(model_call_result_meta(
                                model,
                                attempt,
                                max_attempts,
                                output,
                                *timeout_seconds,
                                attempt_started_at.elapsed().as_millis(),
                                !is_final_attempt,
                            )),
                            Err(error_message.clone()),
                        );
                        if is_final_attempt {
                            return Err(error);
                        }
                        retry_error = Some(RetryError::Message(error_message));
                        continue;
                    }
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
                            Some(model_call_result_meta(
                                model,
                                attempt,
                                max_attempts,
                                output,
                                *timeout_seconds,
                                attempt_started_at.elapsed().as_millis(),
                                !is_final_attempt,
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
                        Some(model_call_result_meta(
                            model,
                            attempt,
                            max_attempts,
                            output,
                            *timeout_seconds,
                            attempt_started_at.elapsed().as_millis(),
                            false,
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
                reject_control_field_write("tool_call", output)?;
                if let Err(error) = validate_tool_capability(context.module, tool, &self.tools) {
                    context.push_event_with_meta(
                        "tool_call",
                        None,
                        None,
                        Some(json!({"tool": tool})),
                        Err(error.to_string()),
                    );
                    return Err(error);
                }
                let input = resolve_input(context.state, context.outputs, input)?;
                let max_attempts = retry
                    .as_ref()
                    .map(|retry| retry.max_attempts)
                    .unwrap_or(1)
                    .max(1);
                for attempt in 1..=max_attempts {
                    if let Some(limit) = context.module.policy.max_tool_calls {
                        let attempted = *context.tool_calls + 1;
                        if attempted > limit {
                            let error = RuntimeError::ToolCallLimitExceeded { limit, attempted };
                            context.push_event_with_meta(
                                "tool_call",
                                Some(input),
                                None,
                                Some(tool_call_result_meta(
                                    tool,
                                    attempt,
                                    max_attempts,
                                    output,
                                    *timeout_seconds,
                                    0,
                                    false,
                                )),
                                Err(error.to_string()),
                            );
                            return Err(error);
                        }
                    }
                    *context.tool_calls += 1;
                    context.push_event_with_meta(
                        "tool_call_start",
                        Some(input.clone()),
                        None,
                        Some(tool_call_meta(
                            tool,
                            attempt,
                            max_attempts,
                            output,
                            *timeout_seconds,
                        )),
                        Ok(()),
                    );
                    let attempt_started_at = Instant::now();
                    let result = match self.tools.call_tool_with_timeout(
                        tool,
                        &input,
                        Duration::from_secs(*timeout_seconds),
                    ) {
                        Ok(result) => result,
                        Err(error) => {
                            let is_final_attempt = attempt == max_attempts;
                            context.push_event_with_meta(
                                "tool_call",
                                Some(input.clone()),
                                None,
                                Some(tool_call_result_meta(
                                    tool,
                                    attempt,
                                    max_attempts,
                                    output,
                                    *timeout_seconds,
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
                        action_timeout_error("tool_call", *timeout_seconds, attempt_started_at)
                    {
                        let is_final_attempt = attempt == max_attempts;
                        context.push_event_with_meta(
                            "tool_call",
                            Some(input.clone()),
                            Some(result),
                            Some(tool_call_result_meta(
                                tool,
                                attempt,
                                max_attempts,
                                output,
                                *timeout_seconds,
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
                            "tool_call",
                            Some(input.clone()),
                            Some(result),
                            Some(tool_call_result_meta(
                                tool,
                                attempt,
                                max_attempts,
                                output,
                                *timeout_seconds,
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
                    context.state.insert(output.clone(), result);
                    let mut meta = tool_call_result_meta(
                        tool,
                        attempt,
                        max_attempts,
                        output,
                        *timeout_seconds,
                        attempt_started_at.elapsed().as_millis(),
                        false,
                    );
                    insert_artifact_result_meta(&mut meta, &artifact_ids);
                    context.push_event_with_meta(
                        "tool_call",
                        Some(input),
                        context.state.get(output).cloned(),
                        Some(meta),
                        Ok(()),
                    );
                    return Ok(());
                }
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
}

fn validate_output(module: &AirModule, output: &str, value: &Value) -> Result<(), RuntimeError> {
    validate_named_value(output, value, output_spec(module, output))
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
    TokenLimit(String),
}

impl RetryError {
    fn from_runtime_error(error: &RuntimeError) -> Self {
        match error {
            RuntimeError::Provider(message) if is_token_limit_error(message) => {
                RetryError::TokenLimit(message.clone())
            }
            other => RetryError::Message(other.to_string()),
        }
    }

    fn message(&self) -> &str {
        match self {
            RetryError::Message(message) | RetryError::TokenLimit(message) => message,
        }
    }

    fn reason(&self) -> &'static str {
        match self {
            RetryError::Message(_) => "previous_error",
            RetryError::TokenLimit(_) => "token_limit",
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
        RetryError::Message(_) => None,
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
        Value::String(
            "The previous response failed AIR runtime schema validation. Return only a corrected JSON object matching required_output_schema exactly.".to_string(),
        ),
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
            let truncated = text.chars().take(*remaining).collect::<String>();
            *remaining = 0;
            Value::String(format!("{truncated}\n[air:truncated]"))
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

struct ExecutionContext<'a> {
    module: &'a AirModule,
    state: &'a mut State,
    outputs: &'a mut State,
    trace: &'a mut Vec<TraceEvent>,
    observer: &'a mut dyn FnMut(&TraceEvent),
    model_calls: &'a mut u32,
    tool_calls: &'a mut u32,
    artifact_registry: &'a mut BTreeSet<String>,
    module_started_at: Instant,
    step: u32,
    rule: &'a str,
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

fn model_call_meta(
    model: &str,
    attempt: u32,
    max_attempts: u32,
    output: &str,
    timeout_seconds: u64,
) -> Value {
    serde_json::json!({
        "model": model,
        "attempt": attempt,
        "max_attempts": max_attempts,
        "output": output,
        "timeout_seconds": timeout_seconds,
    })
}

fn model_call_result_meta(
    model: &str,
    attempt: u32,
    max_attempts: u32,
    output: &str,
    timeout_seconds: u64,
    elapsed_ms: u128,
    will_retry: bool,
) -> Value {
    let mut meta = model_call_meta(model, attempt, max_attempts, output, timeout_seconds);
    insert_result_meta(&mut meta, elapsed_ms, will_retry);
    meta
}

fn tool_call_meta(
    tool: &str,
    attempt: u32,
    max_attempts: u32,
    output: &str,
    timeout_seconds: u64,
) -> Value {
    serde_json::json!({
        "tool": tool,
        "attempt": attempt,
        "max_attempts": max_attempts,
        "output": output,
        "timeout_seconds": timeout_seconds,
    })
}

fn tool_call_result_meta(
    tool: &str,
    attempt: u32,
    max_attempts: u32,
    output: &str,
    timeout_seconds: u64,
    elapsed_ms: u128,
    will_retry: bool,
) -> Value {
    let mut meta = tool_call_meta(tool, attempt, max_attempts, output, timeout_seconds);
    insert_result_meta(&mut meta, elapsed_ms, will_retry);
    meta
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
            "ref" | "path" | "literal" | "object" | "array" | "template" | "truncate"
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
    }
}

fn read_path<'a>(
    state: &'a State,
    outputs: &'a State,
    path: &str,
) -> Result<&'a Value, RuntimeError> {
    let normalized = normalize_path(path);
    let mut segments = normalized.split('.');
    let Some(first) = segments.next().filter(|segment| !segment.is_empty()) else {
        return Err(RuntimeError::MissingField(path.to_string()));
    };

    let mut value = read_field(state, outputs, first)?;
    for segment in segments {
        let Some(object) = value.as_object() else {
            return Err(RuntimeError::MissingField(path.to_string()));
        };
        value = object
            .get(segment)
            .ok_or_else(|| RuntimeError::MissingField(path.to_string()))?;
    }

    Ok(value)
}

fn normalize_path(path: &str) -> &str {
    path.strip_prefix("input.")
        .or_else(|| path.strip_prefix("state."))
        .or_else(|| path.strip_prefix("output."))
        .or_else(|| path.strip_prefix("outputs."))
        .unwrap_or(path)
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
    let (left, right, expected_equal) = if let Some((left, right)) = clause.split_once("==") {
        (left, right, true)
    } else if let Some((left, right)) = clause.split_once("!=") {
        (left, right, false)
    } else {
        return Err(RuntimeError::UnsupportedCondition(clause.to_string()));
    };

    let actual = read_path(state, outputs, left.trim())?;
    let expected = parse_condition_literal(right.trim())?;
    Ok((actual == &expected) == expected_equal)
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
