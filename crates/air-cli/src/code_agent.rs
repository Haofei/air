use crate::code_budget::{code_budget_limit_status, code_budget_limit_violation, CodeBudgetLimits};
#[cfg(test)]
use crate::code_context::default_context_budget_chars;
#[cfg(test)]
use crate::code_input::build_input;
pub(crate) use crate::code_input::CodeRecipe;
use crate::code_input::{
    build_input_with_pack, default_profile, path_ref_to_input_string, recipe_name, CodeInputOptions,
};
#[cfg(test)]
use crate::code_loop::{code_loop_feedback, code_loop_iteration_input};
use crate::code_loop::{code_outputs_complete, iteration_path, run_code_loop, CodeLoopOptions};
use crate::code_pack::{load_code_agent_pack, CodeAgentInputFacts, CodeAgentPackContext};
use crate::code_session::{
    code_session_feedback, code_session_turn_id, code_session_turn_index,
    code_session_workspace_revert, CodeSessionPart, CodeSessionPatchSet, CodeSessionState,
    CodeSessionTurn, CodeSessionTurnPack, CodeSessionTurnSummary, CodeSessionTurnTime,
};
use crate::explain::build_plan_explanation;
use crate::planner::module_base_dir_for_store_path;
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::run_plan::{run_plan, run_plan_capture, RunPlanOptions};
use air_runtime::{read_trace_jsonl, TraceEvent, TraceStatus};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct CodeOptions {
    pub(crate) task: String,
    pub(crate) pack: Option<PathBuf>,
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
    pub(crate) max_iterations: usize,
    pub(crate) max_estimated_model_calls: Option<usize>,
    pub(crate) max_estimated_tool_calls: Option<usize>,
    pub(crate) tool_config: Option<PathBuf>,
}

pub(crate) struct CodeSessionOptions {
    pub(crate) session: PathBuf,
    pub(crate) fork: Option<PathBuf>,
    pub(crate) revert_to: Option<String>,
    pub(crate) revert_workspace_turn: Option<String>,
    pub(crate) apply_workspace: bool,
    pub(crate) in_place: bool,
}

pub(crate) fn code(options: CodeOptions) -> Result<()> {
    let CodeOptions {
        task,
        pack,
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
        max_iterations,
        max_estimated_model_calls,
        max_estimated_tool_calls,
        tool_config,
    } = options;
    let budget_limits = CodeBudgetLimits {
        max_estimated_model_calls,
        max_estimated_tool_calls,
    };
    let pack = load_code_agent_pack(pack)?;

    let requested_recipe = CodeRecipe::Edit;
    let recipe = CodeRecipe::Edit;
    pack.validate_recipe_input_facts(
        recipe_name(recipe),
        &CodeAgentInputFacts {
            task: !task.trim().is_empty(),
        },
    )?;
    let profile = match profile {
        Some(profile) => profile,
        None => default_profile(&pack, recipe)?,
    };
    let mut input = build_input_with_pack(&pack, CodeInputOptions { task })?;
    let mut session_state = match session.as_ref() {
        Some(path) => Some(CodeSessionState::read(path)?),
        None => None,
    };
    if let Some(state) = session_state.as_ref() {
        input = code_session_turn_input(&input, &state.turns);
    }

    if explain {
        print_explain(CodePrintExplainOptions {
            resolved_recipe: recipe,
            pack: &pack,
            profile: &profile,
            input: &input,
            loop_enabled,
            max_iterations,
            budget_limits,
        })?;
        return Ok(());
    }

    enforce_code_profile_budget_limits(
        &profile,
        if loop_enabled { max_iterations } else { 1 },
        budget_limits,
    )?;

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
    let outputs = if loop_enabled {
        run_code_loop(CodeLoopOptions {
            pack: pack.clone(),
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
        let turn_id = code_session_turn_id(turn_number);
        let turn_time = CodeSessionTurnTime::now();
        let trace_files =
            code_session_trace_files(effective_trace_out.as_ref(), loop_enabled, &outputs);
        let parts = code_session_parts(&trace_files)?;
        let summary = code_session_turn_summary(&parts);
        let patch_sets = code_session_patch_sets(&outputs);
        state.append_turn(CodeSessionTurn {
            id: turn_id,
            time: turn_time,
            task: session_input
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            requested_recipe: Some(recipe_name(requested_recipe).to_string()),
            recipe: recipe_name(recipe).to_string(),
            profile: path_ref_to_input_string(&profile),
            pack: code_session_turn_pack(&pack, recipe, &profile),
            input: Value::Object(session_input),
            completed: code_outputs_complete(&pack, recipe, &outputs)?,
            trace_files: trace_files
                .iter()
                .map(|path| path_ref_to_input_string(path))
                .collect(),
            summary,
            parts,
            patch_sets,
            recovery: None,
            outputs: outputs.clone(),
        });
        state.write(path)?;
    }
    serde_json::to_writer_pretty(std::io::stdout(), &outputs)?;
    println!();
    Ok(())
}

pub(crate) fn code_session(options: CodeSessionOptions) -> Result<()> {
    let CodeSessionOptions {
        session,
        fork,
        revert_to,
        revert_workspace_turn,
        apply_workspace,
        in_place,
    } = options;
    let mut state = CodeSessionState::read(&session)?;
    let original_turn_count = state.turns.len();
    let workspace_revert = if let Some(target) = revert_workspace_turn.as_deref() {
        Some(code_session_workspace_revert(
            &state,
            target,
            apply_workspace,
        )?)
    } else {
        None
    };
    let mut reverted_to = None;
    if let Some(target) = revert_to.as_deref() {
        let index = code_session_turn_index(&state, target)?;
        state.turns.truncate(index + 1);
        reverted_to = state.turns.get(index).map(|turn| turn.id.clone());
    }
    let output_path = if let Some(path) = fork {
        Some(path)
    } else if in_place {
        Some(session.clone())
    } else {
        None
    };
    if revert_to.is_some() && output_path.is_none() {
        bail!("air code-session --revert-to requires --fork or --in-place");
    }
    if let Some(path) = output_path.as_ref() {
        state.write(path)?;
    }
    let trace_file_count = state
        .turns
        .iter()
        .map(|turn| turn.trace_files.len())
        .sum::<usize>();
    let patch_set_count = state
        .turns
        .iter()
        .map(|turn| turn.patch_sets.len())
        .sum::<usize>();
    let summary = json!({
        "source": path_ref_to_input_string(&session),
        "output": output_path.as_ref().map(|path| path_ref_to_input_string(path)),
        "version": state.version,
        "original_turn_count": original_turn_count,
        "turn_count": state.turns.len(),
        "latest_turn_id": state.turns.last().map(|turn| turn.id.as_str()),
        "reverted_to": reverted_to,
        "trace_file_count": trace_file_count,
        "patch_set_count": patch_set_count,
        "workspace_reverted": workspace_revert
            .as_ref()
            .is_some_and(|summary| summary.applied && summary.success),
        "workspace_revert": workspace_revert,
        "note": if apply_workspace {
            "AIR code-session applied reverse patches from indexed patch_sets after dry-run checks."
        } else {
            "AIR code-session forks/truncates session history and only dry-runs workspace reverse patches unless --apply-workspace is set."
        }
    });
    serde_json::to_writer_pretty(std::io::stdout(), &summary)?;
    println!();
    Ok(())
}

fn code_session_turn_pack(
    pack: &CodeAgentPackContext,
    recipe: CodeRecipe,
    active_profile: &Path,
) -> Option<CodeSessionTurnPack> {
    let recipe = pack.recipe_for_id(recipe_name(recipe)).ok()?;
    let default_profile = path_ref_to_input_string(&recipe.default_profile);
    let active_profile = path_ref_to_input_string(active_profile);
    Some(CodeSessionTurnPack {
        path: path_ref_to_input_string(&pack.path),
        recipe: recipe.id,
        profile_override: active_profile != default_profile,
        default_profile,
        intent: recipe.intent,
        completion: recipe.completion,
    })
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
        let start_meta = code_session_start_meta_by_action(&events);
        let trace_file = path_ref_to_input_string(trace_file);
        for event in &events {
            let Some(mut part) = code_session_part_from_event(&trace_file, event) else {
                continue;
            };
            if (part.model.is_none() || part.tool.is_none() || part.approval_for.is_empty())
                && event
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("_air_truncated"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                if let Some(meta) = start_meta.get(&code_session_event_key(event)) {
                    if part.model.is_none() {
                        part.model = meta
                            .get("model")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                    }
                    if part.tool.is_none() {
                        part.tool = meta
                            .get("tool")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                    }
                    if part.approval_for.is_empty() {
                        part.approval_for = meta
                            .get("approval_for")
                            .and_then(Value::as_array)
                            .map(|values| {
                                values
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(ToString::to_string)
                                    .collect()
                            })
                            .unwrap_or_default();
                    }
                }
            }
            parts.push(part);
        }
    }
    Ok(parts)
}

fn code_session_start_meta_by_action(events: &[TraceEvent]) -> HashMap<String, Value> {
    let mut start_meta = HashMap::new();
    for event in events {
        if !matches!(
            event.action.as_str(),
            "model_call_start"
                | "tool_call_start"
                | "tool_batch_dispatch_item_start"
                | "approval_start"
        ) {
            continue;
        }
        let Some(meta) = event.meta.clone() else {
            continue;
        };
        let action = event.action.trim_end_matches("_start");
        start_meta.insert(code_session_event_key_with_action(event, action), meta);
    }
    start_meta
}

fn code_session_event_key(event: &TraceEvent) -> String {
    code_session_event_key_with_action(event, &event.action)
}

fn code_session_event_key_with_action(event: &TraceEvent, action: &str) -> String {
    format!("{}:{}:{}:{}", event.agent, event.step, event.rule, action)
}

fn code_session_turn_summary(parts: &[CodeSessionPart]) -> CodeSessionTurnSummary {
    let mut summary = CodeSessionTurnSummary::default();
    for part in parts {
        if part.kind == "model_call" {
            summary.model_call_count += 1;
        }
        if matches!(part.kind.as_str(), "tool_call" | "tool_batch_dispatch_item") {
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

fn code_session_patch_sets(outputs: &Value) -> Vec<CodeSessionPatchSet> {
    let mut patch_sets = Vec::new();
    collect_code_session_patch_sets(outputs, "$", &mut patch_sets);
    patch_sets
}

fn collect_code_session_patch_sets(
    value: &Value,
    path: &str,
    patch_sets: &mut Vec<CodeSessionPatchSet>,
) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Object(workspace_diff)) = object.get("workspace_diff") {
                let artifact_ids = workspace_diff
                    .get("artifacts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|artifact| artifact.get("id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                patch_sets.push(CodeSessionPatchSet {
                    source: path.to_string(),
                    repo: workspace_diff
                        .get("repo")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    workspace_clean_before: object
                        .get("workspace_clean_before")
                        .and_then(Value::as_bool),
                    workspace_clean_after: object.get("workspace_clean").and_then(Value::as_bool),
                    preexisting_changed_files: object
                        .get("preexisting_changed_files")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    changed_files: object
                        .get("changed_files")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    workspace_changed_files: object
                        .get("workspace_changed_files")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    diff: workspace_diff
                        .get("diff")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    diff_bytes: workspace_diff.get("bytes").and_then(Value::as_u64),
                    diff_truncated: workspace_diff.get("truncated").and_then(Value::as_bool),
                    artifact_ids,
                });
            }
            for (key, child) in object {
                collect_code_session_patch_sets(child, &format!("{path}.{key}"), patch_sets);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_code_session_patch_sets(child, &format!("{path}[{index}]"), patch_sets);
            }
        }
        _ => {}
    }
}

fn sort_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn code_session_part_from_event(trace_file: &str, event: &TraceEvent) -> Option<CodeSessionPart> {
    if !matches!(
        event.action.as_str(),
        "model_call" | "tool_call" | "tool_batch_dispatch_item" | "approval" | "return"
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

struct CodePrintExplainOptions<'a> {
    resolved_recipe: CodeRecipe,
    pack: &'a CodeAgentPackContext,
    profile: &'a Path,
    input: &'a Map<String, Value>,
    loop_enabled: bool,
    max_iterations: usize,
    budget_limits: CodeBudgetLimits,
}

fn print_explain(options: CodePrintExplainOptions<'_>) -> Result<()> {
    let CodePrintExplainOptions {
        resolved_recipe,
        pack,
        profile,
        input,
        loop_enabled,
        max_iterations,
        budget_limits,
    } = options;
    let metadata = explain_metadata_for_profile(profile)?;
    let pack_recipe = pack.recipe_for_id(recipe_name(resolved_recipe))?;
    let pack_default_profile = path_ref_to_input_string(&pack_recipe.default_profile);
    let active_profile = path_ref_to_input_string(profile);
    let budget_iterations = if loop_enabled { max_iterations } else { 1 };
    let total_model_calls = metadata
        .max_estimated_model_calls
        .saturating_mul(budget_iterations);
    let total_tool_calls = metadata
        .max_estimated_tool_calls
        .saturating_mul(budget_iterations);
    let budget_limit = code_budget_limit_status(total_model_calls, total_tool_calls, budget_limits);
    let explanation = json!({
        "command": "code",
        "will_run": false,
        "pack": {
            "path": path_ref_to_input_string(&pack.path),
            "default_profile": pack_default_profile,
            "profile_override": active_profile != pack_default_profile,
            "intent": pack_recipe.intent,
            "input": pack_recipe.input,
            "completion": pack_recipe.completion,
        },
        "profile": active_profile,
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
                "max_estimated_model_calls": total_model_calls,
                "max_estimated_tool_calls": total_tool_calls
            }
        },
        "budget_limit": budget_limit,
        "loop": {
            "enabled": loop_enabled,
            "max_iterations": max_iterations
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

fn enforce_code_profile_budget_limits(
    profile: &Path,
    iterations: usize,
    budget_limits: CodeBudgetLimits,
) -> Result<()> {
    if budget_limits.max_estimated_model_calls.is_none()
        && budget_limits.max_estimated_tool_calls.is_none()
    {
        return Ok(());
    }
    let metadata = explain_metadata_for_profile(profile)?;
    let model_calls = metadata
        .max_estimated_model_calls
        .saturating_mul(iterations);
    let tool_calls = metadata.max_estimated_tool_calls.saturating_mul(iterations);
    if let Some(violation) = code_budget_limit_violation(model_calls, tool_calls, budget_limits) {
        bail!(
            "estimated code-agent budget exceeded: model_calls={} tool_calls={} limit={}",
            model_calls,
            tool_calls,
            violation
        );
    }
    Ok(())
}

fn is_workspace_write_capability(capability: &str) -> bool {
    matches!(capability, "file.write")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_code_input_keys(input: &Map<String, Value>) {
        let keys = input.keys().map(String::as_str).collect::<HashSet<_>>();
        assert_eq!(keys, HashSet::from(["task", "acceptance_assertions"]));
    }

    #[test]
    fn code_agent_pack_declares_all_default_profiles() {
        let pack = load_code_agent_pack(None).unwrap();
        let recipes = [CodeRecipe::Edit];

        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert_eq!(pack.pack.recipes.len(), recipes.len());
        for recipe in recipes {
            let profile = default_profile(&pack, recipe).unwrap();
            assert!(
                workspace_root.join(&profile).exists(),
                "default profile for {} does not exist: {}",
                recipe_name(recipe),
                profile.display()
            );
            assert!(
                pack.recipe_for_id(recipe_name(recipe))
                    .unwrap()
                    .default_profile
                    == profile,
                "pack is missing {} -> {}",
                recipe_name(recipe),
                profile.display()
            );
        }
    }

    #[test]
    fn builds_edit_input() {
        let input = build_input(CodeInputOptions {
            task: "fix it".to_string(),
        })
        .unwrap();

        assert_eq!(input["task"], Value::String("fix it".to_string()));
        assert_eq!(input["acceptance_assertions"], Value::Array(Vec::new()));
        assert_code_input_keys(&input);
    }

    #[test]
    fn edit_input_uses_task_as_the_only_request() {
        let input = build_input(CodeInputOptions {
            task: "move shared helper".to_string(),
        })
        .unwrap();

        assert_code_input_keys(&input);
    }

    #[test]
    fn edit_input_keeps_source_paths_inside_task_text() {
        let input = build_input(CodeInputOptions {
            task: "Refactor crates/air-tools/src/file_tools.rs by extracting a shared helper for resolving and validating a repo-relative input path inside base_dir, then use it from file_write and file_edit without changing behavior.".to_string(),
        })
        .unwrap();

        assert_code_input_keys(&input);
    }

    #[test]
    fn code_input_preserves_open_ended_task_text() {
        let input = build_input(CodeInputOptions {
            task: "fix the failing add function".to_string(),
        })
        .unwrap();

        assert_eq!(
            input["task"],
            Value::String("fix the failing add function".to_string())
        );
        assert_code_input_keys(&input);
    }

    #[test]
    fn code_input_uses_single_loop_for_write_task() {
        let input = build_input(CodeInputOptions {
            task: "change the provider and keep tests passing".to_string(),
        })
        .unwrap();
        assert_code_input_keys(&input);
    }

    #[test]
    fn code_input_uses_unified_loop_even_with_review_words() {
        let input = build_input(CodeInputOptions {
            task: "review it".to_string(),
        })
        .unwrap();

        assert_eq!(input["task"], Value::String("review it".to_string()));
        assert_code_input_keys(&input);
    }

    #[test]
    fn code_input_uses_unified_loop_for_analysis_tasks() {
        let input = build_input(CodeInputOptions {
            task: "understand the next code-agent milestone".to_string(),
        })
        .unwrap();

        assert_eq!(
            input["task"],
            Value::String("understand the next code-agent milestone".to_string())
        );
        assert_code_input_keys(&input);
    }

    #[test]
    fn code_input_does_not_route_to_read_only_explore() {
        let input = build_input(CodeInputOptions {
            task: "understand this".to_string(),
        })
        .unwrap();

        assert_code_input_keys(&input);
    }

    #[test]
    fn explain_payload_reports_unified_code_loop() {
        let input = build_input(CodeInputOptions {
            task: "fix it".to_string(),
        })
        .unwrap();
        let explanation = json!({
            "command": "code",
            "will_run": false,
            "pack": {
                "path": crate::code_pack::CODE_AGENT_PACK_PATH,
                "default_profile": path_ref_to_input_string(
                    &default_profile(&load_code_agent_pack(None).unwrap(), CodeRecipe::Edit)
                        .unwrap()
                ),
                "profile_override": false,
                "intent": load_code_agent_pack(None)
                    .unwrap()
                    .recipe_for_id("edit")
                    .unwrap()
                    .intent,
            },
            "profile": path_ref_to_input_string(
                &default_profile(&load_code_agent_pack(None).unwrap(), CodeRecipe::Edit)
                    .unwrap()
            ),
            "input": Value::Object(input),
        });

        assert!(explanation.get("requested_recipe").is_none());
        assert!(explanation.get("resolved_recipe").is_none());
        assert_eq!(
            explanation["profile"],
            Value::String("examples/code-agent/edit.air-profile.yaml".to_string())
        );
        assert_eq!(
            explanation["pack"]["path"],
            Value::String("examples/code-agent/code-agent.air-pack.yaml".to_string())
        );
        assert!(explanation["pack"].get("recipe").is_none());
        assert_eq!(
            explanation["pack"]["default_profile"],
            Value::String("examples/code-agent/edit.air-profile.yaml".to_string())
        );
        assert_eq!(explanation["pack"]["profile_override"], Value::Bool(false));
        let input = explanation["input"].as_object().unwrap();
        assert_code_input_keys(input);
    }

    #[test]
    fn explain_metadata_reports_profile_capabilities() {
        let profile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/edit.air-profile.yaml");
        let metadata = explain_metadata_for_profile(&profile).unwrap();

        assert!(metadata
            .capabilities
            .iter()
            .any(|capability| capability == "file.write"));
        assert!(metadata.writes_workspace);
        assert!(!metadata.read_only);
        assert!(metadata.max_estimated_model_calls > 0);
        assert!(metadata.max_estimated_tool_calls > 0);
        assert!(metadata.plan.ends_with("code-edit.air-plan.yaml"));
    }

    #[test]
    fn edit_loop_exposes_core_opencode_style_discovery_tools() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let declared_tools = yaml["tools"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<HashSet<_>>();
        let choose_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        let allowed_tools = choose_rule["actions"][0]["input"]["object"]["allowed_tools"]["array"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["literal"].as_str())
            .collect::<HashSet<_>>();

        for tool in [
            "todo.write",
            "todo.read",
            "glob",
            "repo.symbols",
            "lsp.references",
            "lsp.diagnostics",
            "read",
            "grep",
            "edit",
            "test.run",
            "git.diff",
        ] {
            assert!(
                declared_tools.contains(tool),
                "missing declared tool {tool}"
            );
            assert!(
                allowed_tools.contains(tool),
                "tool {tool} is not visible to the edit decider"
            );
        }
        assert!(
            declared_tools.contains("format.run"),
            "formatter must be declared for model-selected and final-gate validation"
        );
        assert!(
            allowed_tools.contains("format.run"),
            "formatter should be model-selected like OpenCode, with AIR still enforcing the final gate"
        );
    }

    #[test]
    fn edit_loop_uses_one_choose_rule_for_targeted_and_general_tasks() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rules = yaml["workflow"]["rules"].as_sequence().unwrap();
        let choose_count = rules
            .iter()
            .filter(|rule| rule["id"].as_str() == Some("choose"))
            .count();
        assert_eq!(
            choose_count, 1,
            "targeted and general tasks should share the same OpenCode-style choose/act loop"
        );
        let choose_rule = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        assert_eq!(
            choose_rule["when"],
            serde_yaml::Value::String("phase == \"choose\"".to_string())
        );
        let allowed_tools = choose_rule["actions"][0]["input"]["object"]["allowed_tools"]["array"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["literal"].as_str())
            .collect::<HashSet<_>>();

        for tool in [
            "todo.write",
            "todo.read",
            "glob",
            "repo.symbols",
            "lsp.references",
            "read",
            "grep",
            "edit",
            "git.diff",
            "format.run",
            "test.run",
        ] {
            assert!(
                allowed_tools.contains(tool),
                "single choose loop should expose {tool}"
            );
        }
    }

    #[test]
    fn edit_loop_uses_direct_phase_kernel() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rules = yaml["workflow"]["rules"].as_sequence().unwrap();
        let rule_ids = rules
            .iter()
            .map(|rule| rule["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            rule_ids,
            vec![
                "init",
                "summarize-at-step-limit",
                "choose",
                "act",
                "edit-applied",
                "manual-format-failed",
                "manual-format-test-failed",
                "manual-format-test-passed-with-assertions",
                "manual-format-test-passed",
                "manual-format-passed",
                "final-format-before-complete",
                "final-format-tool-error",
                "final-format-failed",
                "final-verify-after-format",
                "final-verify-truncated-failed",
                "final-verify-failed",
                "final-verify-passed-with-assertions",
                "final-acceptance-assertions-failed",
                "final-acceptance-assertions-passed",
                "acceptance-assertions-failed",
                "acceptance-assertions-passed",
                "final-verify-passed",
                "manual-test-failed",
                "manual-test-passed-with-assertions",
                "manual-test-passed",
                "edit-validation-failed",
                "continue-after-act",
                "summarize",
                "done",
            ]
        );
    }

    #[test]
    fn edit_loop_records_tool_results_for_provider_tool_history() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rules = yaml["workflow"]["rules"].as_sequence().unwrap();
        let act_rule = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("act"))
            .unwrap();

        assert_eq!(
            act_rule["actions"][1]["target"],
            serde_yaml::Value::String("observations".to_string())
        );
        assert_eq!(
            act_rule["actions"][1]["value"]["object"]["action"]["literal"],
            serde_yaml::Value::String("tool_result".to_string())
        );
        assert_eq!(
            act_rule["actions"][1]["value"]["object"]["result"]["ref"],
            serde_yaml::Value::String("observation".to_string())
        );
        assert_eq!(
            act_rule["actions"][1]["value"]["object"]["assistant"]["ref"],
            serde_yaml::Value::String("decision".to_string())
        );
        assert_eq!(
            act_rule["actions"][1]["value"]["object"]["requested"]["ref"],
            serde_yaml::Value::String("decision.tool_calls".to_string())
        );

        let choose_rule = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        let choose_input = choose_rule["actions"][0]["input"]["object"]
            .as_mapping()
            .unwrap();
        let observations_key = serde_yaml::Value::String("observations".to_string());
        assert_eq!(
            choose_input[&observations_key]["take_last_within_bytes"]["ref"],
            serde_yaml::Value::String("observations".to_string())
        );
    }

    #[test]
    fn edit_loop_exposes_opencode_style_edit_contract() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let choose_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        let edit_contract =
            &choose_rule["actions"][0]["input"]["object"]["tool_schemas"]["literal"]["edit"];
        let required = edit_contract["required"].as_mapping().unwrap();
        let optional = edit_contract["optional"].as_mapping().unwrap();

        assert!(
            required.contains_key(serde_yaml::Value::String("filePath".to_string())),
            "edit tool contract should expose OpenCode-style required key filePath"
        );
        for key in ["oldString", "newString", "edits", "replaceAll"] {
            assert!(
                optional.contains_key(serde_yaml::Value::String(key.to_string())),
                "edit tool contract should expose OpenCode-style optional key {key}"
            );
        }
        assert!(
            !required.contains_key(serde_yaml::Value::String("oldString".to_string()))
                && !required.contains_key(serde_yaml::Value::String("newString".to_string())),
            "edit should allow multi-edit calls that provide edits instead of top-level oldString/newString"
        );
        for key in [
            "path",
            "old_string",
            "new_string",
            "dry_run",
            "max_changed_lines",
            "match_strategy",
        ] {
            assert!(
                !optional.contains_key(serde_yaml::Value::String(key.to_string())),
                "model-visible edit contract should not expose internal/alias key {key}"
            );
        }
    }

    #[test]
    fn edit_loop_context_policy_discourages_broad_repeated_reads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let choose_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        let max_items = choose_rule["actions"][0]["input"]["object"]["observations"]["max_items"]
            .as_u64()
            .unwrap();
        assert!(
            max_items <= 4,
            "recent observations should stay compact enough to avoid read-only loops"
        );
        let max_bytes = choose_rule["actions"][0]["input"]["object"]["observations"]["max_bytes"]
            .as_u64()
            .unwrap();
        assert!(
            max_bytes <= 24_000,
            "recent observations should not keep growing with repeated exploration"
        );
        let policy = choose_rule["actions"][0]["input"]["object"]["tool_policy"]["literal"]
            ["context"]
            .as_str()
            .unwrap();
        assert!(
            policy.contains("Do not repeat the same read/search"),
            "model context policy should prevent read-only loops: {policy}"
        );
        assert!(
            policy.contains("When the task names an exact file path"),
            "model should start from explicit file paths instead of broad symbol maps: {policy}"
        );
    }

    #[test]
    fn edit_loop_replays_provider_assistant_turn_without_rationale_wrapper() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let decision_schema = &yaml["state"]["decision"];
        let required = decision_schema["required"].as_sequence().unwrap();
        assert!(
            !required
                .iter()
                .any(|field| field.as_str() == Some("rationale")),
            "OpenCode-style tool loops should not require a separate rationale wrapper"
        );

        let act_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("act"))
            .unwrap();
        assert!(
            act_rule["actions"][1]["value"]["object"]
                .get("rationale")
                .is_none(),
            "OpenCode-style tool loops should replay the assistant turn, not a separate rationale field"
        );
        assert!(
            act_rule["actions"][1]["value"]["object"]
                .get("assistant")
                .is_some(),
            "provider assistant content/tool calls should be replayed with tool results"
        );
    }

    #[test]
    fn edit_loop_records_workspace_diff_without_write_path_scope() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let record_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("summarize"))
            .unwrap();
        let diff_action = &record_rule["actions"][0];

        assert_eq!(
            diff_action["input"]["array"][0]["object"]["tool"]["literal"],
            serde_yaml::Value::String("git.diff".to_string())
        );
        assert_eq!(
            diff_action["input"]["array"][0]["object"]["input"]["literal"],
            serde_yaml::Value::Mapping(Default::default()),
            "code agent should audit actual workspace changes instead of restricting final diff to guessed write_paths"
        );
    }

    #[test]
    fn edit_loop_sets_final_diff_fields_deterministically_after_summary() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let summarize_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("summarize"))
            .unwrap();

        assert_eq!(
            summarize_rule["actions"][2]["output"],
            serde_yaml::Value::String("edit_summary".to_string())
        );
        assert_eq!(
            summarize_rule["actions"][3]["values"]["edit"]["object"]["changed_files"]["ref"],
            serde_yaml::Value::String("final_diff_result[0].output.changed_files".to_string())
        );
        assert_eq!(
            summarize_rule["actions"][3]["values"]["edit"]["object"]["workspace_diff"]["ref"],
            serde_yaml::Value::String("final_diff_result[0].output".to_string())
        );
    }

    #[test]
    fn edit_loop_summarizer_uses_bounded_final_evidence() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let summarize_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("summarize"))
            .unwrap();

        let observations = &summarize_rule["actions"][2]["input"]["object"]["observations"];
        assert_eq!(
            observations["take_last_within_bytes"]["ref"],
            serde_yaml::Value::String("observations".to_string())
        );
        assert!(
            observations["max_items"].as_u64().unwrap() <= 8,
            "summarizer should receive only a compact final evidence tail"
        );
        assert!(
            observations["max_bytes"].as_u64().unwrap() <= 30_000,
            "summarizer should not receive the full edit-loop observation history"
        );
        assert_eq!(
            summarize_rule["actions"][2]["input"]["object"]["final_diff_result"]["ref"],
            serde_yaml::Value::String("final_diff_result".to_string())
        );
    }

    #[test]
    fn edit_loop_handles_failed_final_verify_before_success() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rule_ids = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|rule| rule["id"].as_str())
            .collect::<Vec<_>>();
        let failed_output = rule_ids
            .iter()
            .position(|id| *id == "final-verify-failed")
            .unwrap();
        let passed = rule_ids
            .iter()
            .position(|id| *id == "final-verify-passed")
            .unwrap();

        assert!(
            failed_output < passed,
            "failed verification should be classified before the generic success branch"
        );
    }

    #[test]
    fn edit_loop_successful_manual_verification_summarizes_without_extra_decider_step() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rules = yaml["workflow"]["rules"].as_sequence().unwrap();

        for id in ["acceptance-assertions-passed", "manual-test-passed"] {
            let rule = rules
                .iter()
                .find(|rule| rule["id"].as_str() == Some(id))
                .unwrap();
            assert_eq!(
                rule["actions"][0]["values"]["verification_status"],
                serde_yaml::Value::String("passed".to_string()),
                "{id} should mark verification as passed"
            );
            assert_eq!(
                rule["actions"][0]["values"]["phase"],
                serde_yaml::Value::String("summarize".to_string()),
                "{id} should summarize immediately after model-selected verification passes"
            );
        }

        for id in ["final-verify-passed", "final-acceptance-assertions-passed"] {
            let rule = rules
                .iter()
                .find(|rule| rule["id"].as_str() == Some(id))
                .unwrap();
            assert_eq!(
                rule["actions"][0]["values"]["verification_status"],
                serde_yaml::Value::String("passed".to_string()),
                "{id} should mark verification as passed"
            );
            assert_eq!(
                rule["actions"][0]["values"]["phase"],
                serde_yaml::Value::String("summarize".to_string()),
                "{id} should summarize immediately after a user-requested final verification gate passes"
            );
        }
    }

    #[test]
    fn edit_loop_collects_diagnostic_context_after_failed_final_verify() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("final-verify-failed"))
            .unwrap();
        let actions = rule["actions"].as_sequence().unwrap();
        let diagnostic_dispatch = actions
            .iter()
            .position(|action| action["kind"].as_str() == Some("tool_batch_dispatch"))
            .unwrap();
        let set_phase = actions
            .iter()
            .position(|action| action["kind"].as_str() == Some("set"))
            .unwrap();
        let tool = &actions[diagnostic_dispatch]["input"]["array"][0]["object"];

        assert!(
            diagnostic_dispatch < set_phase,
            "diagnostic context should be collected before returning to the model correction step"
        );
        assert_eq!(
            tool["tool"]["literal"],
            serde_yaml::Value::String("diagnostic.context".to_string())
        );
        assert_eq!(
            tool["input"]["object"]["diagnostics"]["ref"],
            serde_yaml::Value::String("verify_result[0].output.diagnostics".to_string())
        );
        assert_eq!(
            actions[diagnostic_dispatch + 1]["value"]["object"]["action"]["literal"],
            serde_yaml::Value::String("verification_context".to_string())
        );
    }

    #[test]
    fn edit_loop_handles_edit_validation_failures_before_continue() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rule_ids = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|rule| rule["id"].as_str())
            .collect::<Vec<_>>();
        let edit_failed = rule_ids
            .iter()
            .position(|id| *id == "edit-validation-failed")
            .unwrap();
        let continue_after_act = rule_ids
            .iter()
            .position(|id| *id == "continue-after-act")
            .unwrap();

        assert!(
            edit_failed < continue_after_act,
            "edit validation failures must be observed before the generic post_act transition"
        );
        let edit_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("edit-validation-failed"))
            .unwrap();
        assert_eq!(
            edit_rule["actions"][0]["input"]["array"][0]["object"]["tool"]["literal"],
            serde_yaml::Value::String("diagnostic.context".to_string())
        );
        assert_eq!(
            edit_rule["actions"][1]["value"]["object"]["action"]["literal"],
            serde_yaml::Value::String("edit_validation_context".to_string())
        );
    }

    #[test]
    fn edit_loop_preserves_tool_errors_in_raw_observations() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let act_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("act"))
            .unwrap();

        assert_eq!(
            act_rule["actions"][1]["value"]["object"]["result"]["ref"],
            serde_yaml::Value::String("observation".to_string()),
            "raw tool results, including errors, should flow back into the single observation stream"
        );
    }

    #[test]
    fn edit_loop_generic_post_act_returns_to_choose() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("continue-after-act"))
            .unwrap();
        assert_eq!(
            rule["when"],
            serde_yaml::Value::String("phase == \"post_act\"".to_string())
        );
        assert_eq!(
            rule["actions"][0]["values"]["phase"],
            serde_yaml::Value::String("choose".to_string())
        );
    }

    #[test]
    fn edit_loop_uses_realistic_bounded_coding_budget() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();

        let max_steps = yaml["workflow"]["max_steps"].as_u64().unwrap();
        let max_model_calls = yaml["policy"]["max_model_calls"].as_u64().unwrap();
        let max_tool_calls = yaml["policy"]["max_tool_calls"].as_u64().unwrap();
        let max_repeated_tool_calls = yaml["policy"]["max_repeated_tool_calls"].as_u64().unwrap();
        let choose_rule = yaml["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        let choose_retry_attempts = choose_rule["actions"][0]["retry"]["max_attempts"]
            .as_u64()
            .unwrap();

        assert!(
            (120..=200).contains(&max_steps),
            "edit loop needs enough steps for explore/edit/verify/fix cycles while staying bounded"
        );
        assert!(
            (32..=48).contains(&max_model_calls),
            "real code-agent loops need room for multiple model-guided edit and verification rounds"
        );
        assert!(
            (160..=240).contains(&max_tool_calls),
            "tool budget should allow repeated narrow reads, writes, tests, diagnostics, and diffs"
        );
        assert!(
            (3..=4).contains(&max_repeated_tool_calls),
            "OpenCode-style doom-loop feedback should stop repeated identical tool calls quickly"
        );
        assert!(
            (4..=6).contains(&choose_retry_attempts),
            "real models sometimes need multiple schema retries before a valid decider object"
        );
    }

    #[test]
    fn edit_loop_exposes_runtime_budget_and_summarizes_at_step_limit() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let rules = yaml["workflow"]["rules"].as_sequence().unwrap();
        let rule_ids = rules
            .iter()
            .filter_map(|rule| rule["id"].as_str())
            .collect::<Vec<_>>();
        let step_limit = rule_ids
            .iter()
            .position(|id| *id == "summarize-at-step-limit")
            .unwrap();
        let choose = rule_ids.iter().position(|id| *id == "choose").unwrap();
        let choose_rule = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        let step_limit_rule = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("summarize-at-step-limit"))
            .unwrap();

        assert!(
            step_limit < choose,
            "step-limit guard must run before the normal decider can choose more tools"
        );
        assert_eq!(
            choose_rule["actions"][0]["input"]["object"]["runtime"]["ref"],
            serde_yaml::Value::String("_air".to_string())
        );
        assert_eq!(
            step_limit_rule["when"],
            serde_yaml::Value::String(
                "phase == \"choose\" && _air.is_last_action_step == true".to_string()
            )
        );
        assert_eq!(
            step_limit_rule["actions"][1]["input"]["object"]["runtime"]["ref"],
            serde_yaml::Value::String("_air".to_string())
        );
    }

    #[test]
    fn completion_detection_matches_recipe_outputs() {
        let pack = load_code_agent_pack(None).unwrap();
        assert!(code_outputs_complete(
            &pack,
            CodeRecipe::Edit,
            &json!({"edit": {"final_success": true}})
        )
        .unwrap());
        assert!(!code_outputs_complete(
            &pack,
            CodeRecipe::Edit,
            &json!({"edit": {"final_success": false}})
        )
        .unwrap());
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
    fn session_patch_sets_extract_workspace_diff() {
        let patch_sets = code_session_patch_sets(&json!({
            "edit": {
                "workspace_clean_before": false,
                "workspace_clean": false,
                "changed_files": [{"path": "src/lib.rs"}],
                "workspace_changed_files": [{"path": "src/lib.rs"}, {"path": "README.md"}],
                "preexisting_changed_files": [{"path": "README.md"}],
                "workspace_diff": {
                    "repo": "/tmp/repo",
                    "diff": "diff --git a/src/lib.rs b/src/lib.rs",
                    "bytes": 42,
                    "truncated": false,
                    "artifacts": [{"id": "git-diff:src/lib.rs"}]
                }
            }
        }));

        assert_eq!(patch_sets.len(), 1);
        assert_eq!(patch_sets[0].source, "$.edit");
        assert_eq!(patch_sets[0].repo, Some("/tmp/repo".to_string()));
        assert_eq!(patch_sets[0].workspace_clean_before, Some(false));
        assert_eq!(patch_sets[0].workspace_clean_after, Some(false));
        assert_eq!(
            patch_sets[0].changed_files[0]["path"],
            Value::String("src/lib.rs".to_string())
        );
        assert_eq!(patch_sets[0].diff_bytes, Some(42));
        assert_eq!(patch_sets[0].diff_truncated, Some(false));
        assert_eq!(patch_sets[0].artifact_ids, vec!["git-diff:src/lib.rs"]);
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
                "artifacts": [{"id": "artifact:1", "kind": "git_diff"}]
            })),
            meta: Some(json!({"model": "code_edit_decider"})),
            status: TraceStatus::Ok,
            error: None,
        };

        let part = code_session_part_from_event("trace.jsonl", &event).unwrap();

        assert_eq!(part.kind, "model_call");
        assert_eq!(part.trace_file, "trace.jsonl");
        assert_eq!(part.model, Some("code_edit_decider".to_string()));
        assert!(part.input_keys.iter().any(|key| key == "task"));
        assert!(part.output_keys.iter().any(|key| key == "summary"));
        assert_eq!(part.files, vec!["src/lib.rs"]);
        assert_eq!(part.artifact_ids, vec!["artifact:1"]);
        assert_eq!(part.artifact_kinds, vec!["git_diff"]);
        assert_eq!(part.status, "ok");
    }

    #[test]
    fn session_part_indexes_batch_tool_trace_event() {
        let event = TraceEvent {
            agent: "code-agent".to_string(),
            step: 4,
            rule: "act".to_string(),
            action: "tool_batch_dispatch_item".to_string(),
            input: Some(json!({"path": "src/lib.rs"})),
            output: Some(json!({
                "path": "src/lib.rs",
                "artifacts": [{"id": "file:src/lib.rs", "kind": "file_span"}]
            })),
            meta: Some(json!({"tool": "read", "index": 0})),
            status: TraceStatus::Ok,
            error: None,
        };

        let part = code_session_part_from_event("trace.jsonl", &event).unwrap();

        assert_eq!(part.kind, "tool_batch_dispatch_item");
        assert_eq!(part.tool, Some("read".to_string()));
        assert_eq!(part.files, vec!["src/lib.rs"]);
        assert_eq!(part.artifact_ids, vec!["file:src/lib.rs"]);
        assert_eq!(part.artifact_kinds, vec!["file_span"]);
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
                model: Some("code_edit_decider".to_string()),
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
                kind: "tool_batch_dispatch_item".to_string(),
                trace_file: "trace.jsonl".to_string(),
                agent: "agent".to_string(),
                step: 2,
                rule: "edit".to_string(),
                action: "tool_batch_dispatch_item".to_string(),
                status: "ok".to_string(),
                model: None,
                tool: Some("edit".to_string()),
                approval_for: Vec::new(),
                files: vec!["src/lib.rs".to_string()],
                artifact_ids: vec!["artifact:1".to_string()],
                artifact_kinds: vec!["file_edit".to_string()],
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
        assert_eq!(summary.models, vec!["code_edit_decider"]);
        assert_eq!(summary.tools, vec!["edit"]);
        assert_eq!(summary.approvals, vec!["file.write"]);
        assert_eq!(summary.files, vec!["src/lib.rs"]);
        assert_eq!(summary.artifact_kinds, vec!["file_edit"]);
    }

    #[test]
    fn loop_iteration_input_appends_previous_outputs_to_task() {
        let mut input = Map::new();
        input.insert("task".to_string(), Value::String("fix it".to_string()));
        let iterations = vec![json!({
            "iteration": 1,
            "completed": false,
            "outputs": {
                "edit": {
                    "final_success": false,
                    "diagnostics": [{"path": "src/lib.rs", "line": 3}]
                }
            }
        })];

        let next = code_loop_iteration_input(&input, &iterations);

        assert!(next.get("target_path").is_none());
        let task = next["task"].as_str().unwrap();
        assert!(task.starts_with("fix it"));
        assert!(task.contains("AIR loop context from previous iterations"));
        assert!(task.contains("final_success"));
    }

    #[test]
    fn budget_limit_status_reports_exceeded_dimension() {
        let status = code_budget_limit_status(
            6,
            29,
            CodeBudgetLimits {
                max_estimated_model_calls: Some(5),
                max_estimated_tool_calls: Some(100),
            },
        );

        assert_eq!(status["exceeded"], Value::Bool(true));
        assert_eq!(status["model_exceeded"], Value::Bool(true));
        assert_eq!(status["tool_exceeded"], Value::Bool(false));
        assert_eq!(status["attempted_model_calls"], Value::from(6));
        assert_eq!(status["attempted_tool_calls"], Value::from(29));
    }

    #[test]
    fn session_turn_input_appends_previous_outputs_to_task() {
        let mut input = Map::new();
        input.insert(
            "task".to_string(),
            Value::String("continue investigation".to_string()),
        );
        let turns = vec![CodeSessionTurn {
            id: "turn-000001".to_string(),
            time: CodeSessionTurnTime {
                created: 1,
                updated: 1,
            },
            task: "inspect repo".to_string(),
            requested_recipe: Some("edit".to_string()),
            recipe: "edit".to_string(),
            profile: "examples/code-agent/edit.air-profile.yaml".to_string(),
            pack: None,
            input: json!({"task": "inspect repo"}),
            completed: true,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary {
                models: vec!["code_edit_decider".to_string()],
                tools: vec!["repo.search".to_string()],
                files: vec!["src/lib.rs".to_string()],
                model_call_count: 1,
                tool_call_count: 1,
                ..CodeSessionTurnSummary::default()
            },
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({
                "edit": {
                    "rationale": "Found the dispatch implementation",
                    "source_ids": ["repo:lib"]
                }
            }),
        }];

        let next = code_session_turn_input(&input, &turns);

        assert!(next.get("target_path").is_none());
        let task = next["task"].as_str().unwrap();
        assert!(task.starts_with("continue investigation"));
        assert!(task.contains("AIR session context from previous turns"));
        assert!(task.contains("models=code_edit_decider"));
        assert!(task.contains("tools=repo.search"));
        assert!(task.contains("files=src/lib.rs"));
        assert!(task.contains("Found the dispatch implementation"));
    }

    #[test]
    fn session_parts_recover_model_from_start_event_when_completion_meta_is_truncated() {
        let events = vec![
            serde_json::from_value::<TraceEvent>(json!({
                "agent": "reviewer",
                "step": 1,
                "rule": "analyze",
                "action": "model_call_start",
                "input": {},
                "output": null,
                "meta": {
                    "model": "code_edit_decider",
                    "output": "review",
                    "timeout_seconds": 120
                },
                "status": "ok",
                "error": null
            }))
            .unwrap(),
            serde_json::from_value::<TraceEvent>(json!({
                "agent": "reviewer",
                "step": 1,
                "rule": "analyze",
                "action": "model_call",
                "input": {},
                "output": {"review": {"summary": "large output omitted"}},
                "meta": {"_air_truncated": true},
                "status": "ok",
                "error": null
            }))
            .unwrap(),
        ];
        let start_meta = code_session_start_meta_by_action(&events);
        let mut part = code_session_part_from_event("trace.jsonl", &events[1]).unwrap();
        assert!(part.model.is_none());
        if let Some(meta) = start_meta.get(&code_session_event_key(&events[1])) {
            part.model = meta
                .get("model")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }

        assert_eq!(part.model, Some("code_edit_decider".to_string()));
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
                requested_recipe: None,
                recipe: "edit".to_string(),
                profile: "examples/code-agent/edit.air-profile.yaml".to_string(),
                pack: None,
                input: json!({"task": "old"}),
                completed: true,
                trace_files: Vec::new(),
                summary: CodeSessionTurnSummary::default(),
                parts: Vec::new(),
                patch_sets: Vec::new(),
                recovery: None,
                outputs: json!({"summary": "older turn"}),
            },
            CodeSessionTurn {
                id: "turn-000002".to_string(),
                time: CodeSessionTurnTime {
                    created: 2,
                    updated: 2,
                },
                task: "new".to_string(),
                requested_recipe: None,
                recipe: "edit".to_string(),
                profile: "examples/code-agent/edit.air-profile.yaml".to_string(),
                pack: None,
                input: json!({"task": "new"}),
                completed: false,
                trace_files: Vec::new(),
                summary: CodeSessionTurnSummary::default(),
                parts: Vec::new(),
                patch_sets: Vec::new(),
                recovery: None,
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
        let pack = load_code_agent_pack(None).unwrap();
        state.append_turn(CodeSessionTurn {
            id: code_session_turn_id(1),
            time: CodeSessionTurnTime {
                created: 42,
                updated: 42,
            },
            task: "fix it".to_string(),
            requested_recipe: Some("edit".to_string()),
            recipe: "edit".to_string(),
            profile: "examples/code-agent/edit.air-profile.yaml".to_string(),
            pack: Some(
                code_session_turn_pack(
                    &pack,
                    CodeRecipe::Edit,
                    Path::new("examples/code-agent/edit.air-profile.yaml"),
                )
                .unwrap(),
            ),
            input: json!({"task": "fix it"}),
            completed: false,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({"edit": {"final_success": false}}),
        });

        state.write(&path).unwrap();
        let roundtrip = CodeSessionState::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(roundtrip.version, 1);
        assert_eq!(roundtrip.turns.len(), 1);
        assert_eq!(roundtrip.turns[0].id, "turn-000001");
        assert_eq!(roundtrip.turns[0].time.created, 42);
        assert_eq!(roundtrip.turns[0].time.updated, 42);
        assert_eq!(roundtrip.turns[0].recipe, "edit");
        assert_eq!(
            roundtrip.turns[0].requested_recipe,
            Some("edit".to_string())
        );
        let pack = roundtrip.turns[0].pack.as_ref().unwrap();
        assert_eq!(pack.path, crate::code_pack::CODE_AGENT_PACK_PATH);
        assert_eq!(pack.recipe, "edit");
        assert_eq!(
            pack.default_profile,
            "examples/code-agent/edit.air-profile.yaml"
        );
        assert!(!pack.profile_override);
        assert_eq!(
            serde_json::to_value(&pack.completion).unwrap()["any"][0]["equals"]["path"],
            Value::String("/edit/final_success".to_string())
        );
        assert!(!roundtrip.turns[0].completed);
    }

    #[test]
    fn session_pack_marks_profile_overrides() {
        let pack = code_session_turn_pack(
            &load_code_agent_pack(None).unwrap(),
            CodeRecipe::Edit,
            Path::new("examples/code-agent/custom-edit.air-profile.yaml"),
        )
        .unwrap();

        assert_eq!(pack.recipe, "edit");
        assert_eq!(
            pack.default_profile,
            "examples/code-agent/edit.air-profile.yaml"
        );
        assert!(pack.profile_override);
        assert!(pack.completion.is_some());
    }

    #[test]
    fn session_turn_index_accepts_id_or_number() {
        let mut state = CodeSessionState::default();
        state.append_turn(CodeSessionTurn {
            id: "turn-000001".to_string(),
            time: CodeSessionTurnTime::default(),
            task: "one".to_string(),
            requested_recipe: None,
            recipe: "explore".to_string(),
            profile: "profile".to_string(),
            pack: None,
            input: json!({}),
            completed: true,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({}),
        });
        state.append_turn(CodeSessionTurn {
            id: "turn-000002".to_string(),
            time: CodeSessionTurnTime::default(),
            task: "two".to_string(),
            requested_recipe: None,
            recipe: "review".to_string(),
            profile: "profile".to_string(),
            pack: None,
            input: json!({}),
            completed: true,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({}),
        });

        assert_eq!(code_session_turn_index(&state, "turn-000002").unwrap(), 1);
        assert_eq!(code_session_turn_index(&state, "1").unwrap(), 0);
        assert!(code_session_turn_index(&state, "3").is_err());
    }

    #[test]
    fn session_workspace_revert_reports_empty_patch_sets_without_mutation() {
        let mut state = CodeSessionState::default();
        state.append_turn(CodeSessionTurn {
            id: "turn-000001".to_string(),
            time: CodeSessionTurnTime::default(),
            task: "inspect".to_string(),
            requested_recipe: None,
            recipe: "explore".to_string(),
            profile: "profile".to_string(),
            pack: None,
            input: json!({}),
            completed: true,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({}),
        });

        let summary = code_session_workspace_revert(&state, "turn-000001", false).unwrap();

        assert_eq!(summary.turn_id, "turn-000001");
        assert!(summary.success);
        assert!(!summary.applied);
        assert_eq!(summary.patch_set_count, 0);
        assert!(summary.results.is_empty());
    }
}
