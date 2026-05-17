use thiserror::Error;

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

    #[error("tool_batch_dispatch tool {tool} is not in allowed_tools {allowed_tools:?}")]
    ToolNotAllowedByAction {
        tool: String,
        allowed_tools: Vec<String>,
    },

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

    #[error("{action} action cannot write AIR-controlled state fields")]
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
