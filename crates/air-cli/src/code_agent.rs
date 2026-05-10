use crate::explain::build_plan_explanation;
use crate::planner::module_base_dir_for_store_path;
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::run_plan::{run_plan, run_plan_capture, RunPlanOptions};
use air_runtime::{read_trace_jsonl, TraceEvent, TraceStatus};
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_CONTEXT_MAX_CHARS: usize = 200_000;
const DEFAULT_CONTEXT_THRESHOLD_PERCENT: usize = 80;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum CodeRecipe {
    /// Select a recipe from typed flags, preferring read-only exploration when ambiguous.
    Auto,
    /// Project-level planning with a task graph, evidence, and acceptance criteria.
    Plan,
    /// Read-only repository exploration and planning.
    Explore,
    /// Grounded code review with repository and external evidence.
    Review,
    /// Core repair loop: explore, select context, patch, and retest.
    Repair,
    /// Build one bounded static page and verify it.
    Build,
}

pub(crate) struct CodeOptions {
    pub(crate) task: String,
    pub(crate) recipe: CodeRecipe,
    pub(crate) target: Option<PathBuf>,
    pub(crate) test: Option<String>,
    pub(crate) query: Option<String>,
    pub(crate) related: Vec<PathBuf>,
    pub(crate) search_query: Option<String>,
    pub(crate) repo_query: Option<String>,
    pub(crate) required_terms: Vec<String>,
    pub(crate) output: Option<PathBuf>,
    pub(crate) brand: Option<String>,
    pub(crate) product: Option<String>,
    pub(crate) constraints: Vec<String>,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) state_out: Option<PathBuf>,
    pub(crate) checkpoint_out: Option<PathBuf>,
    pub(crate) jit_cache: Option<PathBuf>,
    pub(crate) session: Option<PathBuf>,
    pub(crate) parallel: bool,
    pub(crate) log: bool,
    pub(crate) explain: bool,
    pub(crate) loop_enabled: bool,
    pub(crate) execute_plan: bool,
    pub(crate) max_iterations: usize,
    pub(crate) tool_config: Option<PathBuf>,
}

pub(crate) fn code(options: CodeOptions) -> Result<()> {
    let CodeOptions {
        task,
        recipe,
        target,
        test,
        query,
        related,
        search_query,
        repo_query,
        required_terms,
        output,
        brand,
        product,
        constraints,
        profile,
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        state_out,
        checkpoint_out,
        jit_cache,
        session,
        parallel,
        log,
        explain,
        loop_enabled,
        execute_plan,
        max_iterations,
        tool_config,
    } = options;

    let requested_recipe = recipe;
    let recipe = resolve_recipe(
        recipe,
        target.as_ref(),
        test.as_ref(),
        search_query.as_ref(),
        repo_query.as_ref(),
        &required_terms,
        output.as_ref(),
        brand.as_ref(),
        product.as_ref(),
        &constraints,
    );
    let profile = profile.unwrap_or_else(|| default_profile(recipe));
    let mut input = build_input(CodeInputOptions {
        task,
        recipe,
        target,
        test,
        query,
        related,
        search_query,
        repo_query,
        required_terms,
        output,
        brand,
        product,
        constraints,
    })?;
    let mut session_state = match session.as_ref() {
        Some(path) => Some(CodeSessionState::read(path)?),
        None => None,
    };
    if let Some(state) = session_state.as_ref() {
        input = code_session_turn_input(&input, &state.turns);
    }

    if explain {
        print_explain(
            requested_recipe,
            recipe,
            &profile,
            &input,
            loop_enabled,
            execute_plan,
            max_iterations,
        )?;
        return Ok(());
    }

    let effective_trace_out = match (&session, &trace_out) {
        (Some(path), None) => Some(default_session_trace_path(
            path,
            session_state
                .as_ref()
                .map_or(1, |state| state.turns.len() + 1),
        )),
        _ => trace_out.clone(),
    };
    if let Some(path) = effective_trace_out.as_ref() {
        ensure_parent_dir(path)?;
    }

    let session_input = input.clone();
    let outputs = if execute_plan {
        if recipe != CodeRecipe::Plan {
            bail!("air code --execute-plan requires the resolved recipe to be plan");
        }
        run_code_project(CodeProjectOptions {
            plan_profile: profile.clone(),
            input,
            model_config: model_config.clone(),
            trace_out: effective_trace_out.clone(),
            trace_redact,
            trace_raw,
            state_out: state_out.clone(),
            checkpoint_out: checkpoint_out.clone(),
            jit_cache: jit_cache.clone(),
            parallel,
            log,
            tool_config: tool_config.clone(),
            max_tasks: max_iterations,
        })?
    } else if loop_enabled {
        run_code_loop(CodeLoopOptions {
            recipe,
            profile: profile.clone(),
            input,
            model_config: model_config.clone(),
            trace_out: effective_trace_out.clone(),
            trace_redact,
            trace_raw,
            state_out: state_out.clone(),
            checkpoint_out: checkpoint_out.clone(),
            jit_cache: jit_cache.clone(),
            parallel,
            log,
            tool_config: tool_config.clone(),
            max_iterations,
        })?
    } else if session.is_some() {
        run_plan_capture(RunPlanOptions {
            plan: None,
            profile: Some(profile.clone()),
            store: None,
            input: None,
            input_values: Some(input.clone()),
            model_config: model_config.clone(),
            trace_out: effective_trace_out.clone(),
            trace_redact,
            trace_raw,
            state_out: state_out.clone(),
            checkpoint_out: checkpoint_out.clone(),
            jit_cache: jit_cache.clone(),
            parallel,
            log,
            example_tools: false,
            tool_config: tool_config.clone(),
        })?
    } else {
        return run_plan(RunPlanOptions {
            plan: None,
            profile: Some(profile),
            store: None,
            input: None,
            input_values: Some(input),
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            state_out,
            checkpoint_out,
            jit_cache,
            parallel,
            log,
            example_tools: false,
            tool_config,
        });
    };

    if let Some(path) = session.as_ref() {
        let state = session_state.get_or_insert_with(CodeSessionState::default);
        let turn_number = state.turns.len() + 1;
        let turn_time = CodeSessionTurnTime::now();
        let trace_files =
            code_session_trace_files(effective_trace_out.as_ref(), loop_enabled, &outputs);
        let parts = code_session_parts(&trace_files)?;
        let summary = code_session_turn_summary(&parts);
        state.append_turn(CodeSessionTurn {
            id: code_session_turn_id(turn_number),
            time: turn_time,
            task: session_input
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            recipe: recipe_name(recipe).to_string(),
            profile: path_ref_to_input_string(&profile),
            input: Value::Object(session_input),
            completed: code_outputs_complete(recipe, &outputs),
            trace_files: trace_files
                .iter()
                .map(|path| path_ref_to_input_string(path))
                .collect(),
            summary,
            parts,
            outputs: outputs.clone(),
        });
        state.write(path)?;
    }
    serde_json::to_writer_pretty(std::io::stdout(), &outputs)?;
    println!();
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CodeSessionState {
    #[serde(default = "code_session_version")]
    version: u32,
    #[serde(default)]
    turns: Vec<CodeSessionTurn>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CodeSessionTurn {
    #[serde(default)]
    id: String,
    #[serde(default)]
    time: CodeSessionTurnTime,
    task: String,
    recipe: String,
    profile: String,
    input: Value,
    completed: bool,
    #[serde(default)]
    trace_files: Vec<String>,
    #[serde(default)]
    summary: CodeSessionTurnSummary,
    #[serde(default)]
    parts: Vec<CodeSessionPart>,
    outputs: Value,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CodeSessionTurnTime {
    created: u64,
    updated: u64,
}

impl CodeSessionTurnTime {
    fn now() -> Self {
        let now = unix_millis();
        Self {
            created: now,
            updated: now,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CodeSessionTurnSummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    approvals: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifact_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifact_kinds: Vec<String>,
    model_call_count: usize,
    tool_call_count: usize,
    approval_count: usize,
    error_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CodeSessionPart {
    kind: String,
    trace_file: String,
    agent: String,
    step: u32,
    rule: String,
    action: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    approval_for: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifact_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifact_kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    input_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    output_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn code_session_version() -> u32 {
    1
}

fn code_session_turn_id(turn_number: usize) -> String {
    format!("turn-{turn_number:06}")
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

impl CodeSessionState {
    fn read(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                version: code_session_version(),
                turns: Vec::new(),
            });
        }
        let state: Self = serde_json::from_str(&fs::read_to_string(path)?)?;
        if state.version != code_session_version() {
            bail!(
                "unsupported AIR code session version {}; expected {}",
                state.version,
                code_session_version()
            );
        }
        Ok(state)
    }

    fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    fn append_turn(&mut self, turn: CodeSessionTurn) {
        self.version = code_session_version();
        self.turns.push(turn);
    }
}

fn default_session_trace_path(session_path: &Path, turn_number: usize) -> PathBuf {
    let parent = session_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = session_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("air-code-session");
    parent
        .join(format!("{stem}.traces"))
        .join(format!("turn{turn_number}.trace.jsonl"))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn code_session_trace_files(
    trace_out: Option<&PathBuf>,
    loop_enabled: bool,
    outputs: &Value,
) -> Vec<PathBuf> {
    if let Some(files) = outputs.get("trace_files").and_then(Value::as_array) {
        return files
            .iter()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect();
    }
    let Some(trace_out) = trace_out else {
        return Vec::new();
    };
    if !loop_enabled {
        return vec![trace_out.clone()];
    }
    let iterations = outputs
        .get("iterations")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    (1..=iterations)
        .map(|iteration| iteration_path(trace_out, iteration))
        .collect()
}

fn code_session_parts(trace_files: &[PathBuf]) -> Result<Vec<CodeSessionPart>> {
    let mut parts = Vec::new();
    for trace_file in trace_files {
        let events = read_trace_jsonl(trace_file).with_context(|| {
            format!(
                "failed to read AIR code session trace {}",
                trace_file.display()
            )
        })?;
        let trace_file = path_ref_to_input_string(trace_file);
        parts.extend(
            events
                .iter()
                .filter_map(|event| code_session_part_from_event(&trace_file, event)),
        );
    }
    Ok(parts)
}

fn code_session_turn_summary(parts: &[CodeSessionPart]) -> CodeSessionTurnSummary {
    let mut summary = CodeSessionTurnSummary::default();
    for part in parts {
        if part.kind == "model_call" {
            summary.model_call_count += 1;
        }
        if part.kind == "tool_call" {
            summary.tool_call_count += 1;
        }
        if part.kind == "approval" {
            summary.approval_count += 1;
        }
        if part.status == "error" {
            summary.error_count += 1;
        }
        if let Some(model) = &part.model {
            summary.models.push(model.clone());
        }
        if let Some(tool) = &part.tool {
            summary.tools.push(tool.clone());
        }
        summary.approvals.extend(part.approval_for.clone());
        summary.files.extend(part.files.clone());
        summary.artifact_ids.extend(part.artifact_ids.clone());
        summary.artifact_kinds.extend(part.artifact_kinds.clone());
    }
    sort_dedup(&mut summary.models);
    sort_dedup(&mut summary.tools);
    sort_dedup(&mut summary.approvals);
    sort_dedup(&mut summary.files);
    sort_dedup(&mut summary.artifact_ids);
    sort_dedup(&mut summary.artifact_kinds);
    summary
}

fn sort_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn code_session_part_from_event(trace_file: &str, event: &TraceEvent) -> Option<CodeSessionPart> {
    if !matches!(
        event.action.as_str(),
        "model_call" | "tool_call" | "approval" | "return"
    ) {
        return None;
    }
    let meta = event.meta.as_ref();
    Some(CodeSessionPart {
        kind: event.action.clone(),
        trace_file: trace_file.to_string(),
        agent: event.agent.clone(),
        step: event.step,
        rule: event.rule.clone(),
        action: event.action.clone(),
        status: trace_status_name(&event.status).to_string(),
        model: meta
            .and_then(|value| value.get("model"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tool: meta
            .and_then(|value| value.get("tool"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        approval_for: meta
            .and_then(|value| value.get("approval_for"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        files: output_files(event.output.as_ref()),
        artifact_ids: artifact_field_values(event.output.as_ref(), "id"),
        artifact_kinds: artifact_field_values(event.output.as_ref(), "kind"),
        input_keys: value_keys(event.input.as_ref()),
        output_keys: value_keys(event.output.as_ref()),
        error: event.error.clone(),
    })
}

fn trace_status_name(status: &TraceStatus) -> &'static str {
    match status {
        TraceStatus::Ok => "ok",
        TraceStatus::Error => "error",
    }
}

fn value_keys(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Object(object)) = value else {
        return Vec::new();
    };
    object.keys().cloned().collect()
}

fn output_files(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Object(object)) = value else {
        return Vec::new();
    };
    let mut files = Vec::new();
    if let Some(path) = object.get("path").and_then(Value::as_str) {
        files.push(path.to_string());
    }
    if let Some(values) = object.get("files").and_then(Value::as_array) {
        for value in values {
            match value {
                Value::String(path) => files.push(path.clone()),
                Value::Object(file) => {
                    if let Some(path) = file.get("path").and_then(Value::as_str) {
                        files.push(path.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(values) = object.get("entries").and_then(Value::as_array) {
        for value in values {
            if let Some(path) = value.get("path").and_then(Value::as_str) {
                files.push(path.to_string());
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn artifact_field_values(value: Option<&Value>, field: &str) -> Vec<String> {
    let Some(Value::Object(object)) = value else {
        return Vec::new();
    };
    let Some(artifacts) = object.get("artifacts").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut values = artifacts
        .iter()
        .filter_map(|artifact| artifact.get(field))
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn print_explain(
    requested_recipe: CodeRecipe,
    resolved_recipe: CodeRecipe,
    profile: &Path,
    input: &Map<String, Value>,
    loop_enabled: bool,
    execute_plan: bool,
    max_iterations: usize,
) -> Result<()> {
    let metadata = explain_metadata_for_profile(profile)?;
    let budget_iterations = if loop_enabled { max_iterations } else { 1 };
    let explanation = json!({
        "command": "code",
        "will_run": false,
        "requested_recipe": recipe_name(requested_recipe),
        "resolved_recipe": recipe_name(resolved_recipe),
        "profile": path_ref_to_input_string(profile),
        "plan": path_ref_to_input_string(&metadata.plan),
        "store": path_ref_to_input_string(&metadata.store),
        "capabilities": metadata.capabilities,
        "read_only": metadata.read_only,
        "writes_workspace": metadata.writes_workspace,
        "budget": {
            "per_iteration": {
                "max_estimated_model_calls": metadata.max_estimated_model_calls,
                "max_estimated_tool_calls": metadata.max_estimated_tool_calls
            },
            "total": {
                "iterations": budget_iterations,
                "max_estimated_model_calls": metadata.max_estimated_model_calls.saturating_mul(budget_iterations),
                "max_estimated_tool_calls": metadata.max_estimated_tool_calls.saturating_mul(budget_iterations)
            }
        },
        "loop": {
            "enabled": loop_enabled,
            "max_iterations": max_iterations
        },
        "project_execution": {
            "enabled": execute_plan,
            "max_tasks": if execute_plan { max_iterations } else { 0 }
        },
        "input": Value::Object(input.clone()),
    });
    serde_json::to_writer_pretty(std::io::stdout(), &explanation)?;
    println!();
    Ok(())
}

struct CodeExplainMetadata {
    plan: PathBuf,
    store: PathBuf,
    capabilities: Vec<String>,
    read_only: bool,
    writes_workspace: bool,
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
}

fn explain_metadata_for_profile(profile: &Path) -> Result<CodeExplainMetadata> {
    let profile_path = profile.to_path_buf();
    let profile = read_run_plan_profile(&profile_path)?;
    let plan_path = resolve_profile_path(&profile_path, &profile.plan);
    let store_path = resolve_profile_path(&profile_path, &profile.store);
    let plan = air_linker::parse_run_plan_file(&plan_path)?;
    let store = air_linker::parse_module_store_file(&store_path)?;
    let base_dir = module_base_dir_for_store_path(&store, &store_path);
    let plan_explanation = build_plan_explanation(&plan, &store, &base_dir)?;
    let mut capabilities = plan.requires.capabilities;
    capabilities.sort();
    capabilities.dedup();
    let writes_workspace = capabilities
        .iter()
        .any(|capability| is_workspace_write_capability(capability));
    Ok(CodeExplainMetadata {
        plan: plan_path,
        store: store_path,
        capabilities,
        read_only: !writes_workspace,
        writes_workspace,
        max_estimated_model_calls: plan_explanation.max_estimated_model_calls(),
        max_estimated_tool_calls: plan_explanation.max_estimated_tool_calls(),
    })
}

fn is_workspace_write_capability(capability: &str) -> bool {
    matches!(capability, "file.write")
}

struct CodeLoopOptions {
    recipe: CodeRecipe,
    profile: PathBuf,
    input: Map<String, Value>,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    trace_raw: bool,
    state_out: Option<PathBuf>,
    checkpoint_out: Option<PathBuf>,
    jit_cache: Option<PathBuf>,
    parallel: bool,
    log: bool,
    tool_config: Option<PathBuf>,
    max_iterations: usize,
}

struct CodeProjectOptions {
    plan_profile: PathBuf,
    input: Map<String, Value>,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    trace_raw: bool,
    state_out: Option<PathBuf>,
    checkpoint_out: Option<PathBuf>,
    jit_cache: Option<PathBuf>,
    parallel: bool,
    log: bool,
    tool_config: Option<PathBuf>,
    max_tasks: usize,
}

fn run_code_project(options: CodeProjectOptions) -> Result<Value> {
    if options.max_tasks == 0 {
        bail!("air code --execute-plan requires --max-iterations to be greater than 0");
    }

    let mut trace_files = Vec::new();
    let plan_trace = options
        .trace_out
        .as_ref()
        .map(|path| labeled_trace_path(path, "plan"));
    if let Some(path) = plan_trace.as_ref() {
        trace_files.push(path_ref_to_input_string(path));
    }

    if options.log {
        eprintln!("[air-code-project] step=plan recipe=plan");
    }
    let plan_outputs = run_plan_capture(RunPlanOptions {
        plan: None,
        profile: Some(options.plan_profile.clone()),
        store: None,
        input: None,
        input_values: Some(options.input.clone()),
        model_config: options.model_config.clone(),
        trace_out: plan_trace,
        trace_redact: options.trace_redact,
        trace_raw: options.trace_raw,
        state_out: options
            .state_out
            .as_ref()
            .map(|path| labeled_trace_path(path, "plan")),
        checkpoint_out: options
            .checkpoint_out
            .as_ref()
            .map(|path| labeled_trace_path(path, "plan")),
        jit_cache: options.jit_cache.clone(),
        parallel: options.parallel,
        log: options.log,
        example_tools: false,
        tool_config: options.tool_config.clone(),
    })?;
    let project_plan = plan_outputs
        .get("project_plan")
        .cloned()
        .context("project plan run did not return project_plan")?;
    let tasks = project_plan
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut executions = Vec::new();
    let mut prior_executions = Vec::new();
    let mut completed = true;
    let mut stopped = false;
    for (index, task) in tasks.iter().take(options.max_tasks).enumerate() {
        let task_number = index + 1;
        let task_id = task
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("task")
            .to_string();
        let title = task
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let recipe = match task_recipe(task) {
            Ok(recipe) => recipe,
            Err(error) => {
                completed = false;
                stopped = true;
                executions.push(json!({
                    "task_id": task_id,
                    "title": title,
                    "completed": false,
                    "error": error.to_string(),
                }));
                break;
            }
        };
        let Some(input) = task.get("input").and_then(Value::as_object) else {
            completed = false;
            stopped = true;
            executions.push(json!({
                "task_id": task_id,
                "title": title,
                "recipe": recipe_name(recipe),
                "completed": false,
                "error": "planned task is missing input object",
            }));
            break;
        };
        let task_input = code_project_task_input(input, &prior_executions);
        let trace_path = options
            .trace_out
            .as_ref()
            .map(|path| labeled_trace_path(path, &format!("task{task_number}")));
        if let Some(path) = trace_path.as_ref() {
            trace_files.push(path_ref_to_input_string(path));
        }

        if options.log {
            eprintln!(
                "[air-code-project] step=task index={} id={} recipe={}",
                task_number,
                task_id,
                recipe_name(recipe)
            );
        }
        let task_result = run_plan_capture(RunPlanOptions {
            plan: None,
            profile: Some(default_profile(recipe)),
            store: None,
            input: None,
            input_values: Some(task_input),
            model_config: options.model_config.clone(),
            trace_out: trace_path,
            trace_redact: options.trace_redact,
            trace_raw: options.trace_raw,
            state_out: options
                .state_out
                .as_ref()
                .map(|path| labeled_trace_path(path, &format!("task{task_number}"))),
            checkpoint_out: options
                .checkpoint_out
                .as_ref()
                .map(|path| labeled_trace_path(path, &format!("task{task_number}"))),
            jit_cache: options.jit_cache.clone(),
            parallel: options.parallel,
            log: options.log,
            example_tools: false,
            tool_config: options.tool_config.clone(),
        });
        match task_result {
            Ok(outputs) => {
                let task_completed = code_outputs_complete(recipe, &outputs);
                completed &= task_completed;
                let execution = json!({
                    "task_id": task_id,
                    "title": title,
                    "recipe": recipe_name(recipe),
                    "completed": task_completed,
                    "outputs": outputs,
                });
                prior_executions.push(execution.clone());
                executions.push(execution);
                if !task_completed {
                    stopped = true;
                    break;
                }
            }
            Err(error) => {
                completed = false;
                stopped = true;
                let execution = json!({
                    "task_id": task_id,
                    "title": title,
                    "recipe": recipe_name(recipe),
                    "completed": false,
                    "error": error.to_string(),
                });
                prior_executions.push(execution.clone());
                executions.push(execution);
                break;
            }
        }
    }

    let executed = executions.len();
    let remaining_task_ids = tasks
        .iter()
        .skip(executed)
        .filter_map(|task| task.get("id").and_then(Value::as_str))
        .map(|id| Value::String(id.to_string()))
        .collect::<Vec<_>>();
    let status = if stopped {
        "stopped"
    } else if tasks.len() > executed {
        completed = false;
        "max_tasks_exhausted"
    } else if completed {
        "completed"
    } else {
        "stopped"
    };

    Ok(json!({
        "project_plan": project_plan,
        "project": {
            "status": status,
            "completed": completed,
            "max_tasks": options.max_tasks,
            "executed_tasks": executed,
            "remaining_task_ids": remaining_task_ids,
            "executions": executions,
        },
        "trace_files": trace_files,
    }))
}

fn task_recipe(task: &Value) -> Result<CodeRecipe> {
    let name = task
        .get("recipe")
        .and_then(Value::as_str)
        .context("planned task is missing recipe")?;
    match name {
        "explore" => Ok(CodeRecipe::Explore),
        "review" => Ok(CodeRecipe::Review),
        "repair" => Ok(CodeRecipe::Repair),
        "build" => Ok(CodeRecipe::Build),
        other => bail!("unsupported planned task recipe {other:?}"),
    }
}

fn code_project_task_input(
    base_input: &Map<String, Value>,
    prior_executions: &[Value],
) -> Map<String, Value> {
    if prior_executions.is_empty() {
        return base_input.clone();
    }
    let mut input = base_input.clone();
    let task = input
        .get("task")
        .and_then(Value::as_str)
        .unwrap_or("execute planned project task");
    let feedback = code_loop_feedback(prior_executions);
    input.insert(
        "task".to_string(),
        Value::String(format!(
            "{task}\n\nAIR project execution context from previous tasks:\n{feedback}"
        )),
    );
    input
}

fn run_code_loop(options: CodeLoopOptions) -> Result<Value> {
    if options.max_iterations == 0 {
        bail!("air code --loop requires --max-iterations to be greater than 0");
    }

    let mut iterations = Vec::new();
    let mut final_outputs = Value::Object(Map::new());
    let mut completed = false;

    for iteration in 1..=options.max_iterations {
        if options.log {
            eprintln!(
                "[air-code-loop] iteration={} recipe={}",
                iteration,
                recipe_name(options.recipe)
            );
        }
        let iteration_input = code_loop_iteration_input(&options.input, &iterations);
        let outputs = run_plan_capture(RunPlanOptions {
            plan: None,
            profile: Some(options.profile.clone()),
            store: None,
            input: None,
            input_values: Some(iteration_input),
            model_config: options.model_config.clone(),
            trace_out: options
                .trace_out
                .as_ref()
                .map(|path| iteration_path(path, iteration)),
            trace_redact: options.trace_redact,
            trace_raw: options.trace_raw,
            state_out: options
                .state_out
                .as_ref()
                .map(|path| iteration_path(path, iteration)),
            checkpoint_out: options
                .checkpoint_out
                .as_ref()
                .map(|path| iteration_path(path, iteration)),
            jit_cache: options.jit_cache.clone(),
            parallel: options.parallel,
            log: options.log,
            example_tools: false,
            tool_config: options.tool_config.clone(),
        })?;
        completed = code_outputs_complete(options.recipe, &outputs);
        final_outputs = outputs.clone();
        iterations.push(json!({
            "iteration": iteration,
            "completed": completed,
            "outputs": outputs,
        }));
        if completed {
            break;
        }
    }

    let status = if completed {
        "completed"
    } else {
        "max_iterations_exhausted"
    };
    let summary = json!({
        "status": status,
        "recipe": recipe_name(options.recipe),
        "completed": completed,
        "iterations": iterations,
        "final_outputs": final_outputs,
    });
    Ok(summary)
}

fn code_session_turn_input(
    base_input: &Map<String, Value>,
    previous_turns: &[CodeSessionTurn],
) -> Map<String, Value> {
    if previous_turns.is_empty() {
        return base_input.clone();
    }

    let mut input = base_input.clone();
    let Some(task) = input.get("task").and_then(Value::as_str) else {
        return input;
    };
    let feedback = code_session_feedback(previous_turns);
    input.insert(
        "task".to_string(),
        Value::String(format!(
            "{task}\n\nAIR session context from previous turns:\n{feedback}"
        )),
    );
    input
}

fn code_session_feedback(previous_turns: &[CodeSessionTurn]) -> String {
    let budget = default_context_budget_chars();
    let mut lines = Vec::new();
    let mut used = 0usize;
    let mut omitted = 0usize;
    for (index, turn) in previous_turns.iter().enumerate().rev() {
        let turn_number = index + 1;
        let summary = code_session_feedback_summary(turn);
        let prefix = format!(
            "- turn {turn_number}: {summary}; outputs={output_summary}",
            output_summary = ""
        );
        let available = budget.saturating_sub(used + prefix.chars().count());
        if available == 0 {
            omitted += 1;
            continue;
        }
        let output_summary = truncate_for_context(
            &serde_json::to_string(&turn.outputs).unwrap_or_default(),
            available,
        );
        let line = format!("- turn {turn_number}: {summary}; outputs={output_summary}");
        used += line.chars().count() + 1;
        lines.push(line);
        if used >= budget {
            omitted += index;
            break;
        }
    }
    if omitted > 0 {
        lines.push(
            format!(
                "- {omitted} older turn(s) omitted because AIR session context is capped at {budget} chars"
            ),
        );
    }
    truncate_for_context(&lines.join("\n"), budget)
}

fn code_session_feedback_summary(turn: &CodeSessionTurn) -> String {
    let mut fields = vec![
        format!("recipe={}", turn.recipe),
        format!("completed={}", turn.completed),
    ];
    if !turn.summary.models.is_empty() {
        fields.push(format!("models={}", turn.summary.models.join(",")));
    }
    if !turn.summary.tools.is_empty() {
        fields.push(format!("tools={}", turn.summary.tools.join(",")));
    }
    if !turn.summary.approvals.is_empty() {
        fields.push(format!("approvals={}", turn.summary.approvals.join(",")));
    }
    if !turn.summary.files.is_empty() {
        fields.push(format!("files={}", turn.summary.files.join(",")));
    }
    if !turn.summary.artifact_kinds.is_empty() {
        fields.push(format!(
            "artifact_kinds={}",
            turn.summary.artifact_kinds.join(",")
        ));
    }
    if turn.summary.model_call_count > 0
        || turn.summary.tool_call_count > 0
        || turn.summary.approval_count > 0
        || turn.summary.error_count > 0
    {
        fields.push(format!(
            "counts=model:{},tool:{},approval:{},error:{}",
            turn.summary.model_call_count,
            turn.summary.tool_call_count,
            turn.summary.approval_count,
            turn.summary.error_count
        ));
    }
    fields.join("; ")
}

fn code_loop_iteration_input(
    base_input: &Map<String, Value>,
    previous_iterations: &[Value],
) -> Map<String, Value> {
    if previous_iterations.is_empty() {
        return base_input.clone();
    }

    let mut input = base_input.clone();
    let Some(task) = input.get("task").and_then(Value::as_str) else {
        return input;
    };
    let feedback = code_loop_feedback(previous_iterations);
    input.insert(
        "task".to_string(),
        Value::String(format!(
            "{task}\n\nAIR loop context from previous iterations:\n{feedback}"
        )),
    );
    input
}

fn code_loop_feedback(previous_iterations: &[Value]) -> String {
    let budget = default_context_budget_chars();
    let mut lines = Vec::new();
    let mut used = 0usize;
    let mut omitted = 0usize;
    for iteration in previous_iterations.iter().rev() {
        let iteration_number = iteration
            .get("iteration")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let completed = iteration
            .get("completed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let outputs = iteration.get("outputs").cloned().unwrap_or(Value::Null);
        let prefix = format!(
            "- iteration {iteration_number}: completed={completed}; outputs={output_summary}",
            output_summary = ""
        );
        let available = budget.saturating_sub(used + prefix.chars().count());
        if available == 0 {
            omitted += 1;
            continue;
        }
        let output_summary = truncate_for_context(
            &serde_json::to_string(&outputs).unwrap_or_default(),
            available,
        );
        let line = format!(
            "- iteration {iteration_number}: completed={completed}; outputs={output_summary}"
        );
        used += line.chars().count() + 1;
        lines.push(line);
        if used >= budget {
            omitted += iteration_number.saturating_sub(1) as usize;
            break;
        }
    }
    if omitted > 0 {
        lines.push(
            format!(
                "- {omitted} older iteration(s) omitted because AIR loop context is capped at {budget} chars"
            ),
        );
    }
    truncate_for_context(&lines.join("\n"), budget)
}

fn default_context_budget_chars() -> usize {
    DEFAULT_CONTEXT_MAX_CHARS.saturating_mul(DEFAULT_CONTEXT_THRESHOLD_PERCENT) / 100
}

fn truncate_for_context(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars == 0 {
        return "[AIR_TRUNCATED]".to_string();
    }
    const MARKER: &str = " [AIR_TRUNCATED]";
    if max_chars <= MARKER.chars().count() {
        return MARKER.chars().take(max_chars).collect();
    }
    let keep = max_chars - MARKER.chars().count();
    let mut truncated = value.chars().take(keep).collect::<String>();
    truncated.push_str(MARKER);
    truncated
}

fn code_outputs_complete(recipe: CodeRecipe, outputs: &Value) -> bool {
    match recipe {
        CodeRecipe::Auto => false,
        CodeRecipe::Plan => outputs
            .pointer("/project/completed")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                outputs
                    .pointer("/project_plan/tasks")
                    .and_then(Value::as_array)
                    .is_some_and(|tasks| !tasks.is_empty())
            }),
        CodeRecipe::Explore => outputs.get("exploration").is_some(),
        CodeRecipe::Review => outputs
            .pointer("/review/search_quality/sufficient")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| outputs.get("review").is_some()),
        CodeRecipe::Repair => outputs
            .pointer("/repair/final_success")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        CodeRecipe::Build => {
            outputs
                .pointer("/build/test_success")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && outputs
                    .pointer("/build/audit_success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        }
    }
}

fn iteration_path(path: &Path, iteration: usize) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("air-code-loop");
    let extension = path.extension().and_then(|value| value.to_str());
    let file_name = if let Some(extension) = extension {
        format!("{stem}.iter{iteration}.{extension}")
    } else {
        format!("{stem}.iter{iteration}")
    };
    parent.join(file_name)
}

fn labeled_trace_path(path: &Path, label: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("air-code");
    let extension = path.extension().and_then(|value| value.to_str());
    let file_name = if let Some(extension) = extension {
        format!("{stem}.{label}.{extension}")
    } else {
        format!("{stem}.{label}")
    };
    parent.join(file_name)
}

struct CodeInputOptions {
    task: String,
    recipe: CodeRecipe,
    target: Option<PathBuf>,
    test: Option<String>,
    query: Option<String>,
    related: Vec<PathBuf>,
    search_query: Option<String>,
    repo_query: Option<String>,
    required_terms: Vec<String>,
    output: Option<PathBuf>,
    brand: Option<String>,
    product: Option<String>,
    constraints: Vec<String>,
}

fn build_input(options: CodeInputOptions) -> Result<Map<String, Value>> {
    let CodeInputOptions {
        task,
        recipe,
        target,
        test,
        query,
        related,
        search_query,
        repo_query,
        required_terms,
        output,
        brand,
        product,
        constraints,
    } = options;

    let recipe = resolve_recipe(
        recipe,
        target.as_ref(),
        test.as_ref(),
        search_query.as_ref(),
        repo_query.as_ref(),
        &required_terms,
        output.as_ref(),
        brand.as_ref(),
        product.as_ref(),
        &constraints,
    );

    match recipe {
        CodeRecipe::Auto => unreachable!("auto recipe is resolved before building input"),
        CodeRecipe::Plan => {
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query.unwrap_or(task)));
            Ok(input)
        }
        CodeRecipe::Explore => {
            let target = required_path(target, "--target", recipe)?;
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query.unwrap_or(task)));
            input.insert(
                "target_path".to_string(),
                Value::String(path_to_input_string(target)),
            );
            Ok(input)
        }
        CodeRecipe::Review => {
            let target = required_path(target, "--target", recipe)?;
            let target = path_to_input_string(target);
            let repo_query = repo_query.or(query).unwrap_or_else(|| target.clone());
            let related = if related.is_empty() {
                vec![PathBuf::from(&target)]
            } else {
                related
            };
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert(
                "search_query".to_string(),
                Value::String(search_query.unwrap_or(task)),
            );
            input.insert("repo_query".to_string(), Value::String(repo_query));
            input.insert("required_terms".to_string(), string_array(required_terms));
            input.insert("target_file".to_string(), Value::String(target));
            input.insert("related_files".to_string(), path_array(related));
            Ok(input)
        }
        CodeRecipe::Repair => {
            let target = required_path(target, "--target", recipe)?;
            let test = required_string(test, "--test", recipe)?;
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query.unwrap_or(task)));
            input.insert(
                "target_path".to_string(),
                Value::String(path_to_input_string(target)),
            );
            input.insert("related_files".to_string(), path_array(related));
            input.insert("test_command".to_string(), Value::String(test));
            Ok(input)
        }
        CodeRecipe::Build => {
            let output = required_path(output, "--output", recipe)?;
            let brand = brand.unwrap_or_else(|| "Product".to_string());
            let product = product.unwrap_or_else(|| brand.clone());
            let constraints = if constraints.is_empty() {
                vec![
                    "single self-contained HTML file".to_string(),
                    "no external network assets".to_string(),
                    "responsive down to mobile width".to_string(),
                    "buttons and text must not overlap".to_string(),
                ]
            } else {
                constraints
            };
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task));
            input.insert(
                "output_path".to_string(),
                Value::String(path_to_input_string(output)),
            );
            input.insert("brand".to_string(), Value::String(brand));
            input.insert("product".to_string(), Value::String(product));
            input.insert("constraints".to_string(), string_array(constraints));
            Ok(input)
        }
    }
}

fn required_path(value: Option<PathBuf>, flag: &str, recipe: CodeRecipe) -> Result<PathBuf> {
    let Some(value) = value else {
        bail!("air code --recipe {} requires {flag}", recipe_name(recipe));
    };
    Ok(value)
}

fn required_string(value: Option<String>, flag: &str, recipe: CodeRecipe) -> Result<String> {
    let Some(value) = value else {
        bail!("air code --recipe {} requires {flag}", recipe_name(recipe));
    };
    Ok(value)
}

fn default_profile(recipe: CodeRecipe) -> PathBuf {
    match recipe {
        CodeRecipe::Auto => PathBuf::from("examples/code-agent/explore.air-profile.yaml"),
        CodeRecipe::Plan => PathBuf::from("examples/code-agent/project-plan.air-profile.yaml"),
        CodeRecipe::Explore => PathBuf::from("examples/code-agent/explore.air-profile.yaml"),
        CodeRecipe::Review => PathBuf::from("examples/code-agent/profile.air-profile.yaml"),
        CodeRecipe::Repair => PathBuf::from("examples/code-agent/repair-core.air-profile.yaml"),
        CodeRecipe::Build => PathBuf::from("examples/code-agent/apple-build.air-profile.yaml"),
    }
}

fn recipe_name(recipe: CodeRecipe) -> &'static str {
    match recipe {
        CodeRecipe::Auto => "auto",
        CodeRecipe::Plan => "plan",
        CodeRecipe::Explore => "explore",
        CodeRecipe::Review => "review",
        CodeRecipe::Repair => "repair",
        CodeRecipe::Build => "build",
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_recipe(
    recipe: CodeRecipe,
    target: Option<&PathBuf>,
    test: Option<&String>,
    search_query: Option<&String>,
    repo_query: Option<&String>,
    required_terms: &[String],
    output: Option<&PathBuf>,
    brand: Option<&String>,
    product: Option<&String>,
    constraints: &[String],
) -> CodeRecipe {
    if recipe != CodeRecipe::Auto {
        return recipe;
    }
    if output.is_some() || brand.is_some() || product.is_some() || !constraints.is_empty() {
        return CodeRecipe::Build;
    }
    if test.is_some() {
        return CodeRecipe::Repair;
    }
    if search_query.is_some() || repo_query.is_some() || !required_terms.is_empty() {
        return CodeRecipe::Review;
    }
    if target.is_none() {
        return CodeRecipe::Plan;
    }
    CodeRecipe::Explore
}

fn path_array(paths: Vec<PathBuf>) -> Value {
    Value::Array(
        paths
            .into_iter()
            .map(path_to_input_string)
            .map(Value::String)
            .collect(),
    )
}

fn string_array(values: Vec<String>) -> Value {
    Value::Array(values.into_iter().map(Value::String).collect())
}

fn path_to_input_string(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn path_ref_to_input_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_repair_input() {
        let input = build_input(CodeInputOptions {
            task: "fix it".to_string(),
            recipe: CodeRecipe::Repair,
            target: Some(PathBuf::from("src/lib.rs")),
            test: Some("unit".to_string()),
            query: None,
            related: vec![PathBuf::from("src/test.rs")],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(input["task"], Value::String("fix it".to_string()));
        assert_eq!(input["query"], Value::String("fix it".to_string()));
        assert_eq!(
            input["target_path"],
            Value::String("src/lib.rs".to_string())
        );
        assert_eq!(input["test_command"], Value::String("unit".to_string()));
        assert_eq!(
            input["related_files"],
            Value::Array(vec![Value::String("src/test.rs".to_string())])
        );
    }

    #[test]
    fn auto_recipe_selects_repair_when_test_is_present() {
        let input = build_input(CodeInputOptions {
            task: "fix it".to_string(),
            recipe: CodeRecipe::Auto,
            target: Some(PathBuf::from("src/lib.rs")),
            test: Some("unit".to_string()),
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(input["test_command"], Value::String("unit".to_string()));
        assert_eq!(
            input["target_path"],
            Value::String("src/lib.rs".to_string())
        );
    }

    #[test]
    fn auto_recipe_selects_build_when_output_is_present() {
        let input = build_input(CodeInputOptions {
            task: "build it".to_string(),
            recipe: CodeRecipe::Auto,
            target: None,
            test: None,
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: Some(PathBuf::from("site/index.html")),
            brand: Some("Acme".to_string()),
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["output_path"],
            Value::String("site/index.html".to_string())
        );
        assert_eq!(input["brand"], Value::String("Acme".to_string()));
        assert_eq!(input["product"], Value::String("Acme".to_string()));
    }

    #[test]
    fn auto_recipe_selects_review_when_review_flags_are_present() {
        let input = build_input(CodeInputOptions {
            task: "review it".to_string(),
            recipe: CodeRecipe::Auto,
            target: Some(PathBuf::from("src/lib.rs")),
            test: None,
            query: None,
            related: vec![],
            search_query: Some("library docs".to_string()),
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["search_query"],
            Value::String("library docs".to_string())
        );
        assert_eq!(
            input["target_file"],
            Value::String("src/lib.rs".to_string())
        );
    }

    #[test]
    fn auto_recipe_selects_plan_without_target() {
        let input = build_input(CodeInputOptions {
            task: "plan the next code-agent milestone".to_string(),
            recipe: CodeRecipe::Auto,
            target: None,
            test: None,
            query: Some("code agent project planning".to_string()),
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["task"],
            Value::String("plan the next code-agent milestone".to_string())
        );
        assert_eq!(
            input["query"],
            Value::String("code agent project planning".to_string())
        );
        assert!(input.get("target_path").is_none());
    }

    #[test]
    fn auto_recipe_defaults_to_read_only_explore() {
        let input = build_input(CodeInputOptions {
            task: "understand this".to_string(),
            recipe: CodeRecipe::Auto,
            target: Some(PathBuf::from("src/lib.rs")),
            test: None,
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(input["query"], Value::String("understand this".to_string()));
        assert_eq!(
            input["target_path"],
            Value::String("src/lib.rs".to_string())
        );
        assert!(input.get("test_command").is_none());
    }

    #[test]
    fn explain_payload_reports_requested_and_resolved_recipe() {
        let input = build_input(CodeInputOptions {
            task: "fix it".to_string(),
            recipe: CodeRecipe::Auto,
            target: Some(PathBuf::from("src/lib.rs")),
            test: Some("unit".to_string()),
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();
        let explanation = json!({
            "command": "code",
            "will_run": false,
            "requested_recipe": recipe_name(CodeRecipe::Auto),
            "resolved_recipe": recipe_name(CodeRecipe::Repair),
            "profile": path_ref_to_input_string(&default_profile(CodeRecipe::Repair)),
            "input": Value::Object(input),
        });

        assert_eq!(
            explanation["requested_recipe"],
            Value::String("auto".to_string())
        );
        assert_eq!(
            explanation["resolved_recipe"],
            Value::String("repair".to_string())
        );
        assert_eq!(
            explanation["profile"],
            Value::String("examples/code-agent/repair-core.air-profile.yaml".to_string())
        );
        assert_eq!(
            explanation["input"]["test_command"],
            Value::String("unit".to_string())
        );
    }

    #[test]
    fn explain_metadata_reports_profile_capabilities() {
        let profile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/repair-core.air-profile.yaml");
        let metadata = explain_metadata_for_profile(&profile).unwrap();

        assert!(metadata
            .capabilities
            .iter()
            .any(|capability| capability == "file.write"));
        assert!(metadata.writes_workspace);
        assert!(!metadata.read_only);
        assert!(metadata.max_estimated_model_calls > 0);
        assert!(metadata.max_estimated_tool_calls > 0);
        assert!(metadata
            .plan
            .ends_with("code-repair-with-explore.air-plan.yaml"));
    }

    #[test]
    fn completion_detection_matches_recipe_outputs() {
        assert!(code_outputs_complete(
            CodeRecipe::Repair,
            &json!({"repair": {"final_success": true}})
        ));
        assert!(!code_outputs_complete(
            CodeRecipe::Repair,
            &json!({"repair": {"final_success": false}})
        ));
        assert!(code_outputs_complete(
            CodeRecipe::Build,
            &json!({"build": {"test_success": true, "audit_success": true}})
        ));
        assert!(!code_outputs_complete(
            CodeRecipe::Build,
            &json!({"build": {"test_success": true, "audit_success": false}})
        ));
        assert!(code_outputs_complete(
            CodeRecipe::Plan,
            &json!({"project_plan": {"tasks": [{"id": "t1"}]}})
        ));
        assert!(!code_outputs_complete(
            CodeRecipe::Plan,
            &json!({"project_plan": {"tasks": []}})
        ));
    }

    #[test]
    fn iteration_path_preserves_extension() {
        assert_eq!(
            iteration_path(Path::new("target/generated/code.trace.jsonl"), 2),
            PathBuf::from("target/generated/code.trace.iter2.jsonl")
        );
    }

    #[test]
    fn default_session_trace_path_uses_sibling_trace_directory() {
        assert_eq!(
            default_session_trace_path(Path::new("target/generated/code_session.json"), 3),
            PathBuf::from("target/generated/code_session.traces/turn3.trace.jsonl")
        );
    }

    #[test]
    fn code_session_trace_files_prefers_explicit_project_trace_index() {
        let files = code_session_trace_files(
            Some(&PathBuf::from("target/generated/code.trace.jsonl")),
            false,
            &json!({
                "trace_files": [
                    "target/generated/code.trace.plan.jsonl",
                    "target/generated/code.trace.task1.jsonl"
                ]
            }),
        );

        assert_eq!(
            files,
            vec![
                PathBuf::from("target/generated/code.trace.plan.jsonl"),
                PathBuf::from("target/generated/code.trace.task1.jsonl"),
            ]
        );
    }

    #[test]
    fn session_part_indexes_trace_event() {
        let event = TraceEvent {
            agent: "code-agent".to_string(),
            step: 2,
            rule: "analyze".to_string(),
            action: "model_call".to_string(),
            input: Some(json!({"task": "review", "context": {}})),
            output: Some(json!({
                "summary": "ok",
                "files": [{"path": "src/lib.rs"}],
                "artifacts": [{"id": "artifact:1", "kind": "file_patch"}]
            })),
            meta: Some(json!({"model": "code_reviewer"})),
            status: TraceStatus::Ok,
            error: None,
        };

        let part = code_session_part_from_event("trace.jsonl", &event).unwrap();

        assert_eq!(part.kind, "model_call");
        assert_eq!(part.trace_file, "trace.jsonl");
        assert_eq!(part.model, Some("code_reviewer".to_string()));
        assert!(part.input_keys.iter().any(|key| key == "task"));
        assert!(part.output_keys.iter().any(|key| key == "summary"));
        assert_eq!(part.files, vec!["src/lib.rs"]);
        assert_eq!(part.artifact_ids, vec!["artifact:1"]);
        assert_eq!(part.artifact_kinds, vec!["file_patch"]);
        assert_eq!(part.status, "ok");
    }

    #[test]
    fn session_turn_summary_rolls_up_parts() {
        let parts = vec![
            CodeSessionPart {
                kind: "model_call".to_string(),
                trace_file: "trace.jsonl".to_string(),
                agent: "agent".to_string(),
                step: 1,
                rule: "analyze".to_string(),
                action: "model_call".to_string(),
                status: "ok".to_string(),
                model: Some("code_reviewer".to_string()),
                tool: None,
                approval_for: Vec::new(),
                files: Vec::new(),
                artifact_ids: Vec::new(),
                artifact_kinds: Vec::new(),
                input_keys: Vec::new(),
                output_keys: Vec::new(),
                error: None,
            },
            CodeSessionPart {
                kind: "tool_call".to_string(),
                trace_file: "trace.jsonl".to_string(),
                agent: "agent".to_string(),
                step: 2,
                rule: "patch".to_string(),
                action: "tool_call".to_string(),
                status: "ok".to_string(),
                model: None,
                tool: Some("file.patch".to_string()),
                approval_for: Vec::new(),
                files: vec!["src/lib.rs".to_string()],
                artifact_ids: vec!["artifact:1".to_string()],
                artifact_kinds: vec!["file_patch".to_string()],
                input_keys: Vec::new(),
                output_keys: Vec::new(),
                error: None,
            },
            CodeSessionPart {
                kind: "approval".to_string(),
                trace_file: "trace.jsonl".to_string(),
                agent: "agent".to_string(),
                step: 3,
                rule: "approve".to_string(),
                action: "approval".to_string(),
                status: "ok".to_string(),
                model: None,
                tool: None,
                approval_for: vec!["file.write".to_string()],
                files: Vec::new(),
                artifact_ids: Vec::new(),
                artifact_kinds: Vec::new(),
                input_keys: Vec::new(),
                output_keys: Vec::new(),
                error: None,
            },
        ];

        let summary = code_session_turn_summary(&parts);

        assert_eq!(summary.model_call_count, 1);
        assert_eq!(summary.tool_call_count, 1);
        assert_eq!(summary.approval_count, 1);
        assert_eq!(summary.models, vec!["code_reviewer"]);
        assert_eq!(summary.tools, vec!["file.patch"]);
        assert_eq!(summary.approvals, vec!["file.write"]);
        assert_eq!(summary.files, vec!["src/lib.rs"]);
        assert_eq!(summary.artifact_kinds, vec!["file_patch"]);
    }

    #[test]
    fn loop_iteration_input_appends_previous_outputs_to_task() {
        let mut input = Map::new();
        input.insert("task".to_string(), Value::String("fix it".to_string()));
        input.insert(
            "target_path".to_string(),
            Value::String("src/lib.rs".to_string()),
        );
        let iterations = vec![json!({
            "iteration": 1,
            "completed": false,
            "outputs": {
                "repair": {
                    "final_success": false,
                    "diagnostics": [{"path": "src/lib.rs", "line": 3}]
                }
            }
        })];

        let next = code_loop_iteration_input(&input, &iterations);

        assert_eq!(next["target_path"], Value::String("src/lib.rs".to_string()));
        let task = next["task"].as_str().unwrap();
        assert!(task.starts_with("fix it"));
        assert!(task.contains("AIR loop context from previous iterations"));
        assert!(task.contains("final_success"));
    }

    #[test]
    fn project_task_input_appends_previous_task_outputs() {
        let mut input = Map::new();
        input.insert(
            "task".to_string(),
            Value::String("inspect the next file".to_string()),
        );
        input.insert(
            "target_path".to_string(),
            Value::String("src/lib.rs".to_string()),
        );
        let previous = vec![json!({
            "task_id": "t1",
            "completed": true,
            "outputs": {"exploration": {"summary": "found routing code"}}
        })];

        let next = code_project_task_input(&input, &previous);

        assert_eq!(next["target_path"], Value::String("src/lib.rs".to_string()));
        let task = next["task"].as_str().unwrap();
        assert!(task.contains("inspect the next file"));
        assert!(task.contains("AIR project execution context from previous tasks"));
        assert!(task.contains("found routing code"));
    }

    #[test]
    fn session_turn_input_appends_previous_outputs_to_task() {
        let mut input = Map::new();
        input.insert(
            "task".to_string(),
            Value::String("continue investigation".to_string()),
        );
        input.insert(
            "target_path".to_string(),
            Value::String("src/lib.rs".to_string()),
        );
        let turns = vec![CodeSessionTurn {
            id: "turn-000001".to_string(),
            time: CodeSessionTurnTime {
                created: 1,
                updated: 1,
            },
            task: "inspect repo".to_string(),
            recipe: "explore".to_string(),
            profile: "examples/code-agent/explore.air-profile.yaml".to_string(),
            input: json!({"task": "inspect repo"}),
            completed: true,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary {
                models: vec!["code_explorer".to_string()],
                tools: vec!["repo.search".to_string()],
                files: vec!["src/lib.rs".to_string()],
                model_call_count: 1,
                tool_call_count: 1,
                ..CodeSessionTurnSummary::default()
            },
            parts: Vec::new(),
            outputs: json!({
                "exploration": {
                    "summary": "Found the dispatch implementation",
                    "source_ids": ["repo:lib"]
                }
            }),
        }];

        let next = code_session_turn_input(&input, &turns);

        assert_eq!(next["target_path"], Value::String("src/lib.rs".to_string()));
        let task = next["task"].as_str().unwrap();
        assert!(task.starts_with("continue investigation"));
        assert!(task.contains("AIR session context from previous turns"));
        assert!(task.contains("models=code_explorer"));
        assert!(task.contains("tools=repo.search"));
        assert!(task.contains("files=src/lib.rs"));
        assert!(task.contains("Found the dispatch implementation"));
    }

    #[test]
    fn session_feedback_is_bounded_and_prefers_recent_turns() {
        let large_output = "x".repeat(default_context_budget_chars() + 1024);
        let turns = vec![
            CodeSessionTurn {
                id: "turn-000001".to_string(),
                time: CodeSessionTurnTime {
                    created: 1,
                    updated: 1,
                },
                task: "old".to_string(),
                recipe: "explore".to_string(),
                profile: "examples/code-agent/explore.air-profile.yaml".to_string(),
                input: json!({"task": "old"}),
                completed: true,
                trace_files: Vec::new(),
                summary: CodeSessionTurnSummary::default(),
                parts: Vec::new(),
                outputs: json!({"summary": "older turn"}),
            },
            CodeSessionTurn {
                id: "turn-000002".to_string(),
                time: CodeSessionTurnTime {
                    created: 2,
                    updated: 2,
                },
                task: "new".to_string(),
                recipe: "repair".to_string(),
                profile: "examples/code-agent/repair-core.air-profile.yaml".to_string(),
                input: json!({"task": "new"}),
                completed: false,
                trace_files: Vec::new(),
                summary: CodeSessionTurnSummary::default(),
                parts: Vec::new(),
                outputs: json!({"summary": large_output}),
            },
        ];

        let feedback = code_session_feedback(&turns);

        assert!(feedback.chars().count() <= default_context_budget_chars());
        assert!(feedback.contains("turn 2"));
        assert!(feedback.contains("AIR_TRUNCATED"));
    }

    #[test]
    fn loop_feedback_is_bounded_and_prefers_recent_iterations() {
        let large_output = "y".repeat(default_context_budget_chars() + 1024);
        let iterations = vec![
            json!({"iteration": 1, "completed": false, "outputs": {"summary": "old"}}),
            json!({"iteration": 2, "completed": false, "outputs": {"summary": large_output}}),
        ];

        let feedback = code_loop_feedback(&iterations);

        assert!(feedback.chars().count() <= default_context_budget_chars());
        assert!(feedback.contains("iteration 2"));
        assert!(feedback.contains("AIR_TRUNCATED"));
    }

    #[test]
    fn session_state_roundtrips_turns() {
        let path = std::env::temp_dir().join(format!(
            "air-code-session-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut state = CodeSessionState::default();
        state.append_turn(CodeSessionTurn {
            id: code_session_turn_id(1),
            time: CodeSessionTurnTime {
                created: 42,
                updated: 42,
            },
            task: "fix it".to_string(),
            recipe: "repair".to_string(),
            profile: "examples/code-agent/repair-core.air-profile.yaml".to_string(),
            input: json!({"task": "fix it"}),
            completed: false,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            outputs: json!({"repair": {"final_success": false}}),
        });

        state.write(&path).unwrap();
        let roundtrip = CodeSessionState::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(roundtrip.version, 1);
        assert_eq!(roundtrip.turns.len(), 1);
        assert_eq!(roundtrip.turns[0].id, "turn-000001");
        assert_eq!(roundtrip.turns[0].time.created, 42);
        assert_eq!(roundtrip.turns[0].time.updated, 42);
        assert_eq!(roundtrip.turns[0].recipe, "repair");
        assert!(!roundtrip.turns[0].completed);
    }

    #[test]
    fn build_recipe_requires_output() {
        let error = build_input(CodeInputOptions {
            task: "build a page".to_string(),
            recipe: CodeRecipe::Build,
            target: None,
            test: None,
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap_err();

        assert!(error.to_string().contains("requires --output"));
    }

    #[test]
    fn builds_review_input() {
        let input = build_input(CodeInputOptions {
            task: "review search".to_string(),
            recipe: CodeRecipe::Review,
            target: Some(PathBuf::from("scripts/search.cjs")),
            test: None,
            query: Some("search".to_string()),
            related: vec![PathBuf::from("scripts/search.test.cjs")],
            search_query: None,
            repo_query: None,
            required_terms: vec!["playwright".to_string()],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["search_query"],
            Value::String("review search".to_string())
        );
        assert_eq!(input["repo_query"], Value::String("search".to_string()));
        assert_eq!(
            input["target_file"],
            Value::String("scripts/search.cjs".to_string())
        );
        assert_eq!(
            input["related_files"],
            Value::Array(vec![Value::String("scripts/search.test.cjs".to_string())])
        );
        assert_eq!(
            input["required_terms"],
            Value::Array(vec![Value::String("playwright".to_string())])
        );
    }

    #[test]
    fn review_input_defaults_related_files_to_target() {
        let input = build_input(CodeInputOptions {
            task: "review search".to_string(),
            recipe: CodeRecipe::Review,
            target: Some(PathBuf::from("scripts/search.cjs")),
            test: None,
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["related_files"],
            Value::Array(vec![Value::String("scripts/search.cjs".to_string())])
        );
    }
}
