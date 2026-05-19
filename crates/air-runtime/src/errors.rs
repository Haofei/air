use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("provider error: {0}")]
    Provider(String),

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
