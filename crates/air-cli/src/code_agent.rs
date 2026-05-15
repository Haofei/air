use crate::code_budget::{code_budget_limit_status, code_budget_limit_violation, CodeBudgetLimits};
#[cfg(test)]
use crate::code_input::build_input;
use crate::code_input::{
    build_input_with_pack, default_profile, path_ref_to_input_string, CodeInputOptions,
};
use crate::code_loop::{code_outputs_complete, iteration_path, run_code_loop, CodeLoopOptions};
use crate::code_pack::{load_code_agent_pack, CodeAgentPackContext};
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

    if task.trim().is_empty() {
        bail!("air code task must not be empty");
    }
    let profile = match profile {
        Some(profile) => profile,
        None => default_profile(&pack)?,
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
            profile: path_ref_to_input_string(&profile),
            pack: code_session_turn_pack(&pack, &profile),
            input: Value::Object(session_input),
            completed: code_outputs_complete(&pack, &outputs)?,
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
    active_profile: &Path,
) -> Option<CodeSessionTurnPack> {
    let default_profile = path_ref_to_input_string(&pack.default_profile());
    let active_profile = path_ref_to_input_string(active_profile);
    Some(CodeSessionTurnPack {
        path: path_ref_to_input_string(&pack.path),
        profile_override: active_profile != default_profile,
        default_profile,
        intent: pack.pack.intent.clone(),
        completion: pack.pack.completion.clone(),
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
    pack: &'a CodeAgentPackContext,
    profile: &'a Path,
    input: &'a Map<String, Value>,
    loop_enabled: bool,
    max_iterations: usize,
    budget_limits: CodeBudgetLimits,
}

fn print_explain(options: CodePrintExplainOptions<'_>) -> Result<()> {
    let CodePrintExplainOptions {
        pack,
        profile,
        input,
        loop_enabled,
        max_iterations,
        budget_limits,
    } = options;
    let metadata = explain_metadata_for_profile(profile)?;
    let pack_default_profile = path_ref_to_input_string(&pack.default_profile());
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
            "intent": pack.pack.intent,
            "completion": pack.pack.completion,
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
    use serde_json::Value;

    fn code_edit_loop_module() -> serde_yaml::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn code_agent_pack_declares_minimal_edit_profile() {
        let pack = load_code_agent_pack(None).unwrap();
        assert_eq!(
            pack.default_profile(),
            PathBuf::from("examples/code-agent/edit.air-profile.yaml")
        );
    }

    #[test]
    fn code_input_is_just_the_task() {
        let input = build_input(CodeInputOptions {
            task: "refactor a helper".to_string(),
        })
        .unwrap();

        assert_eq!(input.len(), 1);
        assert_eq!(
            input["task"],
            Value::String("refactor a helper".to_string())
        );
    }

    #[test]
    fn edit_loop_uses_minimal_rule_set() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|rule| rule["id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            rules,
            vec![
                "init",
                "summarize-at-step-limit",
                "choose",
                "act",
                "edit-applied",
                "bash-passed-after-patch",
                "bash-passed-before-patch",
                "bash-failed",
                "complete-needs-verification",
                "complete-verified",
                "continue-after-act",
                "summarize",
                "done",
            ]
        );
    }

    #[test]
    fn edit_loop_exposes_only_minimal_tools() {
        let module = code_edit_loop_module();
        let tools = module["tools"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            tools,
            vec!["glob", "read", "grep", "lsp", "edit", "bash", "todowrite"]
        );
    }

    #[test]
    fn choose_request_stays_small_and_opencode_shaped() {
        let module = code_edit_loop_module();
        let choose = module["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        let input = &choose["actions"][0]["input"]["object"];
        let keys = input.as_mapping().unwrap().keys().collect::<Vec<_>>();
        assert_eq!(keys.len(), 3);
        assert!(input.get("task").is_some());
        assert!(input.get("observations").is_some());
        assert!(input.get("allowed_tools").is_some());
        assert!(input.get("tool_schemas").is_none());
    }
}
