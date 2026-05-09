#![allow(clippy::too_many_arguments)]

use air_core::{AirModule, Diagnostic, Severity, TypeSpec};
use air_runtime::{
    ModelProvider, RunResult, RuntimeError, State, ToolProvider, TraceEvent, TraceStatus, Vm,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
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
    required: Vec::new(),
    properties: BTreeMap::new(),
    items: None,
    min_items: None,
    max_items: None,
    enum_values: Vec::new(),
});

fn integer_type() -> &'static TypeSpec {
    &LINK_TYPE_INTEGER
}

fn object_type() -> &'static TypeSpec {
    &LINK_TYPE_OBJECT
}

fn array_type() -> &'static TypeSpec {
    &LINK_TYPE_ARRAY
}

fn json_value_type(value: &Value) -> &'static TypeSpec {
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

static LINK_TYPE_STRING: TypeSpec = TypeSpec::Shorthand(air_core::PrimitiveType::String);
static LINK_TYPE_NUMBER: TypeSpec = TypeSpec::Shorthand(air_core::PrimitiveType::Number);
static LINK_TYPE_BOOLEAN: TypeSpec = TypeSpec::Shorthand(air_core::PrimitiveType::Boolean);

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

    #[error("failed to parse AIR system YAML: {0}")]
    Parse(#[from] serde_yaml::Error),

    #[error("failed to parse AIR module: {0}")]
    ModuleParse(#[from] air_parser::ParseError),

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
}

pub fn parse_system_file(path: impl AsRef<Path>) -> Result<AirSystem, LinkerError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| LinkerError::Read {
        path: path.display().to_string(),
        source,
    })?;
    Ok(serde_yaml::from_str(&source)?)
}

pub fn parse_module_store_file(path: impl AsRef<Path>) -> Result<ModuleStore, LinkerError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| LinkerError::Read {
        path: path.display().to_string(),
        source,
    })?;
    Ok(serde_yaml::from_str(&source)?)
}

pub fn parse_run_plan_file(path: impl AsRef<Path>) -> Result<RunPlan, LinkerError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| LinkerError::Read {
        path: path.display().to_string(),
        source,
    })?;
    Ok(serde_yaml::from_str(&source)?)
}

pub fn validate_system(system: &AirSystem, base_dir: impl AsRef<Path>) -> SystemVerificationReport {
    let mut verifier = SystemVerifier::default();
    verifier.verify(system, base_dir.as_ref());
    SystemVerificationReport {
        diagnostics: verifier.diagnostics,
    }
}

pub fn validate_run_plan(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
) -> SystemVerificationReport {
    let mut verifier = RunPlanVerifier::default();
    verifier.verify(plan, store);
    if verifier
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return SystemVerificationReport {
            diagnostics: verifier.diagnostics,
        };
    }

    if let Some(dynamic_report) = validate_dynamic_run_plan(plan, store, base_dir.as_ref()) {
        verifier.diagnostics.extend(dynamic_report.diagnostics);
    }
    if let Some(capability_report) = validate_run_plan_capabilities(plan, store, base_dir.as_ref())
    {
        verifier.diagnostics.extend(capability_report.diagnostics);
    }
    if verifier
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return SystemVerificationReport {
            diagnostics: verifier.diagnostics,
        };
    }

    match resolve_run_plan_for_validation(plan, store) {
        Ok(system) => {
            verifier
                .diagnostics
                .extend(validate_system(&system, base_dir).diagnostics);
        }
        Err(error) => verifier.error("AIRP020", error.to_string()),
    }

    SystemVerificationReport {
        diagnostics: verifier.diagnostics,
    }
}

pub fn run_system<T, M>(
    system: &AirSystem,
    base_dir: impl AsRef<Path>,
    inputs: State,
    tools: T,
    models: M,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
{
    run_system_with_observer(system, base_dir, inputs, tools, models, |_| {})
}

pub fn run_run_plan<T, M>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
    inputs: State,
    tools: T,
    models: M,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
{
    run_run_plan_with_observer(plan, store, base_dir, inputs, tools, models, |_| {})
}

pub fn run_run_plan_with_observer<T, M, F>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
    inputs: State,
    tools: T,
    models: M,
    observer: F,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
{
    run_run_plan_with_observer_and_checkpoint(
        plan,
        store,
        base_dir,
        inputs,
        tools,
        models,
        observer,
        |_| Ok(()),
    )
}

pub fn run_run_plan_with_observer_and_checkpoint<T, M, F, C>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
    inputs: State,
    tools: T,
    models: M,
    mut observer: F,
    mut checkpoint: C,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let base_dir = base_dir.as_ref();
    let verification = validate_run_plan(plan, store, base_dir);
    if !verification.is_success() {
        return Err(LinkerError::RunPlanVerify {
            diagnostics: verification.diagnostics,
        });
    }

    if has_dynamic_fanouts(plan) {
        return run_dynamic_run_plan_with_observer_and_checkpoint(
            plan, store, base_dir, inputs, tools, models, observer, checkpoint,
        );
    }

    let system = resolve_run_plan(plan, store)?;
    let mut trace = plan_trace_events(plan);
    for event in &trace {
        observer(event);
    }

    let mut result = run_system_with_resume_with_observer_and_checkpoint(
        &system,
        base_dir,
        inputs,
        ResumeState::default(),
        tools,
        models,
        |event| observer(event),
        |checkpoint_state| checkpoint(checkpoint_state),
    )?;
    trace.append(&mut result.trace);
    result.trace = trace;
    Ok(result)
}

pub fn run_run_plan_parallel_with_observer_and_checkpoint<T, M, TF, MF, F, C>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
    inputs: State,
    tools_factory: TF,
    models_factory: MF,
    mut observer: F,
    mut checkpoint: C,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let base_dir = base_dir.as_ref();
    let verification = validate_run_plan(plan, store, base_dir);
    if !verification.is_success() {
        return Err(LinkerError::RunPlanVerify {
            diagnostics: verification.diagnostics,
        });
    }

    if has_dynamic_fanouts(plan) {
        return run_dynamic_run_plan_parallel_with_observer_and_checkpoint(
            plan,
            store,
            base_dir,
            inputs,
            tools_factory,
            models_factory,
            observer,
            checkpoint,
        );
    }

    let system = resolve_run_plan(plan, store)?;
    let mut trace = plan_trace_events(plan);
    for event in &trace {
        observer(event);
    }

    let mut result = run_system_parallel_with_resume_with_observer_and_checkpoint(
        &system,
        base_dir,
        inputs,
        ResumeState::default(),
        tools_factory,
        models_factory,
        |event| observer(event),
        |checkpoint_state| checkpoint(checkpoint_state),
    )?;
    trace.append(&mut result.trace);
    result.trace = trace;
    Ok(result)
}

pub fn resume_run_plan<T, M>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
    inputs: State,
    resume: ResumeState,
    tools: T,
    models: M,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
{
    resume_run_plan_with_observer(plan, store, base_dir, inputs, resume, tools, models, |_| {})
}

pub fn resume_run_plan_with_observer<T, M, F>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
    inputs: State,
    resume: ResumeState,
    tools: T,
    models: M,
    observer: F,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
{
    resume_run_plan_with_observer_and_checkpoint(
        plan,
        store,
        base_dir,
        inputs,
        resume,
        tools,
        models,
        observer,
        |_| Ok(()),
    )
}

pub fn resume_run_plan_with_observer_and_checkpoint<T, M, F, C>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
    inputs: State,
    resume: ResumeState,
    tools: T,
    models: M,
    mut observer: F,
    mut checkpoint: C,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let base_dir = base_dir.as_ref();
    let verification = validate_run_plan(plan, store, base_dir);
    if !verification.is_success() {
        return Err(LinkerError::RunPlanVerify {
            diagnostics: verification.diagnostics,
        });
    }

    if has_dynamic_fanouts(plan) {
        return resume_dynamic_run_plan_with_observer_and_checkpoint(
            plan, store, base_dir, inputs, resume, tools, models, observer, checkpoint,
        );
    }

    let system = resolve_run_plan(plan, store)?;
    let mut trace = plan_trace_events(plan);
    trace.push(TraceEvent {
        agent: "$planner".to_string(),
        step: trace.len() as u32,
        rule: "run_plan".to_string(),
        action: "resume".to_string(),
        input: Some(json!({
            "completed_modules": resume.module_outputs.keys().collect::<Vec<_>>(),
            "overrides": resume.output_overrides.keys().collect::<Vec<_>>(),
        })),
        output: None,
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    });
    for event in &trace {
        observer(event);
    }

    let mut result = run_system_with_resume_with_observer_and_checkpoint(
        &system,
        base_dir,
        inputs,
        resume,
        tools,
        models,
        |event| observer(event),
        |checkpoint_state| checkpoint(checkpoint_state),
    )?;
    trace.append(&mut result.trace);
    result.trace = trace;
    Ok(result)
}

pub fn specialize_run_plan_trace(
    events: &[TraceEvent],
    store: &ModuleStore,
    base_dir: impl AsRef<Path>,
) -> Result<TraceSpecialization, LinkerError> {
    let base_dir = base_dir.as_ref();
    let output = events
        .iter()
        .rev()
        .find(|event| {
            event.agent == "$planner"
                && event.rule == "dynamic_fanout"
                && event.action == "resolve_dynamic_plan"
                && event.status == TraceStatus::Ok
        })
        .or_else(|| {
            events.iter().find(|event| {
                event.agent == "$planner"
                    && event.rule == "run_plan"
                    && event.action == "resolve_plan"
                    && event.status == TraceStatus::Ok
            })
        })
        .and_then(|event| event.output.clone())
        .ok_or_else(|| {
            LinkerError::TraceSpecialization(
                "trace does not contain a planner resolve_plan event".to_string(),
            )
        })?;

    let plan: RunPlan = serde_json::from_value(output).map_err(|error| {
        LinkerError::TraceSpecialization(format!(
            "planner resolve_plan output is not a RunPlan: {error}"
        ))
    })?;

    let verification = validate_run_plan(&plan, store, base_dir);
    if !verification.is_success() {
        return Err(LinkerError::RunPlanVerify {
            diagnostics: verification.diagnostics,
        });
    }

    let cache_identity = trace_specialization_identity(&plan, store, base_dir)?;
    Ok(TraceSpecialization {
        plan,
        cache_identity,
    })
}

pub fn run_system_with_observer<T, M, F>(
    system: &AirSystem,
    base_dir: impl AsRef<Path>,
    inputs: State,
    tools: T,
    models: M,
    observer: F,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
{
    run_system_with_resume_with_observer(
        system,
        base_dir,
        inputs,
        ResumeState::default(),
        tools,
        models,
        observer,
    )
}

fn run_dynamic_run_plan_with_observer_and_checkpoint<T, M, F, C>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: &Path,
    inputs: State,
    tools: T,
    models: M,
    mut observer: F,
    mut checkpoint: C,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let Some(dynamic) = &plan.dynamic else {
        unreachable!("dynamic runner called without dynamic fanouts");
    };
    if dynamic.fanouts.is_empty() {
        return Err(LinkerError::DynamicFanout(
            "dynamic.fanouts is empty".to_string(),
        ));
    }

    let base_system = resolve_run_plan(plan, store)?;
    let modules = load_modules(&base_system, base_dir)?;
    let order = topological_order(&base_system)?;

    let mut trace = plan_trace_events(plan);
    for event in &trace {
        observer(event);
    }

    let mut vm = Vm { tools, models };
    let mut module_outputs = BTreeMap::new();
    let mut skipped_modules = BTreeSet::new();
    let mut materialized: BTreeMap<String, MaterializedFanout> = BTreeMap::new();

    for module_id in order {
        if module_outputs.contains_key(&module_id) {
            continue;
        }
        let current_plan = materialize_partial_dynamic_run_plan(plan, &materialized)?;
        let current_system = resolve_run_plan(&current_plan, store)?;
        let incoming = incoming_connections(&current_system)?;

        if let Some(condition) = current_system.node_conditions.get(&module_id) {
            if !link_condition_matches(condition, &inputs, &module_outputs)? {
                let event = TraceEvent {
                    agent: "$system".to_string(),
                    step: trace.len() as u32,
                    rule: "node_condition".to_string(),
                    action: "skip".to_string(),
                    input: Some(json!({
                        "node": module_id,
                        "when": condition,
                    })),
                    output: None,
                    meta: None,
                    status: TraceStatus::Ok,
                    error: None,
                };
                observer(&event);
                trace.push(event);
                skipped_modules.insert(module_id);
                continue;
            }
        }

        let module = modules
            .get(&module_id)
            .ok_or_else(|| LinkerError::UnknownModule(module_id.clone()))?;
        let module_inputs = build_module_inputs(
            &module_id,
            &inputs,
            &module_outputs,
            &incoming,
            &skipped_modules,
        )?;
        let result = vm
            .run_with_observer(module, module_inputs, |event| observer(event))
            .map_err(|source| LinkerError::Runtime {
                module: module_id.clone(),
                source,
            })?;
        trace.extend(result.trace);
        module_outputs.insert(module_id.clone(), result.outputs);
        checkpoint(&SystemCheckpoint {
            completed_module: module_id.clone(),
            module_outputs: module_outputs.clone(),
        })
        .map_err(LinkerError::Checkpoint)?;

        if let Some(halt) = triggered_halt(&current_system, &module_id, &module_outputs)? {
            let outputs = if halt.outputs.is_empty() {
                collect_system_outputs(&current_system, &module_outputs)?
            } else {
                collect_outputs(&halt.outputs, &module_outputs)?
            };
            let event = TraceEvent {
                agent: "$system".to_string(),
                step: trace.len() as u32,
                rule: "halt".to_string(),
                action: "halt".to_string(),
                input: Some(json!({
                    "after": halt.after,
                    "ref": halt.when.reference,
                    "equals": halt.when.equals,
                })),
                output: Some(serde_json::Value::Object(outputs.clone())),
                meta: None,
                status: TraceStatus::Ok,
                error: None,
            };
            observer(&event);
            trace.push(event);
            return Ok(SystemRunResult {
                outputs,
                module_outputs,
                trace,
                status: RunStatus::Halted {
                    after: halt.after.clone(),
                    reference: halt.when.reference.clone(),
                    equals: halt.when.equals.clone(),
                },
            });
        }

        for fanout in pending_dynamic_after(dynamic, &materialized, &module_id) {
            let created = run_dynamic_fanout(
                fanout,
                store,
                base_dir,
                &inputs,
                &mut module_outputs,
                &mut vm,
                &mut trace,
                &mut observer,
                &mut checkpoint,
            )?;
            materialized.insert(fanout.id.clone(), created);
            run_child_dynamic_fanouts(
                dynamic,
                &fanout.id,
                store,
                base_dir,
                &inputs,
                &mut module_outputs,
                &mut vm,
                &mut materialized,
                &mut trace,
                &mut observer,
                &mut checkpoint,
            )?;
        }
    }

    let missing = missing_dynamic_fanouts(dynamic, &materialized);
    if !missing.is_empty() {
        return Err(LinkerError::DynamicFanout(format!(
            "dynamic fanout(s) were not materialized: {}",
            missing.join(", ")
        )));
    }

    let final_plan =
        push_resolved_dynamic_plan_event(plan, &materialized, &mut trace, &mut observer)?;
    let final_system = resolve_run_plan(&final_plan, store)?;
    let outputs = collect_system_outputs(&final_system, &module_outputs)?;
    Ok(SystemRunResult {
        outputs,
        module_outputs,
        trace,
        status: RunStatus::Completed,
    })
}

fn resume_dynamic_run_plan_with_observer_and_checkpoint<T, M, F, C>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: &Path,
    inputs: State,
    resume: ResumeState,
    tools: T,
    models: M,
    mut observer: F,
    mut checkpoint: C,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let Some(dynamic) = &plan.dynamic else {
        unreachable!("dynamic resume called without dynamic fanouts");
    };
    if dynamic.fanouts.is_empty() {
        return Err(LinkerError::DynamicFanout(
            "dynamic.fanouts is empty".to_string(),
        ));
    }

    let base_system = resolve_run_plan(plan, store)?;
    let modules = load_modules(&base_system, base_dir)?;
    let order = topological_order(&base_system)?;

    let mut trace = plan_trace_events(plan);
    trace.push(TraceEvent {
        agent: "$planner".to_string(),
        step: trace.len() as u32,
        rule: "run_plan".to_string(),
        action: "resume".to_string(),
        input: Some(json!({
            "completed_modules": resume.module_outputs.keys().collect::<Vec<_>>(),
            "overrides": resume.output_overrides.keys().collect::<Vec<_>>(),
        })),
        output: None,
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    });
    for event in &trace {
        observer(event);
    }

    let mut vm = Vm { tools, models };
    let mut module_outputs = resume.module_outputs;
    apply_output_overrides(&mut module_outputs, &resume.output_overrides)?;
    let mut skipped_modules = BTreeSet::new();
    let mut materialized: BTreeMap<String, MaterializedFanout> = BTreeMap::new();

    for module_id in order {
        let current_plan = materialize_partial_dynamic_run_plan(plan, &materialized)?;
        let current_system = resolve_run_plan(&current_plan, store)?;
        let incoming = incoming_connections(&current_system)?;
        let was_completed = module_outputs.contains_key(&module_id);
        if !was_completed {
            if let Some(condition) = current_system.node_conditions.get(&module_id) {
                if !link_condition_matches(condition, &inputs, &module_outputs)? {
                    let event = TraceEvent {
                        agent: "$system".to_string(),
                        step: trace.len() as u32,
                        rule: "node_condition".to_string(),
                        action: "skip".to_string(),
                        input: Some(json!({
                            "node": module_id,
                            "when": condition,
                        })),
                        output: None,
                        meta: None,
                        status: TraceStatus::Ok,
                        error: None,
                    };
                    observer(&event);
                    trace.push(event);
                    skipped_modules.insert(module_id);
                    continue;
                }
            }

            let module = modules
                .get(&module_id)
                .ok_or_else(|| LinkerError::UnknownModule(module_id.clone()))?;
            let module_inputs = build_module_inputs(
                &module_id,
                &inputs,
                &module_outputs,
                &incoming,
                &skipped_modules,
            )?;
            let result = vm
                .run_with_observer(module, module_inputs, |event| observer(event))
                .map_err(|source| LinkerError::Runtime {
                    module: module_id.clone(),
                    source,
                })?;
            trace.extend(result.trace);
            module_outputs.insert(module_id.clone(), result.outputs);
            checkpoint(&SystemCheckpoint {
                completed_module: module_id.clone(),
                module_outputs: module_outputs.clone(),
            })
            .map_err(LinkerError::Checkpoint)?;
        }

        if let Some(halt) = triggered_halt(&current_system, &module_id, &module_outputs)? {
            let outputs = if halt.outputs.is_empty() {
                collect_system_outputs(&current_system, &module_outputs)?
            } else {
                collect_outputs(&halt.outputs, &module_outputs)?
            };
            let event = TraceEvent {
                agent: "$system".to_string(),
                step: trace.len() as u32,
                rule: "halt".to_string(),
                action: "halt".to_string(),
                input: Some(json!({
                    "after": halt.after,
                    "ref": halt.when.reference,
                    "equals": halt.when.equals,
                })),
                output: Some(serde_json::Value::Object(outputs.clone())),
                meta: None,
                status: TraceStatus::Ok,
                error: None,
            };
            observer(&event);
            trace.push(event);
            return Ok(SystemRunResult {
                outputs,
                module_outputs,
                trace,
                status: RunStatus::Halted {
                    after: halt.after.clone(),
                    reference: halt.when.reference.clone(),
                    equals: halt.when.equals.clone(),
                },
            });
        }

        for fanout in pending_dynamic_after(dynamic, &materialized, &module_id) {
            let created = run_dynamic_fanout(
                fanout,
                store,
                base_dir,
                &inputs,
                &mut module_outputs,
                &mut vm,
                &mut trace,
                &mut observer,
                &mut checkpoint,
            )?;
            materialized.insert(fanout.id.clone(), created);
            run_child_dynamic_fanouts(
                dynamic,
                &fanout.id,
                store,
                base_dir,
                &inputs,
                &mut module_outputs,
                &mut vm,
                &mut materialized,
                &mut trace,
                &mut observer,
                &mut checkpoint,
            )?;
        }
    }

    let missing = missing_dynamic_fanouts(dynamic, &materialized);
    if !missing.is_empty() {
        return Err(LinkerError::DynamicFanout(format!(
            "dynamic fanout(s) were not materialized: {}",
            missing.join(", ")
        )));
    }

    let final_plan =
        push_resolved_dynamic_plan_event(plan, &materialized, &mut trace, &mut observer)?;
    let final_system = resolve_run_plan(&final_plan, store)?;
    let outputs = collect_system_outputs(&final_system, &module_outputs)?;
    Ok(SystemRunResult {
        outputs,
        module_outputs,
        trace,
        status: RunStatus::Completed,
    })
}

fn run_dynamic_run_plan_parallel_with_observer_and_checkpoint<T, M, TF, MF, F, C>(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: &Path,
    inputs: State,
    tools_factory: TF,
    models_factory: MF,
    mut observer: F,
    mut checkpoint: C,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let Some(dynamic) = &plan.dynamic else {
        unreachable!("dynamic parallel runner called without dynamic fanouts");
    };
    if dynamic.fanouts.is_empty() {
        return Err(LinkerError::DynamicFanout(
            "dynamic.fanouts is empty".to_string(),
        ));
    }

    let base_system = resolve_run_plan(plan, store)?;
    let modules = load_modules(&base_system, base_dir)?;
    let order = topological_order(&base_system)?;

    let mut trace = plan_trace_events(plan);
    for event in &trace {
        observer(event);
    }

    let mut module_outputs = BTreeMap::new();
    let mut skipped_modules = BTreeSet::new();
    let mut materialized: BTreeMap<String, MaterializedFanout> = BTreeMap::new();

    for module_id in order {
        if module_outputs.contains_key(&module_id) {
            continue;
        }
        let current_plan = materialize_partial_dynamic_run_plan(plan, &materialized)?;
        let current_system = resolve_run_plan(&current_plan, store)?;
        let incoming = incoming_connections(&current_system)?;

        if let Some(condition) = current_system.node_conditions.get(&module_id) {
            if !link_condition_matches(condition, &inputs, &module_outputs)? {
                let event = TraceEvent {
                    agent: "$system".to_string(),
                    step: trace.len() as u32,
                    rule: "node_condition".to_string(),
                    action: "skip".to_string(),
                    input: Some(json!({
                        "node": module_id,
                        "when": condition,
                    })),
                    output: None,
                    meta: None,
                    status: TraceStatus::Ok,
                    error: None,
                };
                observer(&event);
                trace.push(event);
                skipped_modules.insert(module_id);
                continue;
            }
        }

        let module = modules
            .get(&module_id)
            .ok_or_else(|| LinkerError::UnknownModule(module_id.clone()))?;
        let module_inputs = build_module_inputs(
            &module_id,
            &inputs,
            &module_outputs,
            &incoming,
            &skipped_modules,
        )?;
        let mut vm = Vm {
            tools: tools_factory(),
            models: models_factory(),
        };
        let result = vm
            .run_with_observer(module, module_inputs, |event| observer(event))
            .map_err(|source| LinkerError::Runtime {
                module: module_id.clone(),
                source,
            })?;
        trace.extend(result.trace);
        module_outputs.insert(module_id.clone(), result.outputs);
        checkpoint(&SystemCheckpoint {
            completed_module: module_id.clone(),
            module_outputs: module_outputs.clone(),
        })
        .map_err(LinkerError::Checkpoint)?;

        if let Some(halt) = triggered_halt(&current_system, &module_id, &module_outputs)? {
            let outputs = if halt.outputs.is_empty() {
                collect_system_outputs(&current_system, &module_outputs)?
            } else {
                collect_outputs(&halt.outputs, &module_outputs)?
            };
            let event = TraceEvent {
                agent: "$system".to_string(),
                step: trace.len() as u32,
                rule: "halt".to_string(),
                action: "halt".to_string(),
                input: Some(json!({
                    "after": halt.after,
                    "ref": halt.when.reference,
                    "equals": halt.when.equals,
                })),
                output: Some(serde_json::Value::Object(outputs.clone())),
                meta: None,
                status: TraceStatus::Ok,
                error: None,
            };
            observer(&event);
            trace.push(event);
            return Ok(SystemRunResult {
                outputs,
                module_outputs,
                trace,
                status: RunStatus::Halted {
                    after: halt.after.clone(),
                    reference: halt.when.reference.clone(),
                    equals: halt.when.equals.clone(),
                },
            });
        }

        for fanout in pending_dynamic_after(dynamic, &materialized, &module_id) {
            let created = run_dynamic_fanout_parallel(
                fanout,
                store,
                base_dir,
                &inputs,
                &mut module_outputs,
                &tools_factory,
                &models_factory,
                &mut trace,
                &mut observer,
                &mut checkpoint,
            )?;
            materialized.insert(fanout.id.clone(), created);
            run_child_dynamic_fanouts_parallel(
                dynamic,
                &fanout.id,
                store,
                base_dir,
                &inputs,
                &mut module_outputs,
                &tools_factory,
                &models_factory,
                &mut materialized,
                &mut trace,
                &mut observer,
                &mut checkpoint,
            )?;
        }
    }

    let missing = missing_dynamic_fanouts(dynamic, &materialized);
    if !missing.is_empty() {
        return Err(LinkerError::DynamicFanout(format!(
            "dynamic fanout(s) were not materialized: {}",
            missing.join(", ")
        )));
    }

    let final_plan =
        push_resolved_dynamic_plan_event(plan, &materialized, &mut trace, &mut observer)?;
    let final_system = resolve_run_plan(&final_plan, store)?;
    let outputs = collect_system_outputs(&final_system, &module_outputs)?;
    Ok(SystemRunResult {
        outputs,
        module_outputs,
        trace,
        status: RunStatus::Completed,
    })
}

pub fn run_system_with_resume_with_observer<T, M, F>(
    system: &AirSystem,
    base_dir: impl AsRef<Path>,
    inputs: State,
    resume: ResumeState,
    tools: T,
    models: M,
    observer: F,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
{
    run_system_with_resume_with_observer_and_checkpoint(
        system,
        base_dir,
        inputs,
        resume,
        tools,
        models,
        observer,
        |_| Ok(()),
    )
}

pub fn run_system_with_resume_with_observer_and_checkpoint<T, M, F, C>(
    system: &AirSystem,
    base_dir: impl AsRef<Path>,
    inputs: State,
    resume: ResumeState,
    tools: T,
    models: M,
    mut observer: F,
    mut checkpoint: C,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let base_dir = base_dir.as_ref();
    let modules = load_modules(system, base_dir)?;
    let verification = validate_loaded_system(system, &modules);
    if !verification.is_success() {
        return Err(LinkerError::SystemVerify {
            diagnostics: verification.diagnostics,
        });
    }
    let execution_batches = execution_batches(system)?;
    let incoming = incoming_connections(system)?;

    let mut vm = Vm { tools, models };
    let mut module_outputs = resume.module_outputs;
    apply_output_overrides(&mut module_outputs, &resume.output_overrides)?;
    let mut skipped_modules = BTreeSet::new();
    let mut trace = Vec::new();

    for batch in execution_batches {
        let pending_modules = batch
            .modules
            .iter()
            .filter(|module_id| !module_outputs.contains_key(*module_id))
            .cloned()
            .collect::<Vec<_>>();
        if pending_modules.is_empty() {
            continue;
        }

        if let Some(group_id) = &batch.group_id {
            let event = TraceEvent {
                agent: "$system".to_string(),
                step: trace.len() as u32,
                rule: "schedule".to_string(),
                action: "schedule_batch".to_string(),
                input: Some(json!({
                    "group": group_id,
                    "nodes": pending_modules,
                    "max_parallel": batch.max_parallel,
                })),
                output: None,
                meta: None,
                status: TraceStatus::Ok,
                error: None,
            };
            observer(&event);
            trace.push(event);
        }

        for module_id in batch.modules {
            if module_outputs.contains_key(&module_id) {
                continue;
            }

            if let Some(condition) = system.node_conditions.get(&module_id) {
                if !link_condition_matches(condition, &inputs, &module_outputs)? {
                    let event = TraceEvent {
                        agent: "$system".to_string(),
                        step: trace.len() as u32,
                        rule: "node_condition".to_string(),
                        action: "skip".to_string(),
                        input: Some(json!({
                            "node": module_id,
                            "when": condition,
                        })),
                        output: None,
                        meta: None,
                        status: TraceStatus::Ok,
                        error: None,
                    };
                    observer(&event);
                    trace.push(event);
                    skipped_modules.insert(module_id);
                    continue;
                }
            }

            let module = modules
                .get(&module_id)
                .ok_or_else(|| LinkerError::UnknownModule(module_id.clone()))?;
            let module_inputs = build_module_inputs(
                &module_id,
                &inputs,
                &module_outputs,
                &incoming,
                &skipped_modules,
            )?;
            let result = vm
                .run_with_observer(module, module_inputs, |event| observer(event))
                .map_err(|source| LinkerError::Runtime {
                    module: module_id.clone(),
                    source,
                })?;
            trace.extend(result.trace);
            module_outputs.insert(module_id.clone(), result.outputs);
            checkpoint(&SystemCheckpoint {
                completed_module: module_id.clone(),
                module_outputs: module_outputs.clone(),
            })
            .map_err(LinkerError::Checkpoint)?;

            if let Some(halt) = triggered_halt(system, &module_id, &module_outputs)? {
                let outputs = if halt.outputs.is_empty() {
                    collect_system_outputs(system, &module_outputs)?
                } else {
                    collect_outputs(&halt.outputs, &module_outputs)?
                };
                let event = TraceEvent {
                    agent: "$system".to_string(),
                    step: trace.len() as u32,
                    rule: "halt".to_string(),
                    action: "halt".to_string(),
                    input: Some(json!({
                        "after": halt.after,
                        "ref": halt.when.reference,
                        "equals": halt.when.equals,
                    })),
                    output: Some(serde_json::Value::Object(outputs.clone())),
                    meta: None,
                    status: TraceStatus::Ok,
                    error: None,
                };
                observer(&event);
                trace.push(event);
                return Ok(SystemRunResult {
                    outputs,
                    module_outputs,
                    trace,
                    status: RunStatus::Halted {
                        after: halt.after.clone(),
                        reference: halt.when.reference.clone(),
                        equals: halt.when.equals.clone(),
                    },
                });
            }
        }
    }

    let outputs = collect_system_outputs(system, &module_outputs)?;

    Ok(SystemRunResult {
        outputs,
        module_outputs,
        trace,
        status: RunStatus::Completed,
    })
}

pub fn run_system_parallel_with_resume_with_observer_and_checkpoint<T, M, TF, MF, F, C>(
    system: &AirSystem,
    base_dir: impl AsRef<Path>,
    inputs: State,
    resume: ResumeState,
    tools_factory: TF,
    models_factory: MF,
    mut observer: F,
    mut checkpoint: C,
) -> Result<SystemRunResult, LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let base_dir = base_dir.as_ref();
    let modules = load_modules(system, base_dir)?;
    let verification = validate_loaded_system(system, &modules);
    if !verification.is_success() {
        return Err(LinkerError::SystemVerify {
            diagnostics: verification.diagnostics,
        });
    }
    let execution_batches = execution_batches(system)?;
    let incoming = incoming_connections(system)?;

    let mut module_outputs = resume.module_outputs;
    apply_output_overrides(&mut module_outputs, &resume.output_overrides)?;
    let mut skipped_modules = BTreeSet::new();
    let mut trace = Vec::new();

    for batch in execution_batches {
        let pending_modules = batch
            .modules
            .iter()
            .filter(|module_id| !module_outputs.contains_key(*module_id))
            .cloned()
            .collect::<Vec<_>>();
        if pending_modules.is_empty() {
            continue;
        }

        if let Some(group_id) = &batch.group_id {
            let event = TraceEvent {
                agent: "$system".to_string(),
                step: trace.len() as u32,
                rule: "schedule".to_string(),
                action: "schedule_batch".to_string(),
                input: Some(json!({
                    "group": group_id,
                    "nodes": pending_modules,
                    "max_parallel": batch.max_parallel,
                    "execution": if batch.modules.len() > 1 { "parallel" } else { "sequential" },
                })),
                output: None,
                meta: None,
                status: TraceStatus::Ok,
                error: None,
            };
            observer(&event);
            trace.push(event);
        }

        let mut jobs = Vec::new();
        for module_id in batch.modules {
            if module_outputs.contains_key(&module_id) {
                continue;
            }

            if let Some(condition) = system.node_conditions.get(&module_id) {
                if !link_condition_matches(condition, &inputs, &module_outputs)? {
                    let event = TraceEvent {
                        agent: "$system".to_string(),
                        step: trace.len() as u32,
                        rule: "node_condition".to_string(),
                        action: "skip".to_string(),
                        input: Some(json!({
                            "node": module_id,
                            "when": condition,
                        })),
                        output: None,
                        meta: None,
                        status: TraceStatus::Ok,
                        error: None,
                    };
                    observer(&event);
                    trace.push(event);
                    skipped_modules.insert(module_id);
                    continue;
                }
            }

            let module = modules
                .get(&module_id)
                .ok_or_else(|| LinkerError::UnknownModule(module_id.clone()))?
                .clone();
            let module_inputs = build_module_inputs(
                &module_id,
                &inputs,
                &module_outputs,
                &incoming,
                &skipped_modules,
            )?;
            jobs.push(ModuleJob {
                module_id,
                module,
                inputs: module_inputs,
            });
        }

        if jobs.is_empty() {
            continue;
        }

        let contains_halt = jobs
            .iter()
            .any(|job| system.halts.iter().any(|halt| halt.after == job.module_id));
        let results = if jobs.len() > 1 && !contains_halt {
            run_module_jobs_parallel(jobs, &tools_factory, &models_factory)?
        } else {
            run_module_jobs_sequential(jobs, &tools_factory, &models_factory)?
        };

        for ModuleJobResult { module_id, result } in results {
            for event in &result.trace {
                observer(event);
            }
            trace.extend(result.trace);
            module_outputs.insert(module_id.clone(), result.outputs);
            checkpoint(&SystemCheckpoint {
                completed_module: module_id.clone(),
                module_outputs: module_outputs.clone(),
            })
            .map_err(LinkerError::Checkpoint)?;

            if let Some(halt) = triggered_halt(system, &module_id, &module_outputs)? {
                let outputs = if halt.outputs.is_empty() {
                    collect_system_outputs(system, &module_outputs)?
                } else {
                    collect_outputs(&halt.outputs, &module_outputs)?
                };
                let event = TraceEvent {
                    agent: "$system".to_string(),
                    step: trace.len() as u32,
                    rule: "halt".to_string(),
                    action: "halt".to_string(),
                    input: Some(json!({
                        "after": halt.after,
                        "ref": halt.when.reference,
                        "equals": halt.when.equals,
                    })),
                    output: Some(serde_json::Value::Object(outputs.clone())),
                    meta: None,
                    status: TraceStatus::Ok,
                    error: None,
                };
                observer(&event);
                trace.push(event);
                return Ok(SystemRunResult {
                    outputs,
                    module_outputs,
                    trace,
                    status: RunStatus::Halted {
                        after: halt.after.clone(),
                        reference: halt.when.reference.clone(),
                        equals: halt.when.equals.clone(),
                    },
                });
            }
        }
    }

    let outputs = collect_system_outputs(system, &module_outputs)?;

    Ok(SystemRunResult {
        outputs,
        module_outputs,
        trace,
        status: RunStatus::Completed,
    })
}

pub fn resolve_run_plan(plan: &RunPlan, store: &ModuleStore) -> Result<AirSystem, LinkerError> {
    let mut modules = BTreeMap::new();
    let mut node_conditions = BTreeMap::new();

    for node in &plan.nodes {
        let module_ref = store
            .modules
            .get(&node.module)
            .ok_or_else(|| LinkerError::UnknownModule(node.module.clone()))?;
        modules.insert(node.id.clone(), module_ref.clone());
        if let Some(condition) = &node.when {
            node_conditions.insert(node.id.clone(), condition.clone());
        }
    }

    Ok(AirSystem {
        system: SystemMetadata {
            name: plan.plan.name.clone(),
            version: plan.plan.version.clone(),
        },
        modules,
        entry: plan.entry.clone(),
        edges: plan.edges.clone(),
        connect: plan.connect.clone(),
        outputs: plan.outputs.clone(),
        halts: plan.halts.clone(),
        node_conditions,
        schedule: plan.schedule.clone(),
    })
}

fn resolve_run_plan_for_validation(
    plan: &RunPlan,
    store: &ModuleStore,
) -> Result<AirSystem, LinkerError> {
    let mut system = resolve_run_plan(plan, store)?;
    let Some(dynamic) = &plan.dynamic else {
        return Ok(system);
    };

    for fanout in &dynamic.fanouts {
        let module_ref = store
            .modules
            .get(&fanout.module)
            .ok_or_else(|| LinkerError::UnknownModule(fanout.module.clone()))?;
        let placeholder = dynamic_placeholder_id(fanout);
        let after_node = dynamic
            .fanouts
            .iter()
            .find(|candidate| candidate.id == fanout.after)
            .map(dynamic_placeholder_id)
            .unwrap_or_else(|| fanout.after.clone());
        let validation_source = dynamic
            .fanouts
            .iter()
            .find(|candidate| candidate.id == fanout.after)
            .map(dynamic_placeholder_id)
            .map(|parent| replace_parent_endpoint(&fanout.source, &parent))
            .transpose()?
            .unwrap_or_else(|| fanout.source.clone());
        system
            .modules
            .insert(placeholder.clone(), module_ref.clone());
        push_unique_edge(
            &mut system.edges,
            SystemEdge {
                from: after_node,
                to: placeholder.clone(),
            },
        );

        for connection in &fanout.input {
            if let Some(from) = &connection.from {
                if let Ok(endpoint) = parse_endpoint(from) {
                    if endpoint.module != "$input" {
                        push_unique_edge(
                            &mut system.edges,
                            SystemEdge {
                                from: endpoint.module,
                                to: placeholder.clone(),
                            },
                        );
                    }
                }
            }
            for module in link_expr_source_modules(connection.value.as_ref()) {
                if module != "$input" {
                    push_unique_edge(
                        &mut system.edges,
                        SystemEdge {
                            from: module,
                            to: placeholder.clone(),
                        },
                    );
                }
            }
            system.connect.push(Connection {
                from: connection
                    .from
                    .as_ref()
                    .map(|from| replace_item_source(from, &validation_source, 0)),
                to: replace_each_endpoint(&connection.to, &placeholder),
                value: connection
                    .value
                    .as_ref()
                    .map(|value| replace_item_sources_in_expr(value, &validation_source, 0)),
            });
        }

        for connection in &fanout.fan_in {
            let to = parse_endpoint(&connection.to)?;
            push_unique_edge(
                &mut system.edges,
                SystemEdge {
                    from: placeholder.clone(),
                    to: to.module,
                },
            );
            system.connect.push(Connection {
                from: connection
                    .from
                    .as_ref()
                    .map(|from| replace_each_endpoint(from, &placeholder)),
                to: connection.to.clone(),
                value: connection
                    .value
                    .as_ref()
                    .map(|value| replace_each_endpoints_in_expr(value, &placeholder)),
            });
        }
    }

    Ok(system)
}

#[derive(Default)]
struct SystemVerifier {
    diagnostics: Vec<Diagnostic>,
}

impl SystemVerifier {
    fn verify(&mut self, system: &AirSystem, base_dir: &Path) {
        if system.system.name.trim().is_empty() {
            self.error("AIRL001", "system.name must not be empty");
        }
        if system.system.version.trim().is_empty() {
            self.error("AIRL002", "system.version must not be empty");
        }
        if !system.modules.contains_key(&system.entry) {
            self.error(
                "AIRL003",
                format!("system.entry references unknown module {}", system.entry),
            );
        }

        let Ok(modules) = load_modules(system, base_dir) else {
            self.error("AIRL004", "failed to load or verify one or more modules");
            return;
        };

        self.diagnostics
            .extend(validate_loaded_system(system, &modules).diagnostics);
    }

    fn error(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(code, message));
    }
}

#[derive(Default)]
struct RunPlanVerifier {
    diagnostics: Vec<Diagnostic>,
}

impl RunPlanVerifier {
    fn verify(&mut self, plan: &RunPlan, store: &ModuleStore) {
        if store.store.name.trim().is_empty() {
            self.error("AIRP001", "store.name must not be empty");
        }
        if store.store.version.trim().is_empty() {
            self.error("AIRP002", "store.version must not be empty");
        }
        if plan.plan.name.trim().is_empty() {
            self.error("AIRP003", "plan.name must not be empty");
        }
        if plan.plan.version.trim().is_empty() {
            self.error("AIRP004", "plan.version must not be empty");
        }

        let mut node_counts: BTreeMap<&str, usize> = BTreeMap::new();
        for node in &plan.nodes {
            if node.id.trim().is_empty() {
                self.error("AIRP010", "run plan node id must not be empty");
            }
            if node.module.trim().is_empty() {
                self.error(
                    "AIRP011",
                    format!("run plan node {} module must not be empty", node.id),
                );
            }
            *node_counts.entry(node.id.as_str()).or_default() += 1;
            if !store.modules.contains_key(&node.module) {
                self.error(
                    "AIRP012",
                    format!(
                        "run plan node {} references unknown store module {}",
                        node.id, node.module
                    ),
                );
            }
        }

        for (node_id, count) in node_counts {
            if count > 1 {
                self.error(
                    "AIRP013",
                    format!("run plan node id {node_id} is duplicated"),
                );
            }
        }

        if !plan.nodes.iter().any(|node| node.id == plan.entry) {
            self.error(
                "AIRP014",
                format!("run plan entry references unknown node {}", plan.entry),
            );
        }

        self.verify_dynamic(plan, store);
    }

    fn verify_dynamic(&mut self, plan: &RunPlan, store: &ModuleStore) {
        let Some(dynamic) = &plan.dynamic else {
            return;
        };

        let node_ids = plan
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        let all_fanout_ids = dynamic
            .fanouts
            .iter()
            .map(|fanout| fanout.id.as_str())
            .collect::<BTreeSet<_>>();
        let mut fanout_ids = BTreeSet::new();
        for fanout in &dynamic.fanouts {
            if fanout.id.trim().is_empty() {
                self.error("AIRP030", "dynamic fanout id must not be empty");
            }
            if !fanout_ids.insert(fanout.id.as_str()) {
                self.error(
                    "AIRP031",
                    format!("dynamic fanout id {} is duplicated", fanout.id),
                );
            }
            let after_is_node = node_ids.contains(fanout.after.as_str());
            let after_is_fanout = all_fanout_ids.contains(fanout.after.as_str());
            if !after_is_node && !after_is_fanout {
                self.error(
                    "AIRP032",
                    format!(
                        "dynamic fanout {} after references unknown node or fanout {}",
                        fanout.id, fanout.after
                    ),
                );
            }
            if fanout.source.starts_with("$parent.") && !after_is_fanout {
                self.error(
                    "AIRP051",
                    format!(
                        "dynamic fanout {} source {} can use $parent only when after references another fanout",
                        fanout.id, fanout.source
                    ),
                );
            }
            if !store.modules.contains_key(&fanout.module) {
                self.error(
                    "AIRP034",
                    format!(
                        "dynamic fanout {} references unknown store module {}",
                        fanout.id, fanout.module
                    ),
                );
            }
            if fanout.id_prefix.trim().is_empty()
                || fanout.id_prefix.contains('.')
                || fanout.id_prefix == "$input"
                || fanout.id_prefix == "$each"
            {
                self.error(
                    "AIRP035",
                    format!(
                        "dynamic fanout {} id_prefix must be a non-empty node id prefix",
                        fanout.id
                    ),
                );
            }
            if fanout.max_items == 0 {
                self.error(
                    "AIRP036",
                    format!(
                        "dynamic fanout {} max_items must be greater than zero",
                        fanout.id
                    ),
                );
            }
            if fanout.min_items.is_some_and(|min| min > fanout.max_items) {
                self.error(
                    "AIRP037",
                    format!(
                        "dynamic fanout {} min_items cannot exceed max_items",
                        fanout.id
                    ),
                );
            }
            if fanout.max_parallel == Some(0) {
                self.error(
                    "AIRP038",
                    format!(
                        "dynamic fanout {} max_parallel must be greater than zero",
                        fanout.id
                    ),
                );
            }
            if fanout.input.is_empty() {
                self.error(
                    "AIRP039",
                    format!("dynamic fanout {} must declare input mappings", fanout.id),
                );
            }
            if fanout.fan_in.is_empty() {
                self.error(
                    "AIRP040",
                    format!("dynamic fanout {} must declare fan_in mappings", fanout.id),
                );
            }

            for connection in &fanout.input {
                if !is_each_endpoint(&connection.to) {
                    self.error(
                        "AIRP041",
                        format!(
                            "dynamic fanout {} input target {} must reference $each",
                            fanout.id, connection.to
                        ),
                    );
                }
                match (&connection.from, &connection.value) {
                    (Some(_), Some(_)) => self.error(
                        "AIRP042",
                        format!(
                            "dynamic fanout {} input mapping to {} cannot declare both from and value",
                            fanout.id, connection.to
                        ),
                    ),
                    (None, None) => self.error(
                        "AIRP042",
                        format!(
                            "dynamic fanout {} input mapping to {} must declare from or value",
                            fanout.id, connection.to
                        ),
                    ),
                    (Some(from), None) => {
                        if !(from == "$item"
                            || from.starts_with("$item.")
                            || parse_endpoint(from).is_ok())
                        {
                            self.error(
                                "AIRP042",
                                format!(
                                    "dynamic fanout {} input source {} must be $item or a normal endpoint",
                                    fanout.id, from
                                ),
                            );
                        }
                    }
                    (None, Some(value)) => {
                        if let Err(error) = validate_dynamic_link_expr_sources(value) {
                            self.error(
                                "AIRP042",
                                format!("dynamic fanout {} input value {error}", fanout.id),
                            );
                        }
                    }
                }
            }

            for connection in &fanout.fan_in {
                match (&connection.from, &connection.value) {
                    (Some(_), Some(_)) => self.error(
                        "AIRP043",
                        format!(
                            "dynamic fanout {} fan_in mapping to {} cannot declare both from and value",
                            fanout.id, connection.to
                        ),
                    ),
                    (None, None) => self.error(
                        "AIRP043",
                        format!(
                            "dynamic fanout {} fan_in mapping to {} must declare from or value",
                            fanout.id, connection.to
                        ),
                    ),
                    (Some(from), None) => {
                        if !is_each_endpoint(from) {
                            self.error(
                                "AIRP043",
                                format!(
                                    "dynamic fanout {} fan_in source {} must reference $each",
                                    fanout.id, from
                                ),
                            );
                        }
                    }
                    (None, Some(value)) => {
                        if !link_expr_references_each(value) {
                            self.error(
                                "AIRP043",
                                format!(
                                    "dynamic fanout {} fan_in value must reference $each",
                                    fanout.id
                                ),
                            );
                        }
                    }
                }
                if parse_endpoint(&connection.to).is_err() {
                    self.error(
                        "AIRP044",
                        format!(
                            "dynamic fanout {} fan_in target {} must be a normal endpoint",
                            fanout.id, connection.to
                        ),
                    );
                }
            }
        }
    }

    fn error(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(code, message));
    }
}

fn plan_trace_events(plan: &RunPlan) -> Vec<TraceEvent> {
    let mut events = vec![TraceEvent {
        agent: "$planner".to_string(),
        step: 0,
        rule: "run_plan".to_string(),
        action: "resolve_plan".to_string(),
        input: None,
        output: Some(json!({
            "plan": &plan.plan,
            "requires": &plan.requires,
            "entry": &plan.entry,
            "nodes": &plan.nodes,
            "edges": &plan.edges,
            "connect": &plan.connect,
            "outputs": &plan.outputs,
            "halts": &plan.halts,
            "decisions": &plan.decisions,
            "schedule": &plan.schedule,
            "dynamic": &plan.dynamic,
        })),
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    }];

    events.extend(
        plan.decisions
            .iter()
            .enumerate()
            .map(|(index, decision)| TraceEvent {
                agent: "$planner".to_string(),
                step: (index + 1) as u32,
                rule: "run_plan".to_string(),
                action: "decision".to_string(),
                input: None,
                output: Some(json!({
                    "id": &decision.id,
                    "rationale": &decision.rationale,
                    "selected": &decision.selected,
                })),
                meta: None,
                status: TraceStatus::Ok,
                error: None,
            }),
    );

    events
}

fn trace_specialization_identity(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: &Path,
) -> Result<Value, LinkerError> {
    let system = resolve_run_plan(plan, store)?;
    let modules = load_modules(&system, base_dir)?;
    let nodes = plan
        .nodes
        .iter()
        .map(|node| {
            let module = modules
                .get(&node.id)
                .ok_or_else(|| LinkerError::UnknownModule(node.id.clone()))?;
            Ok((
                node.id.clone(),
                json!({
                    "store_module": node.module,
                    "agent": module.agent,
                    "inputs": module.inputs,
                    "outputs": module.outputs,
                    "requires": module.requires,
                    "policy": module.policy,
                }),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, LinkerError>>()?;

    Ok(json!({
        "kind": "air.trace_specialization.v1",
        "plan": &plan.plan,
        "requires": &plan.requires,
        "nodes": nodes,
        "schedule": &plan.schedule,
    }))
}

fn validate_run_plan_capabilities(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: &Path,
) -> Option<SystemVerificationReport> {
    let mut diagnostics = Vec::new();
    let allowed = plan
        .requires
        .capabilities
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut module_ids = plan
        .nodes
        .iter()
        .map(|node| node.module.clone())
        .collect::<BTreeSet<_>>();

    if let Some(dynamic) = &plan.dynamic {
        module_ids.extend(dynamic.fanouts.iter().map(|fanout| fanout.module.clone()));
    }

    for module_id in module_ids {
        let Some(module_ref) = store.modules.get(&module_id) else {
            continue;
        };
        let Ok(module) = air_parser::parse_air_file(base_dir.join(&module_ref.path)) else {
            continue;
        };
        for capability in &module.requires.capabilities {
            if !allowed.contains(capability) {
                diagnostics.push(Diagnostic::error(
                    "AIRP060",
                    format!(
                        "run plan {} uses module {} requiring capability {}, but it is missing from plan.requires.capabilities",
                        plan.plan.name, module_id, capability
                    ),
                ));
            }
        }
    }

    Some(SystemVerificationReport { diagnostics })
}

fn validate_loaded_system(
    system: &AirSystem,
    modules: &BTreeMap<String, AirModule>,
) -> SystemVerificationReport {
    let mut verifier = LoadedSystemVerifier {
        diagnostics: Vec::new(),
        system,
        modules,
    };
    verifier.verify();
    SystemVerificationReport {
        diagnostics: verifier.diagnostics,
    }
}

fn validate_dynamic_run_plan(
    plan: &RunPlan,
    store: &ModuleStore,
    base_dir: &Path,
) -> Option<SystemVerificationReport> {
    let dynamic = plan.dynamic.as_ref()?;
    let system = resolve_run_plan(plan, store).ok()?;
    let modules = load_modules(&system, base_dir).ok()?;
    let mut diagnostics = Vec::new();

    for fanout in &dynamic.fanouts {
        let source_type = if let Some(path) = fanout.source.strip_prefix("$parent.") {
            let Some(parent) = dynamic
                .fanouts
                .iter()
                .find(|candidate| candidate.id == fanout.after)
            else {
                diagnostics.push(Diagnostic::error(
                    "AIRP051",
                    format!(
                        "dynamic fanout {} source {} can use $parent only when after references another fanout",
                        fanout.id, fanout.source
                    ),
                ));
                continue;
            };
            let Some(module_ref) = store.modules.get(&parent.module) else {
                continue;
            };
            let Ok(parent_module) = air_parser::parse_air_file(base_dir.join(&module_ref.path))
            else {
                continue;
            };
            let normalized = match normalize_endpoint_path(path) {
                Ok(path) => path,
                Err(error) => {
                    diagnostics.push(Diagnostic::error("AIRP045", error.to_string()));
                    continue;
                }
            };
            let mut parts = normalized.split('.');
            let Some(field) = parts.next().filter(|field| !field.is_empty()) else {
                diagnostics.push(Diagnostic::error(
                    "AIRP047",
                    format!(
                        "dynamic fanout {} source {} is not an output endpoint",
                        fanout.id, fanout.source
                    ),
                ));
                continue;
            };
            let path = parts.map(str::to_string).collect::<Vec<_>>();
            let Some(source_type) = parent_module
                .outputs
                .get(field)
                .and_then(|field_type| nested_type(field_type, &path))
                .cloned()
            else {
                diagnostics.push(Diagnostic::error(
                    "AIRP047",
                    format!(
                        "dynamic fanout {} source {} is not an output endpoint",
                        fanout.id, fanout.source
                    ),
                ));
                continue;
            };
            source_type
        } else {
            let source = match parse_endpoint(&fanout.source) {
                Ok(source) => source,
                Err(error) => {
                    diagnostics.push(Diagnostic::error("AIRP045", error.to_string()));
                    continue;
                }
            };
            if source.append {
                diagnostics.push(Diagnostic::error(
                    "AIRP046",
                    format!(
                        "dynamic fanout {} source {} cannot use append reducer",
                        fanout.id, fanout.source
                    ),
                ));
                continue;
            }
            if source.module == "$input" {
                continue;
            }

            let Some(source_type) = modules.get(&source.module).and_then(|module| {
                module
                    .outputs
                    .get(&source.field)
                    .and_then(|field_type| nested_type(field_type, &source.path))
                    .cloned()
            }) else {
                diagnostics.push(Diagnostic::error(
                    "AIRP047",
                    format!(
                        "dynamic fanout {} source {} is not an output endpoint",
                        fanout.id, fanout.source
                    ),
                ));
                continue;
            };
            source_type
        };

        if !is_array_type(&source_type) {
            diagnostics.push(Diagnostic::error(
                "AIRP048",
                format!(
                    "dynamic fanout {} source {} must be an array output, got {}",
                    fanout.id,
                    fanout.source,
                    source_type.kind_name()
                ),
            ));
            continue;
        }

        let (source_min_items, source_max_items) = array_bounds(&source_type);
        if let Some(source_max_items) = source_max_items {
            if source_max_items > fanout.max_items {
                diagnostics.push(Diagnostic::error(
                    "AIRP049",
                    format!(
                        "dynamic fanout {} max_items={} is smaller than source {} max_items={source_max_items}",
                        fanout.id, fanout.max_items, fanout.source
                    ),
                ));
            }
        }
        if let (Some(fanout_min_items), Some(source_min_items)) =
            (fanout.min_items, source_min_items)
        {
            if source_min_items < fanout_min_items {
                diagnostics.push(Diagnostic::error(
                    "AIRP050",
                    format!(
                        "dynamic fanout {} min_items={fanout_min_items} is larger than source {} min_items={source_min_items}",
                        fanout.id, fanout.source
                    ),
                ));
            }
        }
    }

    Some(SystemVerificationReport { diagnostics })
}

struct LoadedSystemVerifier<'a> {
    diagnostics: Vec<Diagnostic>,
    system: &'a AirSystem,
    modules: &'a BTreeMap<String, AirModule>,
}

impl LoadedSystemVerifier<'_> {
    fn verify(&mut self) {
        if let Err(error) = topological_order(self.system) {
            self.error("AIRL010", error.to_string());
        }

        let parsed_connections = self.verify_connections();
        self.verify_required_inputs(&parsed_connections);
        self.verify_outputs();
        self.verify_halts();
        self.verify_node_conditions();
        self.verify_schedule();
    }

    fn verify_connections(&mut self) -> Vec<(Endpoint, Endpoint)> {
        let mut parsed = Vec::new();

        for connection in &self.system.connect {
            let to = match parse_endpoint(&connection.to) {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    self.error("AIRL022", error.to_string());
                    continue;
                }
            };

            let source = match connection_source_expr(connection) {
                Ok(source) => source,
                Err(error) => {
                    self.error("AIRL021", error);
                    continue;
                }
            };
            let from_type = self.infer_link_expr_type(&source);

            if !to.path.is_empty() {
                self.error(
                    "AIRL026",
                    format!(
                        "connect target {}.{} cannot use nested paths",
                        to.module,
                        to.display_field()
                    ),
                );
            }

            let to_type = self
                .modules
                .get(&to.module)
                .and_then(|module| module.inputs.get(&to.field));
            if to_type.is_none() {
                self.error(
                    "AIRL020",
                    format!(
                        "connect target {}.{} is not an input endpoint",
                        to.module, to.field
                    ),
                );
            }

            if to.append && to_type.is_some_and(|to_type| !is_array_type(to_type)) {
                self.error(
                    "AIRL025",
                    format!(
                        "connect target {}.{}[] requires an array input",
                        to.module, to.field
                    ),
                );
            }

            if let (Some(from_type), Some(to_type)) = (from_type, to_type) {
                let target_type = if to.append {
                    array_item_type(to_type)
                } else {
                    Some(to_type)
                };
                if target_type.is_some_and(|target_type| !types_compatible(&from_type, target_type))
                {
                    self.error(
                        "AIRL023",
                        format!(
                            "connect value ({}) is not compatible with {}.{} ({})",
                            from_type.kind_name(),
                            to.module,
                            if to.append {
                                format!("{}[]", to.field)
                            } else {
                                to.field.clone()
                            },
                            to_type.kind_name()
                        ),
                    );
                }
            }

            for from in link_expr_endpoints(&source) {
                parsed.push((from, to.clone()));
            }
        }

        parsed
    }

    fn infer_link_expr_type(&mut self, expr: &LinkExpr) -> Option<TypeSpec> {
        match expr {
            LinkExpr::From { from } => {
                let endpoint = match parse_endpoint(from) {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        self.error("AIRL021", error.to_string());
                        return None;
                    }
                };
                if endpoint.append {
                    self.error(
                        "AIRL024",
                        format!(
                            "connect source {}.{}[] cannot use append reducer",
                            endpoint.module, endpoint.field
                        ),
                    );
                }
                if endpoint.module == "$input" {
                    return None;
                }
                let source_type = self.modules.get(&endpoint.module).and_then(|module| {
                    module
                        .outputs
                        .get(&endpoint.field)
                        .and_then(|field_type| nested_type(field_type, &endpoint.path))
                });
                if source_type.is_none() {
                    self.error(
                        "AIRL020",
                        format!(
                            "connect source {}.{} is not an output endpoint",
                            endpoint.module,
                            endpoint.display_field()
                        ),
                    );
                }
                source_type.cloned()
            }
            LinkExpr::Count { count } => {
                if let Some(source_type) = self.infer_link_expr_type(count) {
                    if !is_array_type(&source_type) {
                        self.error(
                            "AIRL023",
                            format!(
                                "count expression requires an array input, got {}",
                                source_type.kind_name()
                            ),
                        );
                    }
                }
                Some(integer_type().clone())
            }
            LinkExpr::Literal { literal } => Some(json_value_type(literal).clone()),
            LinkExpr::Object { .. } => Some(object_type().clone()),
            LinkExpr::Array { .. } => Some(array_type().clone()),
            LinkExpr::Coalesce { coalesce } => {
                let mut inferred = None;
                for candidate in coalesce {
                    let candidate_type = self.infer_link_expr_type(candidate);
                    if inferred.is_none() && candidate_type.is_some() {
                        inferred = candidate_type;
                    }
                }
                inferred
            }
        }
    }

    fn verify_required_inputs(&mut self, connections: &[(Endpoint, Endpoint)]) {
        let mut covered: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (_, to) in connections {
            covered
                .entry(to.module.as_str())
                .or_default()
                .push(to.field.as_str());
        }

        for (module_id, module) in self.modules {
            if module_id == &self.system.entry {
                continue;
            }

            for input in module.inputs.keys() {
                let is_covered = covered
                    .get(module_id.as_str())
                    .is_some_and(|fields| fields.contains(&input.as_str()));
                if !is_covered {
                    self.error(
                        "AIRL030",
                        format!("module {module_id} input {input} is not covered by connect"),
                    );
                }
            }
        }
    }

    fn verify_outputs(&mut self) {
        for (name, endpoint) in &self.system.outputs {
            let endpoint = match parse_endpoint(endpoint) {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    self.error("AIRL041", error.to_string());
                    continue;
                }
            };

            let exists = self
                .modules
                .get(&endpoint.module)
                .and_then(|module| {
                    module
                        .outputs
                        .get(&endpoint.field)
                        .and_then(|field_type| nested_type(field_type, &endpoint.path))
                })
                .is_some();
            if !exists {
                self.error(
                    "AIRL040",
                    format!(
                        "system output {name} references unknown output endpoint {}.{}",
                        endpoint.module,
                        endpoint.display_field()
                    ),
                );
            }
        }
    }

    fn verify_halts(&mut self) {
        for halt in &self.system.halts {
            if !self.modules.contains_key(&halt.after) {
                self.error(
                    "AIRL050",
                    format!("halt.after references unknown module {}", halt.after),
                );
            }

            let condition = match parse_endpoint(&halt.when.reference) {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    self.error("AIRL051", error.to_string());
                    continue;
                }
            };
            if condition.module == "$input" || condition.append {
                self.error(
                    "AIRL052",
                    format!(
                        "halt condition must reference a module output endpoint, got {}.{}",
                        condition.module,
                        condition.display_field()
                    ),
                );
            }
            if !self.output_endpoint_exists(&condition) {
                self.error(
                    "AIRL053",
                    format!(
                        "halt condition references unknown output endpoint {}.{}",
                        condition.module,
                        condition.display_field()
                    ),
                );
            }

            for (name, endpoint) in &halt.outputs {
                let endpoint = match parse_endpoint(endpoint) {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        self.error("AIRL054", error.to_string());
                        continue;
                    }
                };
                if endpoint.module == "$input"
                    || endpoint.append
                    || !self.output_endpoint_exists(&endpoint)
                {
                    self.error(
                        "AIRL055",
                        format!(
                            "halt output {name} references unknown output endpoint {}.{}",
                            endpoint.module,
                            endpoint.display_field()
                        ),
                    );
                }
            }
        }
    }

    fn verify_node_conditions(&mut self) {
        for (node, condition) in &self.system.node_conditions {
            if !self.modules.contains_key(node) {
                self.error(
                    "AIRL056",
                    format!("node condition references unknown module {node}"),
                );
            }
            self.verify_link_condition("node condition", condition);
        }
    }

    fn verify_schedule(&mut self) {
        let Some(schedule) = &self.system.schedule else {
            return;
        };

        if schedule.max_parallel == Some(0) {
            self.error("AIRL070", "schedule.max_parallel must be greater than zero");
        }

        let predecessors = direct_predecessors(self.system);
        let mut scheduled_nodes = BTreeSet::new();
        for group in &schedule.groups {
            if group.id.trim().is_empty() {
                self.error("AIRL071", "schedule group id must not be empty");
            }
            if group.nodes.is_empty() {
                self.error(
                    "AIRL072",
                    format!("schedule group {} must contain at least one node", group.id),
                );
            }
            if group.max_parallel == Some(0) {
                self.error(
                    "AIRL073",
                    format!(
                        "schedule group {} max_parallel must be greater than zero",
                        group.id
                    ),
                );
            }

            let mut group_nodes = BTreeSet::new();
            for node in &group.nodes {
                if !self.modules.contains_key(node) {
                    self.error(
                        "AIRL074",
                        format!("schedule group {} references unknown node {node}", group.id),
                    );
                    continue;
                }
                if !group_nodes.insert(node.clone()) {
                    self.error(
                        "AIRL075",
                        format!("schedule group {} contains duplicate node {node}", group.id),
                    );
                }
                if !scheduled_nodes.insert(node.clone()) {
                    self.error(
                        "AIRL076",
                        format!("node {node} appears in more than one schedule group"),
                    );
                }
            }

            let Some(first_node) = group.nodes.first() else {
                continue;
            };
            let first_predecessors = predecessors.get(first_node).cloned().unwrap_or_default();
            for node in group.nodes.iter().skip(1) {
                let node_predecessors = predecessors.get(node).cloned().unwrap_or_default();
                if node_predecessors != first_predecessors {
                    self.error(
                        "AIRL077",
                        format!(
                            "schedule group {} nodes must share the same direct predecessors",
                            group.id
                        ),
                    );
                    break;
                }
            }
        }
    }

    fn verify_link_condition(&mut self, label: &str, condition: &str) {
        for group in condition.split("||") {
            for clause in group.split("&&") {
                let clause = clause.trim();
                let Some((left, right)) =
                    clause.split_once("==").or_else(|| clause.split_once("!="))
                else {
                    self.error(
                        "AIRL057",
                        format!("{label} has unsupported condition clause {clause}"),
                    );
                    continue;
                };
                if right.trim().is_empty() {
                    self.error("AIRL058", format!("{label} has empty condition literal"));
                }
                let endpoint = match parse_endpoint(left.trim()) {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        self.error("AIRL059", error.to_string());
                        continue;
                    }
                };
                if endpoint.append {
                    self.error(
                        "AIRL060",
                        format!(
                            "{label} cannot use append endpoint {}.{}[]",
                            endpoint.module,
                            endpoint.display_field()
                        ),
                    );
                }
                if endpoint.module != "$input" && !self.output_endpoint_exists(&endpoint) {
                    self.error(
                        "AIRL061",
                        format!(
                            "{label} references unknown output endpoint {}.{}",
                            endpoint.module,
                            endpoint.display_field()
                        ),
                    );
                }
            }
        }
    }

    fn output_endpoint_exists(&self, endpoint: &Endpoint) -> bool {
        self.modules
            .get(&endpoint.module)
            .and_then(|module| {
                module
                    .outputs
                    .get(&endpoint.field)
                    .and_then(|field_type| nested_type(field_type, &endpoint.path))
            })
            .is_some()
    }

    fn error(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(code, message));
    }
}

fn load_modules(
    system: &AirSystem,
    base_dir: &Path,
) -> Result<BTreeMap<String, AirModule>, LinkerError> {
    let mut modules = BTreeMap::new();

    for (id, module_ref) in &system.modules {
        let module = air_parser::parse_air_file(base_dir.join(&module_ref.path))?;
        let verification = air_verify::verify(&module);
        if !verification.is_success() {
            return Err(LinkerError::ModuleVerify {
                module: id.clone(),
                diagnostics: verification.diagnostics,
            });
        }
        modules.insert(id.clone(), module);
    }

    Ok(modules)
}

fn has_dynamic_fanouts(plan: &RunPlan) -> bool {
    plan.dynamic
        .as_ref()
        .is_some_and(|dynamic| !dynamic.fanouts.is_empty())
}

fn run_dynamic_fanout<T, M, F, C>(
    fanout: &DynamicFanout,
    store: &ModuleStore,
    base_dir: &Path,
    root_inputs: &State,
    module_outputs: &mut BTreeMap<String, State>,
    vm: &mut Vm<T, M>,
    trace: &mut Vec<TraceEvent>,
    observer: &mut F,
    checkpoint: &mut C,
) -> Result<MaterializedFanout, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let source = parse_endpoint(&fanout.source)?;
    let source_value = read_endpoint_value(root_inputs, module_outputs, &source)?;
    let items = source_value.as_array().cloned().ok_or_else(|| {
        LinkerError::DynamicFanout(format!(
            "fanout {} source {} must evaluate to an array",
            fanout.id, fanout.source
        ))
    })?;
    if let Some(min_items) = fanout.min_items {
        if items.len() < min_items {
            return Err(LinkerError::DynamicFanout(format!(
                "fanout {} expected at least {min_items} items, got {}",
                fanout.id,
                items.len()
            )));
        }
    }
    if items.len() > fanout.max_items {
        return Err(LinkerError::DynamicFanout(format!(
            "fanout {} expected at most {} items, got {}",
            fanout.id,
            fanout.max_items,
            items.len()
        )));
    }

    let module_ref = store
        .modules
        .get(&fanout.module)
        .ok_or_else(|| LinkerError::UnknownModule(fanout.module.clone()))?;
    let module = air_parser::parse_air_file(base_dir.join(&module_ref.path))?;
    let verification = air_verify::verify(&module);
    if !verification.is_success() {
        return Err(LinkerError::ModuleVerify {
            module: fanout.module.clone(),
            diagnostics: verification.diagnostics,
        });
    }

    let created = (0..items.len())
        .map(|index| MaterializedFanoutNode {
            id: dynamic_item_id(fanout, index),
            after: fanout.after.clone(),
            source: fanout.source.clone(),
            item_index: index,
        })
        .collect::<Vec<_>>();
    let created_ids = materialized_node_ids(&created);
    let event = TraceEvent {
        agent: "$planner".to_string(),
        step: trace.len() as u32,
        rule: "dynamic_fanout".to_string(),
        action: "materialize".to_string(),
        input: Some(json!({
            "id": fanout.id,
            "source": fanout.source,
            "module": fanout.module,
            "max_items": fanout.max_items,
        })),
        output: Some(json!({
            "nodes": &created_ids,
        })),
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    };
    observer(&event);
    trace.push(event);

    for (index, item) in items.iter().enumerate() {
        let module_id = created[index].id.clone();
        if module_outputs.contains_key(&module_id) {
            continue;
        }
        let module_inputs = build_dynamic_fanout_inputs(fanout, item, root_inputs, module_outputs)?;
        let event = TraceEvent {
            agent: "$system".to_string(),
            step: trace.len() as u32,
            rule: "dynamic_fanout".to_string(),
            action: "run_node_start".to_string(),
            input: Some(json!({
                "fanout": fanout.id,
                "node": module_id,
                "index": index,
                "module": fanout.module,
                "item": item,
            })),
            output: None,
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        };
        observer(&event);
        trace.push(event);

        let mut scoped_trace = Vec::new();
        let result = vm
            .run_with_observer(&module, module_inputs, |event| {
                let scoped = scoped_dynamic_event(&module_id, event);
                observer(&scoped);
                scoped_trace.push(scoped);
            })
            .map_err(|source| LinkerError::Runtime {
                module: module_id.clone(),
                source,
            })?;
        let output_keys = result.outputs.keys().cloned().collect::<Vec<_>>();
        trace.extend(scoped_trace);
        module_outputs.insert(module_id.clone(), result.outputs);
        checkpoint(&SystemCheckpoint {
            completed_module: module_id.clone(),
            module_outputs: module_outputs.clone(),
        })
        .map_err(LinkerError::Checkpoint)?;
        let event = TraceEvent {
            agent: "$system".to_string(),
            step: trace.len() as u32,
            rule: "dynamic_fanout".to_string(),
            action: "run_node_done".to_string(),
            input: Some(json!({
                "fanout": fanout.id,
                "node": module_id,
                "index": index,
                "module": fanout.module,
            })),
            output: Some(json!({
                "outputs": output_keys,
            })),
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        };
        observer(&event);
        trace.push(event);
    }

    Ok(MaterializedFanout { nodes: created })
}

fn run_child_dynamic_fanouts<T, M, F, C>(
    dynamic: &RunPlanDynamic,
    after_fanout: &str,
    store: &ModuleStore,
    base_dir: &Path,
    root_inputs: &State,
    module_outputs: &mut BTreeMap<String, State>,
    vm: &mut Vm<T, M>,
    materialized: &mut BTreeMap<String, MaterializedFanout>,
    trace: &mut Vec<TraceEvent>,
    observer: &mut F,
    checkpoint: &mut C,
) -> Result<(), LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    for fanout in pending_dynamic_after(dynamic, materialized, after_fanout) {
        let created = run_child_dynamic_fanout(
            fanout,
            after_fanout,
            store,
            base_dir,
            root_inputs,
            module_outputs,
            vm,
            materialized,
            trace,
            observer,
            checkpoint,
        )?;
        materialized.insert(fanout.id.clone(), created);
        run_child_dynamic_fanouts(
            dynamic,
            &fanout.id,
            store,
            base_dir,
            root_inputs,
            module_outputs,
            vm,
            materialized,
            trace,
            observer,
            checkpoint,
        )?;
    }
    Ok(())
}

fn run_child_dynamic_fanout<T, M, F, C>(
    fanout: &DynamicFanout,
    parent_fanout_id: &str,
    store: &ModuleStore,
    base_dir: &Path,
    root_inputs: &State,
    module_outputs: &mut BTreeMap<String, State>,
    vm: &mut Vm<T, M>,
    materialized: &BTreeMap<String, MaterializedFanout>,
    trace: &mut Vec<TraceEvent>,
    observer: &mut F,
    checkpoint: &mut C,
) -> Result<MaterializedFanout, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let parent_nodes = materialized
        .get(parent_fanout_id)
        .ok_or_else(|| {
            LinkerError::DynamicFanout(format!(
                "parent fanout {parent_fanout_id} was not materialized"
            ))
        })?
        .nodes
        .clone();
    let module_ref = store
        .modules
        .get(&fanout.module)
        .ok_or_else(|| LinkerError::UnknownModule(fanout.module.clone()))?;
    let module = air_parser::parse_air_file(base_dir.join(&module_ref.path))?;
    let verification = air_verify::verify(&module);
    if !verification.is_success() {
        return Err(LinkerError::ModuleVerify {
            module: fanout.module.clone(),
            diagnostics: verification.diagnostics,
        });
    }

    let mut created = Vec::new();
    for (parent_index, parent_node) in parent_nodes.iter().enumerate() {
        let source = replace_parent_endpoint(&fanout.source, &parent_node.id)?;
        let source_endpoint = parse_endpoint(&source)?;
        let source_value = read_endpoint_value(root_inputs, module_outputs, &source_endpoint)?;
        let items = source_value.as_array().ok_or_else(|| {
            LinkerError::DynamicFanout(format!(
                "fanout {} source {} must evaluate to an array",
                fanout.id, source
            ))
        })?;
        enforce_dynamic_item_bounds(fanout, items.len())?;
        for index in 0..items.len() {
            created.push(MaterializedFanoutNode {
                id: dynamic_child_item_id(fanout, parent_index, index),
                after: parent_node.id.clone(),
                source: source.clone(),
                item_index: index,
            });
        }
    }
    let created_ids = materialized_node_ids(&created);
    let event = TraceEvent {
        agent: "$planner".to_string(),
        step: trace.len() as u32,
        rule: "dynamic_fanout".to_string(),
        action: "materialize".to_string(),
        input: Some(json!({
            "id": fanout.id,
            "after": parent_fanout_id,
            "source": fanout.source,
            "module": fanout.module,
            "max_items": fanout.max_items,
        })),
        output: Some(json!({
            "nodes": &created_ids,
        })),
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    };
    observer(&event);
    trace.push(event);

    for node in &created {
        if module_outputs.contains_key(&node.id) {
            continue;
        }
        let source_endpoint = parse_endpoint(&node.source)?;
        let source_value = read_endpoint_value(root_inputs, module_outputs, &source_endpoint)?;
        let item = source_value
            .as_array()
            .and_then(|items| items.get(node.item_index))
            .ok_or_else(|| {
                LinkerError::DynamicFanout(format!(
                    "fanout {} source {} missing item {}",
                    fanout.id, node.source, node.item_index
                ))
            })?;
        let module_inputs = build_dynamic_fanout_inputs(fanout, item, root_inputs, module_outputs)?;
        let event = TraceEvent {
            agent: "$system".to_string(),
            step: trace.len() as u32,
            rule: "dynamic_fanout".to_string(),
            action: "run_node_start".to_string(),
            input: Some(json!({
                "fanout": fanout.id,
                "node": node.id,
                "index": node.item_index,
                "module": fanout.module,
                "item": item,
            })),
            output: None,
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        };
        observer(&event);
        trace.push(event);

        let mut scoped_trace = Vec::new();
        let result = vm
            .run_with_observer(&module, module_inputs, |event| {
                let scoped = scoped_dynamic_event(&node.id, event);
                observer(&scoped);
                scoped_trace.push(scoped);
            })
            .map_err(|source| LinkerError::Runtime {
                module: node.id.clone(),
                source,
            })?;
        let output_keys = result.outputs.keys().cloned().collect::<Vec<_>>();
        trace.extend(scoped_trace);
        module_outputs.insert(node.id.clone(), result.outputs);
        checkpoint(&SystemCheckpoint {
            completed_module: node.id.clone(),
            module_outputs: module_outputs.clone(),
        })
        .map_err(LinkerError::Checkpoint)?;
        let event = TraceEvent {
            agent: "$system".to_string(),
            step: trace.len() as u32,
            rule: "dynamic_fanout".to_string(),
            action: "run_node_done".to_string(),
            input: Some(json!({
                "fanout": fanout.id,
                "node": node.id,
                "index": node.item_index,
                "module": fanout.module,
            })),
            output: Some(json!({
                "outputs": output_keys,
            })),
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        };
        observer(&event);
        trace.push(event);
    }

    Ok(MaterializedFanout { nodes: created })
}

fn run_dynamic_fanout_parallel<T, M, TF, MF, F, C>(
    fanout: &DynamicFanout,
    store: &ModuleStore,
    base_dir: &Path,
    root_inputs: &State,
    module_outputs: &mut BTreeMap<String, State>,
    tools_factory: &TF,
    models_factory: &MF,
    trace: &mut Vec<TraceEvent>,
    observer: &mut F,
    checkpoint: &mut C,
) -> Result<MaterializedFanout, LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let source = parse_endpoint(&fanout.source)?;
    let source_value = read_endpoint_value(root_inputs, module_outputs, &source)?;
    let items = source_value.as_array().cloned().ok_or_else(|| {
        LinkerError::DynamicFanout(format!(
            "fanout {} source {} must evaluate to an array",
            fanout.id, fanout.source
        ))
    })?;
    enforce_dynamic_item_bounds(fanout, items.len())?;

    let module_ref = store
        .modules
        .get(&fanout.module)
        .ok_or_else(|| LinkerError::UnknownModule(fanout.module.clone()))?;
    let module = air_parser::parse_air_file(base_dir.join(&module_ref.path))?;
    let verification = air_verify::verify(&module);
    if !verification.is_success() {
        return Err(LinkerError::ModuleVerify {
            module: fanout.module.clone(),
            diagnostics: verification.diagnostics,
        });
    }

    let created = (0..items.len())
        .map(|index| MaterializedFanoutNode {
            id: dynamic_item_id(fanout, index),
            after: fanout.after.clone(),
            source: fanout.source.clone(),
            item_index: index,
        })
        .collect::<Vec<_>>();
    let created_ids = materialized_node_ids(&created);
    let event = TraceEvent {
        agent: "$planner".to_string(),
        step: trace.len() as u32,
        rule: "dynamic_fanout".to_string(),
        action: "materialize".to_string(),
        input: Some(json!({
            "id": fanout.id,
            "source": fanout.source,
            "module": fanout.module,
            "max_items": fanout.max_items,
            "max_parallel": fanout.max_parallel,
        })),
        output: Some(json!({
            "nodes": &created_ids,
        })),
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    };
    observer(&event);
    trace.push(event);

    let max_parallel = fanout
        .max_parallel
        .unwrap_or_else(|| created_ids.len().max(1))
        .max(1);
    let event = TraceEvent {
        agent: "$system".to_string(),
        step: trace.len() as u32,
        rule: "dynamic_fanout".to_string(),
        action: "schedule_batch".to_string(),
        input: Some(json!({
            "fanout": fanout.id,
            "nodes": &created_ids,
            "max_parallel": max_parallel,
            "execution": if created_ids.len() > 1 { "parallel" } else { "sequential" },
        })),
        output: None,
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    };
    observer(&event);
    trace.push(event);

    let mut jobs = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let module_id = created[index].id.clone();
        if module_outputs.contains_key(&module_id) {
            continue;
        }
        let module_inputs = build_dynamic_fanout_inputs(fanout, item, root_inputs, module_outputs)?;
        let event = TraceEvent {
            agent: "$system".to_string(),
            step: trace.len() as u32,
            rule: "dynamic_fanout".to_string(),
            action: "run_node_start".to_string(),
            input: Some(json!({
                "fanout": fanout.id,
                "node": module_id,
                "index": index,
                "module": fanout.module,
                "item": item,
            })),
            output: None,
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        };
        observer(&event);
        trace.push(event);
        jobs.push(DynamicFanoutJob {
            module_id,
            index,
            module: module.clone(),
            inputs: module_inputs,
        });
    }

    let mut pending = jobs.into_iter();
    loop {
        let batch = pending.by_ref().take(max_parallel).collect::<Vec<_>>();
        if batch.is_empty() {
            break;
        }

        let results = if batch.len() > 1 {
            run_dynamic_fanout_jobs_parallel(batch, tools_factory, models_factory)?
        } else {
            run_dynamic_fanout_jobs_sequential(batch, tools_factory, models_factory)?
        };

        for result in results {
            for event in &result.trace {
                observer(event);
            }
            trace.extend(result.trace);
            module_outputs.insert(result.module_id.clone(), result.outputs);
            checkpoint(&SystemCheckpoint {
                completed_module: result.module_id.clone(),
                module_outputs: module_outputs.clone(),
            })
            .map_err(LinkerError::Checkpoint)?;
            let event = TraceEvent {
                agent: "$system".to_string(),
                step: trace.len() as u32,
                rule: "dynamic_fanout".to_string(),
                action: "run_node_done".to_string(),
                input: Some(json!({
                    "fanout": fanout.id,
                    "node": result.module_id,
                    "index": result.index,
                    "module": fanout.module,
                })),
                output: Some(json!({
                    "outputs": result.output_keys,
                })),
                meta: None,
                status: TraceStatus::Ok,
                error: None,
            };
            observer(&event);
            trace.push(event);
        }
    }

    Ok(MaterializedFanout { nodes: created })
}

fn run_child_dynamic_fanouts_parallel<T, M, TF, MF, F, C>(
    dynamic: &RunPlanDynamic,
    after_fanout: &str,
    store: &ModuleStore,
    base_dir: &Path,
    root_inputs: &State,
    module_outputs: &mut BTreeMap<String, State>,
    tools_factory: &TF,
    models_factory: &MF,
    materialized: &mut BTreeMap<String, MaterializedFanout>,
    trace: &mut Vec<TraceEvent>,
    observer: &mut F,
    checkpoint: &mut C,
) -> Result<(), LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    for fanout in pending_dynamic_after(dynamic, materialized, after_fanout) {
        let created = run_child_dynamic_fanout_parallel(
            fanout,
            after_fanout,
            store,
            base_dir,
            root_inputs,
            module_outputs,
            tools_factory,
            models_factory,
            materialized,
            trace,
            observer,
            checkpoint,
        )?;
        materialized.insert(fanout.id.clone(), created);
        run_child_dynamic_fanouts_parallel(
            dynamic,
            &fanout.id,
            store,
            base_dir,
            root_inputs,
            module_outputs,
            tools_factory,
            models_factory,
            materialized,
            trace,
            observer,
            checkpoint,
        )?;
    }
    Ok(())
}

fn run_child_dynamic_fanout_parallel<T, M, TF, MF, F, C>(
    fanout: &DynamicFanout,
    parent_fanout_id: &str,
    store: &ModuleStore,
    base_dir: &Path,
    root_inputs: &State,
    module_outputs: &mut BTreeMap<String, State>,
    tools_factory: &TF,
    models_factory: &MF,
    materialized: &BTreeMap<String, MaterializedFanout>,
    trace: &mut Vec<TraceEvent>,
    observer: &mut F,
    checkpoint: &mut C,
) -> Result<MaterializedFanout, LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
    F: FnMut(&TraceEvent),
    C: FnMut(&SystemCheckpoint) -> Result<(), String>,
{
    let parent_nodes = materialized
        .get(parent_fanout_id)
        .ok_or_else(|| {
            LinkerError::DynamicFanout(format!(
                "parent fanout {parent_fanout_id} was not materialized"
            ))
        })?
        .nodes
        .clone();
    let module_ref = store
        .modules
        .get(&fanout.module)
        .ok_or_else(|| LinkerError::UnknownModule(fanout.module.clone()))?;
    let module = air_parser::parse_air_file(base_dir.join(&module_ref.path))?;
    let verification = air_verify::verify(&module);
    if !verification.is_success() {
        return Err(LinkerError::ModuleVerify {
            module: fanout.module.clone(),
            diagnostics: verification.diagnostics,
        });
    }

    let mut created = Vec::new();
    for (parent_index, parent_node) in parent_nodes.iter().enumerate() {
        let source = replace_parent_endpoint(&fanout.source, &parent_node.id)?;
        let source_endpoint = parse_endpoint(&source)?;
        let source_value = read_endpoint_value(root_inputs, module_outputs, &source_endpoint)?;
        let items = source_value.as_array().ok_or_else(|| {
            LinkerError::DynamicFanout(format!(
                "fanout {} source {} must evaluate to an array",
                fanout.id, source
            ))
        })?;
        enforce_dynamic_item_bounds(fanout, items.len())?;
        for index in 0..items.len() {
            created.push(MaterializedFanoutNode {
                id: dynamic_child_item_id(fanout, parent_index, index),
                after: parent_node.id.clone(),
                source: source.clone(),
                item_index: index,
            });
        }
    }
    let created_ids = materialized_node_ids(&created);
    let event = TraceEvent {
        agent: "$planner".to_string(),
        step: trace.len() as u32,
        rule: "dynamic_fanout".to_string(),
        action: "materialize".to_string(),
        input: Some(json!({
            "id": fanout.id,
            "after": parent_fanout_id,
            "source": fanout.source,
            "module": fanout.module,
            "max_items": fanout.max_items,
            "max_parallel": fanout.max_parallel,
        })),
        output: Some(json!({
            "nodes": &created_ids,
        })),
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    };
    observer(&event);
    trace.push(event);

    let max_parallel = fanout
        .max_parallel
        .unwrap_or_else(|| created_ids.len().max(1))
        .max(1);
    let event = TraceEvent {
        agent: "$system".to_string(),
        step: trace.len() as u32,
        rule: "dynamic_fanout".to_string(),
        action: "schedule_batch".to_string(),
        input: Some(json!({
            "fanout": fanout.id,
            "nodes": &created_ids,
            "max_parallel": max_parallel,
            "execution": if created_ids.len() > 1 { "parallel" } else { "sequential" },
        })),
        output: None,
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    };
    observer(&event);
    trace.push(event);

    let mut jobs = Vec::new();
    for node in &created {
        if module_outputs.contains_key(&node.id) {
            continue;
        }
        let source_endpoint = parse_endpoint(&node.source)?;
        let source_value = read_endpoint_value(root_inputs, module_outputs, &source_endpoint)?;
        let item = source_value
            .as_array()
            .and_then(|items| items.get(node.item_index))
            .ok_or_else(|| {
                LinkerError::DynamicFanout(format!(
                    "fanout {} source {} missing item {}",
                    fanout.id, node.source, node.item_index
                ))
            })?;
        let module_inputs = build_dynamic_fanout_inputs(fanout, item, root_inputs, module_outputs)?;
        let event = TraceEvent {
            agent: "$system".to_string(),
            step: trace.len() as u32,
            rule: "dynamic_fanout".to_string(),
            action: "run_node_start".to_string(),
            input: Some(json!({
                "fanout": fanout.id,
                "node": node.id,
                "index": node.item_index,
                "module": fanout.module,
                "item": item,
            })),
            output: None,
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        };
        observer(&event);
        trace.push(event);
        jobs.push(DynamicFanoutJob {
            module_id: node.id.clone(),
            index: node.item_index,
            module: module.clone(),
            inputs: module_inputs,
        });
    }

    let mut pending = jobs.into_iter();
    loop {
        let batch = pending.by_ref().take(max_parallel).collect::<Vec<_>>();
        if batch.is_empty() {
            break;
        }

        let results = if batch.len() > 1 {
            run_dynamic_fanout_jobs_parallel(batch, tools_factory, models_factory)?
        } else {
            run_dynamic_fanout_jobs_sequential(batch, tools_factory, models_factory)?
        };

        for result in results {
            for event in &result.trace {
                observer(event);
            }
            trace.extend(result.trace);
            module_outputs.insert(result.module_id.clone(), result.outputs);
            checkpoint(&SystemCheckpoint {
                completed_module: result.module_id.clone(),
                module_outputs: module_outputs.clone(),
            })
            .map_err(LinkerError::Checkpoint)?;
            let event = TraceEvent {
                agent: "$system".to_string(),
                step: trace.len() as u32,
                rule: "dynamic_fanout".to_string(),
                action: "run_node_done".to_string(),
                input: Some(json!({
                    "fanout": fanout.id,
                    "node": result.module_id,
                    "index": result.index,
                    "module": fanout.module,
                })),
                output: Some(json!({
                    "outputs": result.output_keys,
                })),
                meta: None,
                status: TraceStatus::Ok,
                error: None,
            };
            observer(&event);
            trace.push(event);
        }
    }

    Ok(MaterializedFanout { nodes: created })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DynamicFanoutJob {
    module_id: String,
    index: usize,
    module: AirModule,
    inputs: State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DynamicFanoutJobResult {
    module_id: String,
    index: usize,
    outputs: State,
    output_keys: Vec<String>,
    trace: Vec<TraceEvent>,
}

fn run_dynamic_fanout_jobs_sequential<T, M, TF, MF>(
    jobs: Vec<DynamicFanoutJob>,
    tools_factory: &TF,
    models_factory: &MF,
) -> Result<Vec<DynamicFanoutJobResult>, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    TF: Fn() -> T,
    MF: Fn() -> M,
{
    jobs.into_iter()
        .map(|job| run_dynamic_fanout_job(job, tools_factory, models_factory))
        .collect()
}

fn run_dynamic_fanout_jobs_parallel<T, M, TF, MF>(
    jobs: Vec<DynamicFanoutJob>,
    tools_factory: &TF,
    models_factory: &MF,
) -> Result<Vec<DynamicFanoutJobResult>, LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
{
    std::thread::scope(|scope| {
        let handles = jobs
            .into_iter()
            .map(|job| {
                scope.spawn(move || run_dynamic_fanout_job(job, tools_factory, models_factory))
            })
            .collect::<Vec<_>>();

        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| LinkerError::ParallelWorkerPanic)?
            })
            .collect()
    })
}

fn run_dynamic_fanout_job<T, M, TF, MF>(
    job: DynamicFanoutJob,
    tools_factory: &TF,
    models_factory: &MF,
) -> Result<DynamicFanoutJobResult, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    TF: Fn() -> T,
    MF: Fn() -> M,
{
    let mut vm = Vm {
        tools: tools_factory(),
        models: models_factory(),
    };
    let result = vm
        .run(&job.module, job.inputs)
        .map_err(|source| LinkerError::Runtime {
            module: job.module_id.clone(),
            source,
        })?;
    let output_keys = result.outputs.keys().cloned().collect::<Vec<_>>();
    let trace = result
        .trace
        .iter()
        .map(|event| scoped_dynamic_event(&job.module_id, event))
        .collect();

    Ok(DynamicFanoutJobResult {
        module_id: job.module_id,
        index: job.index,
        outputs: result.outputs,
        output_keys,
        trace,
    })
}

fn build_dynamic_fanout_inputs(
    fanout: &DynamicFanout,
    item: &Value,
    root_inputs: &State,
    module_outputs: &BTreeMap<String, State>,
) -> Result<State, LinkerError> {
    let mut inputs = State::new();
    for connection in &fanout.input {
        let target = parse_each_endpoint(&connection.to)?;
        if target.append || !target.path.is_empty() {
            return Err(LinkerError::InvalidEndpoint(connection.to.clone()));
        }
        let source = connection_source_expr(connection).map_err(LinkerError::InvalidEndpoint)?;
        let value = eval_dynamic_link_expr(&source, item, root_inputs, module_outputs)?;
        inputs.insert(target.field, value);
    }
    Ok(inputs)
}

fn read_dynamic_source<'a>(
    source: &str,
    item: &'a Value,
    root_inputs: &'a State,
    module_outputs: &'a BTreeMap<String, State>,
) -> Result<&'a Value, LinkerError> {
    if source == "$item" {
        return Ok(item);
    }
    if let Some(path) = source.strip_prefix("$item.") {
        let normalized = normalize_endpoint_path(path)?;
        let path = normalized
            .split('.')
            .map(str::to_string)
            .collect::<Vec<_>>();
        return read_json_path(item, &path).ok_or_else(|| LinkerError::MissingOutput {
            module: "$item".to_string(),
            field: path.join("."),
        });
    }

    let endpoint = parse_endpoint(source)?;
    read_endpoint_value(root_inputs, module_outputs, &endpoint)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MaterializedFanout {
    nodes: Vec<MaterializedFanoutNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MaterializedFanoutNode {
    id: String,
    after: String,
    source: String,
    item_index: usize,
}

fn materialized_node_ids(nodes: &[MaterializedFanoutNode]) -> Vec<String> {
    nodes.iter().map(|node| node.id.clone()).collect()
}

fn materialize_dynamic_run_plan(
    plan: &RunPlan,
    materialized: &BTreeMap<String, MaterializedFanout>,
) -> Result<RunPlan, LinkerError> {
    materialize_dynamic_run_plan_with_mode(plan, materialized, true)
}

fn materialize_partial_dynamic_run_plan(
    plan: &RunPlan,
    materialized: &BTreeMap<String, MaterializedFanout>,
) -> Result<RunPlan, LinkerError> {
    materialize_dynamic_run_plan_with_mode(plan, materialized, false)
}

fn materialize_dynamic_run_plan_with_mode(
    plan: &RunPlan,
    materialized: &BTreeMap<String, MaterializedFanout>,
    require_all: bool,
) -> Result<RunPlan, LinkerError> {
    let mut expanded = plan.clone();
    let Some(dynamic) = &plan.dynamic else {
        return Ok(expanded);
    };
    expanded.dynamic = if require_all {
        None
    } else {
        Some(RunPlanDynamic {
            fanouts: dynamic
                .fanouts
                .iter()
                .filter(|fanout| !materialized.contains_key(&fanout.id))
                .cloned()
                .collect(),
        })
    };

    let mut schedule_groups = expanded.schedule.take().unwrap_or(SystemSchedule {
        max_parallel: None,
        groups: Vec::new(),
    });

    for fanout in &dynamic.fanouts {
        let Some(materialized_fanout) = materialized.get(&fanout.id) else {
            if require_all {
                return Err(LinkerError::DynamicFanout(format!(
                    "fanout {} was not materialized",
                    fanout.id
                )));
            }
            continue;
        };
        let node_ids = materialized_node_ids(&materialized_fanout.nodes);
        for node in &materialized_fanout.nodes {
            expanded.nodes.push(RunPlanNode {
                id: node.id.clone(),
                module: fanout.module.clone(),
                when: None,
            });
            push_unique_edge(
                &mut expanded.edges,
                SystemEdge {
                    from: node.after.clone(),
                    to: node.id.clone(),
                },
            );
            for connection in &fanout.input {
                expanded.connect.push(Connection {
                    from: connection
                        .from
                        .as_ref()
                        .map(|from| replace_item_source(from, &node.source, node.item_index)),
                    to: replace_each_endpoint(&connection.to, &node.id),
                    value: connection.value.as_ref().map(|value| {
                        replace_item_sources_in_expr(value, &node.source, node.item_index)
                    }),
                });
                if let Some(from) = &connection.from {
                    if let Ok(endpoint) = parse_endpoint(from) {
                        if endpoint.module != "$input" {
                            push_unique_edge(
                                &mut expanded.edges,
                                SystemEdge {
                                    from: endpoint.module,
                                    to: node.id.clone(),
                                },
                            );
                        }
                    }
                }
                for module in link_expr_source_modules(connection.value.as_ref()) {
                    if module != "$input" {
                        push_unique_edge(
                            &mut expanded.edges,
                            SystemEdge {
                                from: module,
                                to: node.id.clone(),
                            },
                        );
                    }
                }
            }
            for connection in &fanout.fan_in {
                let target = parse_endpoint(&connection.to)?;
                push_unique_edge(
                    &mut expanded.edges,
                    SystemEdge {
                        from: node.id.clone(),
                        to: target.module,
                    },
                );
                expanded.connect.push(Connection {
                    from: connection
                        .from
                        .as_ref()
                        .map(|from| replace_each_endpoint(from, &node.id)),
                    to: connection.to.clone(),
                    value: connection
                        .value
                        .as_ref()
                        .map(|value| replace_each_endpoints_in_expr(value, &node.id)),
                });
            }
        }

        if !node_ids.is_empty()
            && materialized_fanout
                .nodes
                .iter()
                .all(|node| node.after == materialized_fanout.nodes[0].after)
        {
            schedule_groups.groups.push(ScheduleGroup {
                id: fanout.id.clone(),
                nodes: node_ids,
                max_parallel: fanout.max_parallel,
            });
        } else if !node_ids.is_empty() {
            let mut grouped: BTreeMap<&str, Vec<String>> = BTreeMap::new();
            for node in &materialized_fanout.nodes {
                grouped
                    .entry(node.after.as_str())
                    .or_default()
                    .push(node.id.clone());
            }
            for (after, nodes) in grouped {
                schedule_groups.groups.push(ScheduleGroup {
                    id: format!("{}_after_{}", fanout.id, after),
                    nodes,
                    max_parallel: fanout.max_parallel,
                });
            }
        }
    }

    if schedule_groups.max_parallel.is_some() || !schedule_groups.groups.is_empty() {
        expanded.schedule = Some(schedule_groups);
    }

    Ok(expanded)
}

fn pending_dynamic_after<'a>(
    dynamic: &'a RunPlanDynamic,
    materialized: &BTreeMap<String, MaterializedFanout>,
    after: &str,
) -> Vec<&'a DynamicFanout> {
    dynamic
        .fanouts
        .iter()
        .filter(|fanout| fanout.after == after && !materialized.contains_key(&fanout.id))
        .collect()
}

fn missing_dynamic_fanouts<'a>(
    dynamic: &'a RunPlanDynamic,
    materialized: &BTreeMap<String, MaterializedFanout>,
) -> Vec<&'a str> {
    dynamic
        .fanouts
        .iter()
        .filter(|fanout| !materialized.contains_key(&fanout.id))
        .map(|fanout| fanout.id.as_str())
        .collect()
}

fn push_resolved_dynamic_plan_event<F>(
    plan: &RunPlan,
    materialized: &BTreeMap<String, MaterializedFanout>,
    trace: &mut Vec<TraceEvent>,
    observer: &mut F,
) -> Result<RunPlan, LinkerError>
where
    F: FnMut(&TraceEvent),
{
    let expanded_plan = materialize_dynamic_run_plan(plan, materialized)?;
    let event = TraceEvent {
        agent: "$planner".to_string(),
        step: trace.len() as u32,
        rule: "dynamic_fanout".to_string(),
        action: "resolve_dynamic_plan".to_string(),
        input: Some(json!({
            "fanouts": materialized.keys().collect::<Vec<_>>(),
        })),
        output: Some(serde_json::to_value(&expanded_plan).map_err(|error| {
            LinkerError::DynamicFanout(format!("failed to serialize dynamic RunPlan: {error}"))
        })?),
        meta: None,
        status: TraceStatus::Ok,
        error: None,
    };
    observer(&event);
    trace.push(event);
    Ok(expanded_plan)
}

fn dynamic_item_id(fanout: &DynamicFanout, index: usize) -> String {
    format!("{}_{}", fanout.id_prefix, index + 1)
}

fn dynamic_child_item_id(fanout: &DynamicFanout, parent_index: usize, index: usize) -> String {
    format!("{}_{}_{}", fanout.id_prefix, parent_index + 1, index + 1)
}

fn enforce_dynamic_item_bounds(fanout: &DynamicFanout, len: usize) -> Result<(), LinkerError> {
    if let Some(min_items) = fanout.min_items {
        if len < min_items {
            return Err(LinkerError::DynamicFanout(format!(
                "fanout {} expected at least {min_items} items, got {}",
                fanout.id, len
            )));
        }
    }
    if len > fanout.max_items {
        return Err(LinkerError::DynamicFanout(format!(
            "fanout {} expected at most {} items, got {}",
            fanout.id, fanout.max_items, len
        )));
    }
    Ok(())
}

fn scoped_dynamic_event(module_id: &str, event: &TraceEvent) -> TraceEvent {
    let mut event = event.clone();
    event.agent = format!("{module_id}/{}", event.agent);
    event
}

fn push_unique_edge(edges: &mut Vec<SystemEdge>, edge: SystemEdge) {
    if !edges
        .iter()
        .any(|existing| existing.from == edge.from && existing.to == edge.to)
    {
        edges.push(edge);
    }
}

fn dynamic_placeholder_id(fanout: &DynamicFanout) -> String {
    format!("{}__item", fanout.id_prefix)
}

fn replace_item_source(source: &str, fanout_source: &str, index: usize) -> String {
    if source == "$item" {
        return format!("{fanout_source}[{index}]");
    }
    if let Some(path) = source.strip_prefix("$item.") {
        return format!("{fanout_source}[{index}].{path}");
    }
    source.to_string()
}

fn replace_parent_endpoint(source: &str, parent_node_id: &str) -> Result<String, LinkerError> {
    if source == "$parent" {
        return Err(LinkerError::InvalidEndpoint(source.to_string()));
    }
    if let Some(path) = source.strip_prefix("$parent.") {
        return Ok(format!("{parent_node_id}.{path}"));
    }
    Ok(source.to_string())
}

fn replace_each_endpoint(endpoint: &str, node_id: &str) -> String {
    if endpoint == "$each" {
        return node_id.to_string();
    }
    endpoint
        .strip_prefix("$each.")
        .map(|field| format!("{node_id}.{field}"))
        .unwrap_or_else(|| endpoint.to_string())
}

fn is_each_endpoint(endpoint: &str) -> bool {
    endpoint == "$each" || endpoint.starts_with("$each.")
}

fn parse_each_endpoint(endpoint: &str) -> Result<Endpoint, LinkerError> {
    parse_endpoint(&replace_each_endpoint(endpoint, "$each"))
}

fn build_module_inputs(
    module_id: &str,
    root_inputs: &State,
    module_outputs: &BTreeMap<String, State>,
    incoming: &BTreeMap<String, Vec<ParsedConnection>>,
    skipped_modules: &BTreeSet<String>,
) -> Result<State, LinkerError> {
    let mut inputs = State::new();
    if module_outputs.is_empty() {
        inputs.extend(root_inputs.clone());
    }

    for connection in incoming.get(module_id).into_iter().flatten() {
        if link_expr_source_modules(Some(&connection.source))
            .iter()
            .any(|module| skipped_modules.contains(module))
            && connection.mode == ConnectionMode::Append
        {
            continue;
        }
        let value = eval_link_expr(&connection.source, root_inputs, module_outputs)?;
        match connection.mode {
            ConnectionMode::Direct => {
                inputs.insert(connection.to_field.clone(), value);
            }
            ConnectionMode::Append => {
                let entry = inputs
                    .entry(connection.to_field.clone())
                    .or_insert_with(|| json!([]));
                let Some(values) = entry.as_array_mut() else {
                    return Err(LinkerError::InvalidEndpoint(format!(
                        "{}[]",
                        connection.to_field
                    )));
                };
                values.push(value);
            }
        }
    }

    Ok(inputs)
}

fn apply_output_overrides(
    module_outputs: &mut BTreeMap<String, State>,
    overrides: &BTreeMap<String, serde_json::Value>,
) -> Result<(), LinkerError> {
    for (endpoint, override_value) in overrides {
        let endpoint = parse_endpoint(endpoint)?;
        if endpoint.module == "$input" || endpoint.append {
            return Err(LinkerError::InvalidEndpoint(format!(
                "{}.{}",
                endpoint.module,
                endpoint.display_field()
            )));
        }
        let source_outputs =
            module_outputs
                .get_mut(&endpoint.module)
                .ok_or_else(|| LinkerError::MissingOutput {
                    module: endpoint.module.clone(),
                    field: endpoint.field.clone(),
                })?;
        let value =
            source_outputs
                .get_mut(&endpoint.field)
                .ok_or_else(|| LinkerError::MissingOutput {
                    module: endpoint.module.clone(),
                    field: endpoint.field.clone(),
                })?;
        set_json_path(value, &endpoint.path, override_value.clone()).ok_or_else(|| {
            LinkerError::MissingOutput {
                module: endpoint.module.clone(),
                field: endpoint.display_field(),
            }
        })?;
    }
    Ok(())
}

fn collect_system_outputs(
    system: &AirSystem,
    module_outputs: &BTreeMap<String, State>,
) -> Result<State, LinkerError> {
    collect_outputs(&system.outputs, module_outputs)
}

fn collect_outputs(
    output_endpoints: &BTreeMap<String, String>,
    module_outputs: &BTreeMap<String, State>,
) -> Result<State, LinkerError> {
    let mut outputs = State::new();

    for (name, endpoint) in output_endpoints {
        let endpoint = parse_endpoint(endpoint)?;
        let source_outputs =
            module_outputs
                .get(&endpoint.module)
                .ok_or_else(|| LinkerError::MissingOutput {
                    module: endpoint.module.clone(),
                    field: endpoint.field.clone(),
                })?;
        let value = source_outputs
            .get(&endpoint.field)
            .and_then(|value| read_json_path(value, &endpoint.path))
            .ok_or_else(|| LinkerError::MissingOutput {
                module: endpoint.module.clone(),
                field: endpoint.display_field(),
            })?;
        outputs.insert(name.clone(), value.clone());
    }

    Ok(outputs)
}

fn triggered_halt<'a>(
    system: &'a AirSystem,
    after_module: &str,
    module_outputs: &BTreeMap<String, State>,
) -> Result<Option<&'a SystemHalt>, LinkerError> {
    for halt in system
        .halts
        .iter()
        .filter(|halt| halt.after == after_module)
    {
        let endpoint = parse_endpoint(&halt.when.reference)?;
        let source_outputs =
            module_outputs
                .get(&endpoint.module)
                .ok_or_else(|| LinkerError::MissingOutput {
                    module: endpoint.module.clone(),
                    field: endpoint.field.clone(),
                })?;
        let value = source_outputs
            .get(&endpoint.field)
            .and_then(|value| read_json_path(value, &endpoint.path))
            .ok_or_else(|| LinkerError::MissingOutput {
                module: endpoint.module.clone(),
                field: endpoint.display_field(),
            })?;
        if value == &halt.when.equals {
            return Ok(Some(halt));
        }
    }
    Ok(None)
}

fn link_condition_matches(
    condition: &str,
    root_inputs: &State,
    module_outputs: &BTreeMap<String, State>,
) -> Result<bool, LinkerError> {
    for group in condition.split("||") {
        let mut group_matches = true;
        for clause in group.split("&&") {
            if !link_condition_clause_matches(clause.trim(), root_inputs, module_outputs)? {
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

fn link_condition_clause_matches(
    clause: &str,
    root_inputs: &State,
    module_outputs: &BTreeMap<String, State>,
) -> Result<bool, LinkerError> {
    let (left, right, expected_equal) = if let Some((left, right)) = clause.split_once("==") {
        (left, right, true)
    } else if let Some((left, right)) = clause.split_once("!=") {
        (left, right, false)
    } else {
        return Err(LinkerError::InvalidEndpoint(clause.to_string()));
    };
    let endpoint = parse_endpoint(left.trim())?;
    let actual = read_endpoint_value(root_inputs, module_outputs, &endpoint)?;
    let expected = parse_link_condition_literal(right.trim())?;
    Ok((actual == &expected) == expected_equal)
}

fn read_endpoint_value<'a>(
    root_inputs: &'a State,
    module_outputs: &'a BTreeMap<String, State>,
    endpoint: &Endpoint,
) -> Result<&'a serde_json::Value, LinkerError> {
    if endpoint.module == "$input" {
        let value = root_inputs
            .get(&endpoint.field)
            .ok_or_else(|| LinkerError::MissingRootInput(endpoint.field.clone()))?;
        return read_json_path(value, &endpoint.path).ok_or_else(|| LinkerError::MissingOutput {
            module: endpoint.module.clone(),
            field: endpoint.display_field(),
        });
    }

    let source_outputs =
        module_outputs
            .get(&endpoint.module)
            .ok_or_else(|| LinkerError::MissingOutput {
                module: endpoint.module.clone(),
                field: endpoint.field.clone(),
            })?;
    let value = source_outputs
        .get(&endpoint.field)
        .ok_or_else(|| LinkerError::MissingOutput {
            module: endpoint.module.clone(),
            field: endpoint.field.clone(),
        })?;
    read_json_path(value, &endpoint.path).ok_or_else(|| LinkerError::MissingOutput {
        module: endpoint.module.clone(),
        field: endpoint.display_field(),
    })
}

fn parse_link_condition_literal(raw: &str) -> Result<serde_json::Value, LinkerError> {
    if raw.is_empty() {
        return Err(LinkerError::InvalidEndpoint(raw.to_string()));
    }
    if (raw.starts_with('"') && raw.ends_with('"'))
        || (raw.starts_with('\'') && raw.ends_with('\''))
    {
        return Ok(serde_json::Value::String(raw[1..raw.len() - 1].to_string()));
    }
    serde_json::from_str(raw).map_err(|_| LinkerError::InvalidEndpoint(raw.to_string()))
}

fn connection_source_expr(connection: &Connection) -> Result<LinkExpr, String> {
    match (&connection.from, &connection.value) {
        (Some(_), Some(_)) => Err(format!(
            "connect target {} cannot declare both from and value",
            connection.to
        )),
        (None, None) => Err(format!(
            "connect target {} must declare from or value",
            connection.to
        )),
        (Some(from), None) => Ok(LinkExpr::From { from: from.clone() }),
        (None, Some(value)) => Ok(value.clone()),
    }
}

fn eval_link_expr(
    expr: &LinkExpr,
    root_inputs: &State,
    module_outputs: &BTreeMap<String, State>,
) -> Result<Value, LinkerError> {
    match expr {
        LinkExpr::From { from } => {
            let endpoint = parse_endpoint(from)?;
            read_endpoint_value(root_inputs, module_outputs, &endpoint).cloned()
        }
        LinkExpr::Literal { literal } => Ok(literal.clone()),
        LinkExpr::Object { object } => {
            let mut out = serde_json::Map::new();
            for (key, value) in object {
                out.insert(
                    key.clone(),
                    eval_link_expr(value, root_inputs, module_outputs)?,
                );
            }
            Ok(Value::Object(out))
        }
        LinkExpr::Array { array } => {
            let mut out = Vec::new();
            for value in array {
                out.push(eval_link_expr(value, root_inputs, module_outputs)?);
            }
            Ok(Value::Array(out))
        }
        LinkExpr::Count { count } => {
            let value = eval_link_expr(count, root_inputs, module_outputs)?;
            let Some(array) = value.as_array() else {
                return Err(LinkerError::InvalidEndpoint("count".to_string()));
            };
            Ok(json!(array.len()))
        }
        LinkExpr::Coalesce { coalesce } => {
            for candidate in coalesce {
                match eval_link_expr(candidate, root_inputs, module_outputs) {
                    Ok(value) if !is_empty_link_value(&value) => return Ok(value),
                    Ok(_) => {}
                    Err(LinkerError::MissingOutput { .. } | LinkerError::MissingRootInput(_)) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(Value::Null)
        }
    }
}

fn eval_dynamic_link_expr(
    expr: &LinkExpr,
    item: &Value,
    root_inputs: &State,
    module_outputs: &BTreeMap<String, State>,
) -> Result<Value, LinkerError> {
    match expr {
        LinkExpr::From { from } => {
            read_dynamic_source(from, item, root_inputs, module_outputs).cloned()
        }
        LinkExpr::Literal { literal } => Ok(literal.clone()),
        LinkExpr::Object { object } => {
            let mut out = serde_json::Map::new();
            for (key, value) in object {
                out.insert(
                    key.clone(),
                    eval_dynamic_link_expr(value, item, root_inputs, module_outputs)?,
                );
            }
            Ok(Value::Object(out))
        }
        LinkExpr::Array { array } => {
            let mut out = Vec::new();
            for value in array {
                out.push(eval_dynamic_link_expr(
                    value,
                    item,
                    root_inputs,
                    module_outputs,
                )?);
            }
            Ok(Value::Array(out))
        }
        LinkExpr::Count { count } => {
            let value = eval_dynamic_link_expr(count, item, root_inputs, module_outputs)?;
            let Some(array) = value.as_array() else {
                return Err(LinkerError::InvalidEndpoint("count".to_string()));
            };
            Ok(json!(array.len()))
        }
        LinkExpr::Coalesce { coalesce } => {
            for candidate in coalesce {
                match eval_dynamic_link_expr(candidate, item, root_inputs, module_outputs) {
                    Ok(value) if !is_empty_link_value(&value) => return Ok(value),
                    Ok(_) => {}
                    Err(LinkerError::MissingOutput { .. } | LinkerError::MissingRootInput(_)) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(Value::Null)
        }
    }
}

fn is_empty_link_value(value: &Value) -> bool {
    value.is_null()
        || value.as_str().is_some_and(str::is_empty)
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
}

fn link_expr_source_modules(expr: Option<&LinkExpr>) -> BTreeSet<String> {
    let mut modules = BTreeSet::new();
    if let Some(expr) = expr {
        collect_link_expr_source_modules(expr, &mut modules);
    }
    modules
}

fn collect_link_expr_source_modules(expr: &LinkExpr, modules: &mut BTreeSet<String>) {
    match expr {
        LinkExpr::From { from } => {
            if let Ok(endpoint) = parse_endpoint(from) {
                if !endpoint.module.starts_with('$') {
                    modules.insert(endpoint.module);
                }
            }
        }
        LinkExpr::Object { object } => {
            for value in object.values() {
                collect_link_expr_source_modules(value, modules);
            }
        }
        LinkExpr::Array { array } | LinkExpr::Coalesce { coalesce: array } => {
            for value in array {
                collect_link_expr_source_modules(value, modules);
            }
        }
        LinkExpr::Count { count } => collect_link_expr_source_modules(count, modules),
        LinkExpr::Literal { .. } => {}
    }
}

fn link_expr_endpoints(expr: &LinkExpr) -> Vec<Endpoint> {
    let mut endpoints = Vec::new();
    collect_link_expr_endpoints(expr, &mut endpoints);
    endpoints
}

fn collect_link_expr_endpoints(expr: &LinkExpr, endpoints: &mut Vec<Endpoint>) {
    match expr {
        LinkExpr::From { from } => {
            if let Ok(endpoint) = parse_endpoint(from) {
                endpoints.push(endpoint);
            }
        }
        LinkExpr::Object { object } => {
            for value in object.values() {
                collect_link_expr_endpoints(value, endpoints);
            }
        }
        LinkExpr::Array { array } | LinkExpr::Coalesce { coalesce: array } => {
            for value in array {
                collect_link_expr_endpoints(value, endpoints);
            }
        }
        LinkExpr::Count { count } => collect_link_expr_endpoints(count, endpoints),
        LinkExpr::Literal { .. } => {}
    }
}

fn replace_item_sources_in_expr(expr: &LinkExpr, fanout_source: &str, index: usize) -> LinkExpr {
    match expr {
        LinkExpr::From { from } => LinkExpr::From {
            from: replace_item_source(from, fanout_source, index),
        },
        LinkExpr::Literal { literal } => LinkExpr::Literal {
            literal: literal.clone(),
        },
        LinkExpr::Object { object } => LinkExpr::Object {
            object: object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        replace_item_sources_in_expr(value, fanout_source, index),
                    )
                })
                .collect(),
        },
        LinkExpr::Array { array } => LinkExpr::Array {
            array: array
                .iter()
                .map(|value| replace_item_sources_in_expr(value, fanout_source, index))
                .collect(),
        },
        LinkExpr::Count { count } => LinkExpr::Count {
            count: Box::new(replace_item_sources_in_expr(count, fanout_source, index)),
        },
        LinkExpr::Coalesce { coalesce } => LinkExpr::Coalesce {
            coalesce: coalesce
                .iter()
                .map(|value| replace_item_sources_in_expr(value, fanout_source, index))
                .collect(),
        },
    }
}

fn replace_each_endpoints_in_expr(expr: &LinkExpr, node_id: &str) -> LinkExpr {
    match expr {
        LinkExpr::From { from } => LinkExpr::From {
            from: replace_each_endpoint(from, node_id),
        },
        LinkExpr::Literal { literal } => LinkExpr::Literal {
            literal: literal.clone(),
        },
        LinkExpr::Object { object } => LinkExpr::Object {
            object: object
                .iter()
                .map(|(key, value)| (key.clone(), replace_each_endpoints_in_expr(value, node_id)))
                .collect(),
        },
        LinkExpr::Array { array } => LinkExpr::Array {
            array: array
                .iter()
                .map(|value| replace_each_endpoints_in_expr(value, node_id))
                .collect(),
        },
        LinkExpr::Count { count } => LinkExpr::Count {
            count: Box::new(replace_each_endpoints_in_expr(count, node_id)),
        },
        LinkExpr::Coalesce { coalesce } => LinkExpr::Coalesce {
            coalesce: coalesce
                .iter()
                .map(|value| replace_each_endpoints_in_expr(value, node_id))
                .collect(),
        },
    }
}

fn validate_dynamic_link_expr_sources(expr: &LinkExpr) -> Result<(), String> {
    match expr {
        LinkExpr::From { from } => {
            if from == "$item" || from.starts_with("$item.") || parse_endpoint(from).is_ok() {
                Ok(())
            } else {
                Err(format!("source {from} must be $item or a normal endpoint"))
            }
        }
        LinkExpr::Object { object } => {
            for value in object.values() {
                validate_dynamic_link_expr_sources(value)?;
            }
            Ok(())
        }
        LinkExpr::Array { array } | LinkExpr::Coalesce { coalesce: array } => {
            for value in array {
                validate_dynamic_link_expr_sources(value)?;
            }
            Ok(())
        }
        LinkExpr::Count { count } => validate_dynamic_link_expr_sources(count),
        LinkExpr::Literal { .. } => Ok(()),
    }
}

fn link_expr_references_each(expr: &LinkExpr) -> bool {
    match expr {
        LinkExpr::From { from } => is_each_endpoint(from),
        LinkExpr::Object { object } => object.values().any(link_expr_references_each),
        LinkExpr::Array { array } | LinkExpr::Coalesce { coalesce: array } => {
            array.iter().any(link_expr_references_each)
        }
        LinkExpr::Count { count } => link_expr_references_each(count),
        LinkExpr::Literal { .. } => false,
    }
}

fn incoming_connections(
    system: &AirSystem,
) -> Result<BTreeMap<String, Vec<ParsedConnection>>, LinkerError> {
    let mut incoming: BTreeMap<String, Vec<ParsedConnection>> = BTreeMap::new();

    for connection in &system.connect {
        let to = parse_endpoint(&connection.to)?;
        let source = connection_source_expr(connection).map_err(LinkerError::InvalidEndpoint)?;
        incoming
            .entry(to.module.clone())
            .or_default()
            .push(ParsedConnection {
                source,
                to_field: to.field,
                mode: if to.append {
                    ConnectionMode::Append
                } else {
                    ConnectionMode::Direct
                },
            });
    }

    Ok(incoming)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutionBatch {
    group_id: Option<String>,
    modules: Vec<String>,
    max_parallel: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleJob {
    module_id: String,
    module: AirModule,
    inputs: State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleJobResult {
    module_id: String,
    result: RunResult,
}

fn run_module_jobs_sequential<T, M, TF, MF>(
    jobs: Vec<ModuleJob>,
    tools_factory: &TF,
    models_factory: &MF,
) -> Result<Vec<ModuleJobResult>, LinkerError>
where
    T: ToolProvider,
    M: ModelProvider,
    TF: Fn() -> T,
    MF: Fn() -> M,
{
    let mut results = Vec::new();
    for job in jobs {
        let mut vm = Vm {
            tools: tools_factory(),
            models: models_factory(),
        };
        let result = vm
            .run(&job.module, job.inputs)
            .map_err(|source| LinkerError::Runtime {
                module: job.module_id.clone(),
                source,
            })?;
        results.push(ModuleJobResult {
            module_id: job.module_id,
            result,
        });
    }
    Ok(results)
}

fn run_module_jobs_parallel<T, M, TF, MF>(
    jobs: Vec<ModuleJob>,
    tools_factory: &TF,
    models_factory: &MF,
) -> Result<Vec<ModuleJobResult>, LinkerError>
where
    T: ToolProvider + Send,
    M: ModelProvider + Send,
    TF: Fn() -> T + Sync,
    MF: Fn() -> M + Sync,
{
    std::thread::scope(|scope| {
        let handles = jobs
            .into_iter()
            .map(|job| {
                scope.spawn(move || {
                    let mut vm = Vm {
                        tools: tools_factory(),
                        models: models_factory(),
                    };
                    let result =
                        vm.run(&job.module, job.inputs)
                            .map_err(|source| LinkerError::Runtime {
                                module: job.module_id.clone(),
                                source,
                            })?;
                    Ok(ModuleJobResult {
                        module_id: job.module_id,
                        result,
                    })
                })
            })
            .collect::<Vec<_>>();

        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| LinkerError::ParallelWorkerPanic)?
            })
            .collect()
    })
}

fn execution_batches(system: &AirSystem) -> Result<Vec<ExecutionBatch>, LinkerError> {
    let Some(schedule) = &system.schedule else {
        return topological_order(system).map(|order| {
            order
                .into_iter()
                .map(|module| ExecutionBatch {
                    group_id: None,
                    modules: vec![module],
                    max_parallel: 1,
                })
                .collect()
        });
    };

    let mut indegree: BTreeMap<String, usize> = system
        .modules
        .keys()
        .map(|module| (module.clone(), 0))
        .collect();
    let mut outgoing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for edge in &system.edges {
        if !system.modules.contains_key(&edge.from) {
            return Err(LinkerError::UnknownModule(edge.from.clone()));
        }
        if !system.modules.contains_key(&edge.to) {
            return Err(LinkerError::UnknownModule(edge.to.clone()));
        }

        *indegree.entry(edge.to.clone()).or_default() += 1;
        outgoing
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }

    let mut remaining: BTreeSet<String> = system.modules.keys().cloned().collect();
    let mut completed = Vec::new();
    let mut batches = Vec::new();
    let default_max_parallel = schedule.max_parallel.unwrap_or(usize::MAX).max(1);

    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter(|module| indegree.get(*module).copied().unwrap_or_default() == 0)
            .cloned()
            .collect::<BTreeSet<_>>();
        if ready.is_empty() {
            return Err(LinkerError::Cycle);
        }

        let scheduled_group = schedule.groups.iter().find(|group| {
            let group_nodes = group.nodes.iter().cloned().collect::<BTreeSet<_>>();
            !group_nodes.is_empty()
                && group_nodes.iter().any(|node| remaining.contains(node))
                && group_nodes
                    .iter()
                    .filter(|node| remaining.contains(*node))
                    .all(|node| ready.contains(node))
        });

        let (group_id, max_parallel, selected) = if let Some(group) = scheduled_group {
            let group_max_parallel = group.max_parallel.unwrap_or(default_max_parallel).max(1);
            let limit = group_max_parallel.min(default_max_parallel);
            (
                Some(group.id.clone()),
                limit,
                group
                    .nodes
                    .iter()
                    .filter(|node| remaining.contains(*node))
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        } else {
            (None, 1, vec![ready.iter().next().expect("ready").clone()])
        };

        for module in &selected {
            remaining.remove(module);
            completed.push(module.clone());
            for next in outgoing.get(module).into_iter().flatten() {
                let degree = indegree.get_mut(next).expect("known module");
                *degree -= 1;
            }
        }

        batches.push(ExecutionBatch {
            group_id,
            modules: selected,
            max_parallel,
        });
    }

    if completed.len() != system.modules.len() {
        return Err(LinkerError::Cycle);
    }

    Ok(batches)
}

fn direct_predecessors(system: &AirSystem) -> BTreeMap<String, BTreeSet<String>> {
    let mut predecessors: BTreeMap<String, BTreeSet<String>> = system
        .modules
        .keys()
        .map(|module| (module.clone(), BTreeSet::new()))
        .collect();

    for edge in &system.edges {
        predecessors
            .entry(edge.to.clone())
            .or_default()
            .insert(edge.from.clone());
    }

    predecessors
}

fn topological_order(system: &AirSystem) -> Result<Vec<String>, LinkerError> {
    let mut indegree: BTreeMap<String, usize> = system
        .modules
        .keys()
        .map(|module| (module.clone(), 0))
        .collect();
    let mut outgoing: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for edge in &system.edges {
        if !system.modules.contains_key(&edge.from) {
            return Err(LinkerError::UnknownModule(edge.from.clone()));
        }
        if !system.modules.contains_key(&edge.to) {
            return Err(LinkerError::UnknownModule(edge.to.clone()));
        }

        *indegree.entry(edge.to.clone()).or_default() += 1;
        outgoing
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }

    let mut ready: Vec<_> = indegree
        .iter()
        .filter_map(|(module, degree)| (*degree == 0).then_some(module.clone()))
        .collect();
    let mut order = Vec::new();

    while let Some(module) = ready.pop() {
        order.push(module.clone());
        for next in outgoing.get(&module).into_iter().flatten() {
            let degree = indegree.get_mut(next).expect("known module");
            *degree -= 1;
            if *degree == 0 {
                ready.push(next.clone());
            }
        }
    }

    if order.len() != system.modules.len() {
        return Err(LinkerError::Cycle);
    }

    Ok(order)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    module: String,
    field: String,
    path: Vec<String>,
    append: bool,
}

impl Endpoint {
    fn display_field(&self) -> String {
        if self.path.is_empty() {
            return self.field.clone();
        }
        format!("{}.{}", self.field, self.path.join("."))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedConnection {
    source: LinkExpr,
    to_field: String,
    mode: ConnectionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionMode {
    Direct,
    Append,
}

fn parse_endpoint(endpoint: &str) -> Result<Endpoint, LinkerError> {
    let Some((module, rest)) = endpoint.split_once('.') else {
        return Err(LinkerError::InvalidEndpoint(endpoint.to_string()));
    };
    let append = rest.ends_with("[]");
    let rest = rest.strip_suffix("[]").unwrap_or(rest);
    let normalized = normalize_endpoint_path(rest)?;
    let mut segments = normalized.split('.');
    let field = segments.next().unwrap_or_default();
    let path = segments.map(str::to_string).collect::<Vec<_>>();
    if module.is_empty() || field.is_empty() || path.iter().any(|segment| segment.is_empty()) {
        return Err(LinkerError::InvalidEndpoint(endpoint.to_string()));
    }
    Ok(Endpoint {
        module: module.to_string(),
        field: field.to_string(),
        path,
        append,
    })
}

fn normalize_endpoint_path(path: &str) -> Result<String, LinkerError> {
    let mut normalized = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('[') {
        let (before, after_start) = rest.split_at(start);
        normalized.push_str(before);
        let after_start = &after_start[1..];
        let Some(end) = after_start.find(']') else {
            return Err(LinkerError::InvalidEndpoint(path.to_string()));
        };
        let (index, after_end) = after_start.split_at(end);
        if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(LinkerError::InvalidEndpoint(path.to_string()));
        }
        normalized.push('.');
        normalized.push_str(index);
        rest = &after_end[1..];
    }
    if rest.contains(']') {
        return Err(LinkerError::InvalidEndpoint(path.to_string()));
    }
    normalized.push_str(rest);
    Ok(normalized)
}

fn types_compatible(from: &TypeSpec, to: &TypeSpec) -> bool {
    from.kind_name() == to.kind_name()
}

fn nested_type<'a>(spec: &'a TypeSpec, path: &[String]) -> Option<&'a TypeSpec> {
    let mut current = spec;
    for segment in path {
        let TypeSpec::Detailed(detailed) = current else {
            return None;
        };
        if detailed.kind.kind_name() == "array" {
            segment.parse::<usize>().ok()?;
            current = detailed.items.as_deref()?;
        } else {
            current = detailed.properties.get(segment)?;
        }
    }
    Some(current)
}

fn read_json_path<'a>(
    value: &'a serde_json::Value,
    path: &[String],
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path {
        if let Some(object) = current.as_object() {
            current = object.get(segment)?;
        } else if let Some(array) = current.as_array() {
            current = array.get(segment.parse::<usize>().ok()?)?;
        } else {
            return None;
        }
    }
    Some(current)
}

fn set_json_path(
    value: &mut serde_json::Value,
    path: &[String],
    override_value: serde_json::Value,
) -> Option<()> {
    if path.is_empty() {
        *value = override_value;
        return Some(());
    }

    let mut current = value;
    for segment in &path[..path.len() - 1] {
        current = match current {
            serde_json::Value::Object(object) => object.get_mut(segment)?,
            serde_json::Value::Array(array) => array.get_mut(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }

    let last = path.last()?;
    match current {
        serde_json::Value::Object(object) => {
            object.insert(last.clone(), override_value);
            Some(())
        }
        serde_json::Value::Array(array) => {
            let index = last.parse::<usize>().ok()?;
            let slot = array.get_mut(index)?;
            *slot = override_value;
            Some(())
        }
        _ => None,
    }
}

fn is_array_type(spec: &TypeSpec) -> bool {
    spec.kind_name() == "array"
}

fn array_item_type(spec: &TypeSpec) -> Option<&TypeSpec> {
    match spec {
        TypeSpec::Detailed(detailed) if detailed.kind.kind_name() == "array" => {
            detailed.items.as_deref()
        }
        _ => None,
    }
}

fn array_bounds(spec: &TypeSpec) -> (Option<usize>, Option<usize>) {
    match spec {
        TypeSpec::Detailed(detailed) if detailed.kind.kind_name() == "array" => {
            (detailed.min_items, detailed.max_items)
        }
        _ => (None, None),
    }
}
