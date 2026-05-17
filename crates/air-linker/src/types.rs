use air_core::{Diagnostic, Severity, TypeSpec};
use air_runtime::{RuntimeError, State, TraceEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirSystem {
    pub system: SystemMetadata,
    pub modules: BTreeMap<String, ModuleRef>,
    pub entry: String,

    #[serde(default)]
    pub edges: Vec<SystemEdge>,

    #[serde(default)]
    pub connect: Vec<Connection>,

    #[serde(default)]
    pub outputs: BTreeMap<String, String>,

    #[serde(default)]
    pub halts: Vec<SystemHalt>,

    #[serde(default)]
    pub node_conditions: BTreeMap<String, String>,

    #[serde(default)]
    pub schedule: Option<SystemSchedule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleStore {
    pub store: StoreMetadata,

    #[serde(default)]
    pub imports: Vec<PathBuf>,

    pub modules: BTreeMap<String, ModuleRef>,

    #[serde(default)]
    pub recipes: Vec<PlanRecipe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreMetadata {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPlan {
    pub plan: RunPlanMetadata,

    #[serde(default)]
    pub requires: air_core::Requirements,

    pub nodes: Vec<RunPlanNode>,
    pub entry: String,

    #[serde(default)]
    pub edges: Vec<SystemEdge>,

    #[serde(default)]
    pub connect: Vec<Connection>,

    #[serde(default)]
    pub outputs: BTreeMap<String, String>,

    #[serde(default)]
    pub halts: Vec<SystemHalt>,

    #[serde(default)]
    pub decisions: Vec<PlanDecision>,

    #[serde(default)]
    pub schedule: Option<SystemSchedule>,

    #[serde(default)]
    pub dynamic: Option<RunPlanDynamic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSchedule {
    #[serde(default)]
    pub max_parallel: Option<usize>,

    #[serde(default)]
    pub groups: Vec<ScheduleGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleGroup {
    pub id: String,
    pub nodes: Vec<String>,

    #[serde(default)]
    pub max_parallel: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPlanDynamic {
    #[serde(default)]
    pub fanouts: Vec<DynamicFanout>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicFanout {
    pub id: String,
    pub after: String,
    pub source: String,
    pub module: String,
    pub id_prefix: String,
    pub max_items: usize,

    #[serde(default)]
    pub min_items: Option<usize>,

    #[serde(default)]
    pub max_parallel: Option<usize>,

    #[serde(default)]
    pub input: Vec<Connection>,

    #[serde(default)]
    pub fan_in: Vec<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPlanMetadata {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRecipe {
    pub id: String,

    #[serde(default)]
    pub visibility: ModuleVisibility,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub covers: Vec<String>,

    #[serde(default)]
    pub priority: i32,

    pub plan: RunPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPlanNode {
    pub id: String,
    pub module: String,

    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDecision {
    pub id: String,
    pub rationale: String,

    #[serde(default)]
    pub selected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemMetadata {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleRef {
    pub path: PathBuf,

    #[serde(default)]
    pub kind: ModuleKind,

    #[serde(default)]
    pub visibility: ModuleVisibility,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub covers: Vec<String>,

    #[serde(default)]
    pub priority: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModuleKind {
    #[default]
    Primitive,
    Composite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModuleVisibility {
    #[default]
    Public,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<LinkExpr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LinkExpr {
    From { from: String },
    Literal { literal: Value },
    Object { object: BTreeMap<String, LinkExpr> },
    Array { array: Vec<LinkExpr> },
    Count { count: Box<LinkExpr> },
    Coalesce { coalesce: Vec<LinkExpr> },
}

static LINK_TYPE_INTEGER: TypeSpec = TypeSpec::Shorthand(air_core::PrimitiveType::Integer);
static LINK_TYPE_OBJECT: TypeSpec = TypeSpec::Shorthand(air_core::PrimitiveType::Object);
static LINK_TYPE_ARRAY: TypeSpec = TypeSpec::Detailed(air_core::DetailedType {
    kind: air_core::DetailedTypeKind::Array,
    additional_properties: true,
    required: Vec::new(),
    properties: BTreeMap::new(),
    items: None,
    min_items: None,
    max_items: None,
    enum_values: Vec::new(),
});
static LINK_TYPE_STRING: TypeSpec = TypeSpec::Shorthand(air_core::PrimitiveType::String);
static LINK_TYPE_NUMBER: TypeSpec = TypeSpec::Shorthand(air_core::PrimitiveType::Number);
static LINK_TYPE_BOOLEAN: TypeSpec = TypeSpec::Shorthand(air_core::PrimitiveType::Boolean);

pub(crate) fn integer_type() -> &'static TypeSpec {
    &LINK_TYPE_INTEGER
}

pub(crate) fn object_type() -> &'static TypeSpec {
    &LINK_TYPE_OBJECT
}

pub(crate) fn array_type() -> &'static TypeSpec {
    &LINK_TYPE_ARRAY
}

pub(crate) fn json_value_type(value: &Value) -> &'static TypeSpec {
    match value {
        Value::Bool(_) => &LINK_TYPE_BOOLEAN,
        Value::Number(number) if number.as_i64().is_some() || number.as_u64().is_some() => {
            &LINK_TYPE_INTEGER
        }
        Value::Number(_) => &LINK_TYPE_NUMBER,
        Value::String(_) => &LINK_TYPE_STRING,
        Value::Array(_) => &LINK_TYPE_ARRAY,
        Value::Object(_) => &LINK_TYPE_OBJECT,
        Value::Null => &LINK_TYPE_OBJECT,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemHalt {
    pub after: String,
    pub when: HaltCondition,

    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HaltCondition {
    #[serde(rename = "ref")]
    pub reference: String,
    pub equals: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemRunResult {
    pub outputs: State,
    pub module_outputs: BTreeMap<String, State>,
    pub trace: Vec<TraceEvent>,
    pub status: RunStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCheckpoint {
    pub completed_module: String,
    pub module_outputs: BTreeMap<String, State>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceSpecialization {
    pub plan: RunPlan,
    pub cache_identity: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeState {
    pub module_outputs: BTreeMap<String, State>,
    pub output_overrides: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunStatus {
    InProgress {
        after: String,
    },
    Completed,
    Halted {
        after: String,
        reference: String,
        equals: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemVerificationReport {
    pub diagnostics: Vec<Diagnostic>,
}

impl SystemVerificationReport {
    pub fn is_success(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Error)]
pub enum LinkerError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("module {module} path {path} resolves outside module base directory {base_dir}")]
    ModulePathOutsideBase {
        module: String,
        path: String,
        base_dir: String,
    },

    #[error("failed to parse AIR file: {0}")]
    Parse(#[from] air_parser::ParseError),

    #[error("module {module} failed verification: {diagnostics:?}")]
    ModuleVerify {
        module: String,
        diagnostics: Vec<air_core::Diagnostic>,
    },

    #[error("system failed verification: {diagnostics:?}")]
    SystemVerify {
        diagnostics: Vec<air_core::Diagnostic>,
    },

    #[error("run plan failed verification: {diagnostics:?}")]
    RunPlanVerify {
        diagnostics: Vec<air_core::Diagnostic>,
    },

    #[error("unknown module {0}")]
    UnknownModule(String),

    #[error("cycle detected in system DAG")]
    Cycle,

    #[error("invalid endpoint {0}; expected module.field")]
    InvalidEndpoint(String),

    #[error("missing output {field} from module {module}")]
    MissingOutput { module: String, field: String },

    #[error("missing root input {0}")]
    MissingRootInput(String),

    #[error("runtime error in module {module}: {source}")]
    Runtime {
        module: String,
        #[source]
        source: RuntimeError,
    },

    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    #[error("parallel worker panicked")]
    ParallelWorkerPanic,

    #[error("dynamic fanout error: {0}")]
    DynamicFanout(String),

    #[error("trace specialization error: {0}")]
    TraceSpecialization(String),

    #[error("module store import cycle at {0}")]
    StoreImportCycle(String),

    #[error("duplicate module id {module} while importing {import}")]
    DuplicateStoreModule { module: String, import: String },

    #[error("duplicate recipe id {recipe} while importing {import}")]
    DuplicateStoreRecipe { recipe: String, import: String },
}
