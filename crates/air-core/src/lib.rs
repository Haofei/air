use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type SchemaMap = BTreeMap<String, TypeSpec>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirModule {
    pub agent: AgentMetadata,

    #[serde(default)]
    pub inputs: SchemaMap,

    #[serde(default)]
    pub outputs: SchemaMap,

    #[serde(default)]
    pub state: SchemaMap,

    #[serde(default)]
    pub requires: Requirements,

    #[serde(default)]
    pub denied: Vec<String>,

    #[serde(default)]
    pub tools: Vec<ToolSpec>,

    pub workflow: Workflow,

    #[serde(default)]
    pub policy: Policy,

    #[serde(default)]
    pub observability: Observability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub name: String,
    pub version: String,

    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Requirements {
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,

    #[serde(default)]
    pub capability: Option<String>,

    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Workflow {
    /// Declarative DAG shape used by linker/system validation.
    ///
    /// The native VM currently executes `state_machine` modules. Compose
    /// multiple modules with RunPlan/AirSystem when you need DAG execution.
    Dag(DagWorkflow),
    StateMachine(StateMachineWorkflow),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagWorkflow {
    pub entry: String,

    #[serde(default)]
    pub nodes: Vec<WorkflowNode>,

    #[serde(default)]
    pub edges: Vec<WorkflowEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateMachineWorkflow {
    pub initial: String,
    pub max_steps: u32,

    #[serde(default)]
    pub rules: Vec<StateRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRule {
    pub id: String,
    pub when: String,

    #[serde(default)]
    pub actions: Vec<StateAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StateAction {
    Set {
        #[serde(default)]
        values: BTreeMap<String, serde_json::Value>,
    },
    Append {
        target: String,
        value: InputSpec,
    },
    ModelCall {
        model: String,
        input: InputSpec,
        output: String,
        timeout_seconds: u64,

        #[serde(default)]
        retry: Option<RetryPolicy>,
    },
    ToolCall {
        tool: String,
        input: InputSpec,
        output: String,
        timeout_seconds: u64,

        #[serde(default)]
        retry: Option<RetryPolicy>,
    },
    ToolBatchDispatch {
        input: InputSpec,
        output: String,
        timeout_seconds: u64,
        max_calls: u32,

        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        allowed_tools: Vec<String>,

        #[serde(default)]
        retry: Option<RetryPolicy>,

        #[serde(default)]
        on_error: ToolErrorMode,
    },
    Approval {
        #[serde(default)]
        approval_for: Vec<String>,
    },
    Return {
        output: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorMode {
    #[default]
    Fail,
    Observe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputSpec {
    Field(String),
    Fields { fields: BTreeMap<String, String> },
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Expr {
    Ref {
        #[serde(rename = "ref")]
        reference: String,
    },
    Path {
        path: String,
    },
    Literal {
        literal: Value,
    },
    Object {
        object: BTreeMap<String, Expr>,
    },
    Array {
        array: Vec<Expr>,
    },
    Template {
        template: String,
    },
    Truncate {
        truncate: Box<Expr>,
        max_chars: usize,
    },
    TakeLast {
        take_last: Box<Expr>,
        max_items: usize,
    },
    TakeLastWithinBytes {
        take_last_within_bytes: Box<Expr>,
        max_bytes: usize,
    },
    SplitLines {
        split_lines: Box<Expr>,
    },
    LineDifference {
        line_difference: Vec<Expr>,
    },
    PathObjects {
        path_objects: Box<Expr>,
    },
    Equals {
        equals: Vec<Expr>,
    },
    IsEmpty {
        is_empty: Box<Expr>,
    },
    Not {
        not: Box<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    pub kind: NodeKind,

    #[serde(default)]
    pub tool: Option<String>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub output: Option<String>,

    #[serde(default)]
    pub approval_for: Vec<String>,

    #[serde(default)]
    pub timeout_seconds: Option<u64>,

    #[serde(default)]
    pub retry: Option<RetryPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    ModelCall,
    ToolCall,
    Approval,
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,

    #[serde(default)]
    pub on_provider_error: Option<ProviderErrorRetryPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderErrorRetryPolicy {
    #[serde(default)]
    pub token_limit: Option<TokenLimitRetryPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenLimitRetryPolicy {
    #[serde(default)]
    pub max_input_chars: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub max_tool_calls: Option<u32>,

    #[serde(default)]
    pub max_model_calls: Option<u32>,

    #[serde(default)]
    pub max_repeated_tool_calls: Option<u32>,

    #[serde(default)]
    pub timeout_seconds: Option<u64>,

    #[serde(default)]
    pub require_approval: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Observability {
    #[serde(default)]
    pub traces: bool,

    #[serde(default)]
    pub metrics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypeSpec {
    Shorthand(PrimitiveType),
    Detailed(DetailedType),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveType {
    String,
    Integer,
    Number,
    Boolean,
    Object,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailedType {
    #[serde(rename = "type")]
    pub kind: DetailedTypeKind,

    #[serde(
        default = "default_additional_properties",
        skip_serializing_if = "is_true"
    )]
    pub additional_properties: bool,

    #[serde(default)]
    pub required: Vec<String>,

    #[serde(default)]
    pub properties: SchemaMap,

    #[serde(default)]
    pub items: Option<Box<TypeSpec>>,

    #[serde(default)]
    pub min_items: Option<usize>,

    #[serde(default)]
    pub max_items: Option<usize>,

    #[serde(default, rename = "enum")]
    pub enum_values: Vec<String>,
}

fn default_additional_properties() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailedTypeKind {
    String,
    Integer,
    Number,
    Boolean,
    Object,
    Array,
    Enum,
}

impl TypeSpec {
    pub fn kind_name(&self) -> &'static str {
        match self {
            TypeSpec::Shorthand(kind) => kind.kind_name(),
            TypeSpec::Detailed(detailed) => detailed.kind.kind_name(),
        }
    }
}

impl PrimitiveType {
    pub fn kind_name(&self) -> &'static str {
        match self {
            PrimitiveType::String => "string",
            PrimitiveType::Integer => "integer",
            PrimitiveType::Number => "number",
            PrimitiveType::Boolean => "boolean",
            PrimitiveType::Object => "object",
        }
    }
}

impl DetailedTypeKind {
    pub fn kind_name(&self) -> &'static str {
        match self {
            DetailedTypeKind::String => "string",
            DetailedTypeKind::Integer => "integer",
            DetailedTypeKind::Number => "number",
            DetailedTypeKind::Boolean => "boolean",
            DetailedTypeKind::Object => "object",
            DetailedTypeKind::Array => "array",
            DetailedTypeKind::Enum => "enum",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
        }
    }
}

pub fn validate_value_against_type(path: &str, value: &Value, spec: &TypeSpec) -> Vec<String> {
    let mut errors = Vec::new();
    validate_value(path, value, spec, &mut errors);
    errors
}

fn validate_value(path: &str, value: &Value, spec: &TypeSpec, errors: &mut Vec<String>) {
    match spec {
        TypeSpec::Shorthand(kind) => validate_primitive(path, value, kind, errors),
        TypeSpec::Detailed(detailed) => validate_detailed(path, value, detailed, errors),
    }
}

fn validate_primitive(path: &str, value: &Value, kind: &PrimitiveType, errors: &mut Vec<String>) {
    let valid = match kind {
        PrimitiveType::String => value.is_string(),
        PrimitiveType::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
        PrimitiveType::Number => value.is_number(),
        PrimitiveType::Boolean => value.is_boolean(),
        PrimitiveType::Object => value.is_object(),
    };

    if !valid {
        errors.push(format!(
            "{path} expected {} got {}",
            kind.kind_name(),
            value_kind(value)
        ));
    }
}

fn validate_detailed(path: &str, value: &Value, spec: &DetailedType, errors: &mut Vec<String>) {
    match spec.kind {
        DetailedTypeKind::String => {
            validate_primitive(path, value, &PrimitiveType::String, errors);
        }
        DetailedTypeKind::Integer => {
            validate_primitive(path, value, &PrimitiveType::Integer, errors);
        }
        DetailedTypeKind::Number => {
            validate_primitive(path, value, &PrimitiveType::Number, errors);
        }
        DetailedTypeKind::Boolean => {
            validate_primitive(path, value, &PrimitiveType::Boolean, errors);
        }
        DetailedTypeKind::Object => validate_object(path, value, spec, errors),
        DetailedTypeKind::Array => validate_array(path, value, spec, errors),
        DetailedTypeKind::Enum => validate_enum(path, value, spec, errors),
    }
}

fn validate_object(path: &str, value: &Value, spec: &DetailedType, errors: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        errors.push(format!("{path} expected object got {}", value_kind(value)));
        return;
    };

    for required in &spec.required {
        if !object.contains_key(required) {
            errors.push(format!("{path}.{required} missing required field"));
        }
    }

    for (field, field_spec) in &spec.properties {
        if let Some(field_value) = object.get(field) {
            validate_value(&format!("{path}.{field}"), field_value, field_spec, errors);
        }
    }

    if !spec.additional_properties {
        for field in object.keys() {
            if !spec.properties.contains_key(field) {
                errors.push(format!("{path}.{field} unexpected additional field"));
            }
        }
    }
}

fn validate_array(path: &str, value: &Value, spec: &DetailedType, errors: &mut Vec<String>) {
    let Some(array) = value.as_array() else {
        errors.push(format!("{path} expected array got {}", value_kind(value)));
        return;
    };

    if let Some(min_items) = spec.min_items {
        if array.len() < min_items {
            errors.push(format!(
                "{path} expected at least {min_items} items got {}",
                array.len()
            ));
        }
    }
    if let Some(max_items) = spec.max_items {
        if array.len() > max_items {
            errors.push(format!(
                "{path} expected at most {max_items} items got {}",
                array.len()
            ));
        }
    }

    if let Some(item_spec) = &spec.items {
        for (index, item) in array.iter().enumerate() {
            validate_value(&format!("{path}[{index}]"), item, item_spec, errors);
        }
    }
}

fn validate_enum(path: &str, value: &Value, spec: &DetailedType, errors: &mut Vec<String>) {
    let Some(value) = value.as_str() else {
        errors.push(format!("{path} expected enum got {}", value_kind(value)));
        return;
    };

    if !spec.enum_values.iter().any(|allowed| allowed == value) {
        errors.push(format!(
            "{path} expected one of [{}] got {value}",
            spec.enum_values.join(", ")
        ));
    }
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Strip the leading namespace prefix (input., state., output., outputs.) from a path.
pub fn normalize_path(path: &str) -> &str {
    path.strip_prefix("input.")
        .or_else(|| path.strip_prefix("state."))
        .or_else(|| path.strip_prefix("output."))
        .or_else(|| path.strip_prefix("outputs."))
        .unwrap_or(path)
}

/// Parse a dot/bracket path like `state.items[0].name` into segments.
pub fn path_segments(path: &str) -> Vec<String> {
    let normalized = normalize_path(path);
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = normalized.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                segments.push(std::mem::take(&mut current));
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
                let mut index = String::new();
                for nested in chars.by_ref() {
                    if nested == ']' {
                        break;
                    }
                    index.push(nested);
                }
                segments.push(index);
                if matches!(chars.peek(), Some('.')) {
                    chars.next();
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() || normalized.ends_with('.') {
        segments.push(current);
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_segments_normalizes_prefixes_and_array_indexes() {
        assert_eq!(
            path_segments("state.observation[0].tool"),
            vec!["observation", "0", "tool"]
        );
        assert_eq!(
            path_segments("outputs.result.items[12]"),
            vec!["result", "items", "12"]
        );
        assert_eq!(
            path_segments("input.customer.issue"),
            vec!["customer", "issue"]
        );
    }
}
