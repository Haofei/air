use crate::explain::build_plan_explanation;
use crate::planner::module_base_dir_for_store_path;
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::run_plan::{run_plan, run_plan_capture, write_trace, RunPlanOptions};
use crate::tools::ToolProviderChoice;
use air_runtime::{read_trace_jsonl, ToolProvider, TraceEvent, TraceStatus};
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    /// Behavior-preserving refactor loop: explore, patch intentionally, and retest.
    Refactor,
    /// Open-ended bounded refactor loop: explore, select one target/test, patch, and retest.
    OpenRefactor,
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
    pub(crate) force_patch: bool,
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

#[derive(Clone, Copy, Debug, Default)]
struct CodeBudgetLimits {
    max_estimated_model_calls: Option<usize>,
    max_estimated_tool_calls: Option<usize>,
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
        force_patch,
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
        max_estimated_model_calls,
        max_estimated_tool_calls,
        tool_config,
    } = options;
    let budget_limits = CodeBudgetLimits {
        max_estimated_model_calls,
        max_estimated_tool_calls,
    };

    let requested_recipe = recipe;
    let recipe = resolve_recipe(
        &task,
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
        force_patch,
    })?;
    let mut session_state = match session.as_ref() {
        Some(path) => Some(CodeSessionState::read(path)?),
        None => None,
    };
    if let Some(state) = session_state.as_ref() {
        input = if execute_plan && recipe == CodeRecipe::Plan {
            code_project_recovery_turn_input(&input, state)
                .unwrap_or_else(|| code_session_turn_input(&input, &state.turns))
        } else {
            code_session_turn_input(&input, &state.turns)
        };
    }

    if explain {
        print_explain(CodePrintExplainOptions {
            requested_recipe,
            resolved_recipe: recipe,
            profile: &profile,
            input: &input,
            loop_enabled,
            execute_plan,
            max_iterations,
            budget_limits,
        })?;
        return Ok(());
    }

    if !execute_plan {
        enforce_code_profile_budget_limits(
            &profile,
            if loop_enabled { max_iterations } else { 1 },
            budget_limits,
        )?;
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
    let mut outputs = if execute_plan {
        if recipe != CodeRecipe::Plan {
            bail!("air code --execute-plan requires the resolved recipe to be plan");
        }
        let resume = session_state.as_ref().and_then(code_session_project_resume);
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
            budget_limits,
            resume,
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
        let turn_id = code_session_turn_id(turn_number);
        let recovery = code_session_recovery_for_outputs(
            recipe,
            execute_plan,
            path,
            turn_number,
            &turn_id,
            &outputs,
        );
        if let Some((recovery, _)) = recovery.as_ref() {
            code_session_insert_recovery(&mut outputs, recovery)?;
        }
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
            patch_sets,
            recovery: recovery.as_ref().map(|(recovery, _)| recovery.clone()),
            outputs: outputs.clone(),
        });
        if let Some((_, fork_path)) = recovery {
            state.write(&fork_path)?;
        }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    patch_sets: Vec<CodeSessionPatchSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery: Option<CodeSessionRecovery>,
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CodeSessionPatchSet {
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_clean_before: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_clean_after: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    preexisting_changed_files: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changed_files: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workspace_changed_files: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diff_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diff_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifact_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CodeSessionRecovery {
    reason: String,
    status: String,
    fork: String,
    turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failed_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failed_execution: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    remaining_task_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workspace_revert_argv: Vec<String>,
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

fn code_session_turn_index(state: &CodeSessionState, target: &str) -> Result<usize> {
    if let Some(index) = state.turns.iter().position(|turn| turn.id == target) {
        return Ok(index);
    }
    if let Ok(number) = target.parse::<usize>() {
        if number > 0 && number <= state.turns.len() {
            return Ok(number - 1);
        }
    }
    bail!("AIR code session does not contain turn {target:?}");
}

#[derive(Clone, Debug, Serialize)]
struct CodeSessionWorkspaceRevertSummary {
    turn_id: String,
    applied: bool,
    success: bool,
    patch_set_count: usize,
    results: Vec<CodeSessionWorkspaceRevertResult>,
}

#[derive(Clone, Debug, Serialize)]
struct CodeSessionWorkspaceRevertResult {
    source: String,
    repo: Option<String>,
    checked: bool,
    applied: bool,
    success: bool,
    status: Option<i32>,
    log: String,
}

fn code_session_workspace_revert(
    state: &CodeSessionState,
    target: &str,
    apply_workspace: bool,
) -> Result<CodeSessionWorkspaceRevertSummary> {
    let index = code_session_turn_index(state, target)?;
    let turn = &state.turns[index];
    let mut patch_sets = turn.patch_sets.clone();
    patch_sets.reverse();
    let mut results = Vec::new();
    let mut success = true;

    for patch_set in &patch_sets {
        let Some(diff) = patch_set
            .diff
            .as_deref()
            .filter(|diff| !diff.trim().is_empty())
        else {
            continue;
        };
        let result = run_git_apply_reverse_check(patch_set, diff)?;
        success &= result.success;
        results.push(result);
    }

    if apply_workspace && success {
        let mut apply_results = Vec::new();
        for patch_set in &patch_sets {
            let Some(diff) = patch_set
                .diff
                .as_deref()
                .filter(|diff| !diff.trim().is_empty())
            else {
                continue;
            };
            let result = run_git_apply_reverse(patch_set, diff, true)?;
            success &= result.success;
            apply_results.push(result);
        }
        results.extend(apply_results);
    }

    Ok(CodeSessionWorkspaceRevertSummary {
        turn_id: turn.id.clone(),
        applied: apply_workspace && success,
        success,
        patch_set_count: patch_sets.len(),
        results,
    })
}

fn run_git_apply_reverse_check(
    patch_set: &CodeSessionPatchSet,
    diff: &str,
) -> Result<CodeSessionWorkspaceRevertResult> {
    run_git_apply_reverse(patch_set, diff, false)
}

fn run_git_apply_reverse(
    patch_set: &CodeSessionPatchSet,
    diff: &str,
    apply: bool,
) -> Result<CodeSessionWorkspaceRevertResult> {
    let repo = patch_set.repo.as_deref().unwrap_or(".");
    let mut command = Command::new("git");
    command.current_dir(repo);
    command.arg("apply").arg("--reverse");
    if !apply {
        command.arg("--check");
    }
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn git apply in {repo}"))?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .context("failed to open git apply stdin")?;
        stdin
            .write_all(diff.as_bytes())
            .context("failed to write reverse patch to git apply")?;
    }
    let output = child
        .wait_with_output()
        .context("failed to wait for git apply")?;
    let mut log = String::new();
    log.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        if !log.is_empty() {
            log.push('\n');
        }
        log.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    Ok(CodeSessionWorkspaceRevertResult {
        source: patch_set.source.clone(),
        repo: patch_set.repo.clone(),
        checked: !apply,
        applied: apply && output.status.success(),
        success: output.status.success(),
        status: output.status.code(),
        log,
    })
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

fn default_session_recovery_fork_path(session_path: &Path, turn_number: usize) -> PathBuf {
    let parent = session_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = session_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("air-code-session");
    parent
        .join(format!("{stem}.forks"))
        .join(format!("turn{turn_number}.failed.json"))
}

fn code_session_recovery_for_outputs(
    recipe: CodeRecipe,
    execute_plan: bool,
    session_path: &Path,
    turn_number: usize,
    turn_id: &str,
    outputs: &Value,
) -> Option<(CodeSessionRecovery, PathBuf)> {
    if recipe != CodeRecipe::Plan || !execute_plan {
        return None;
    }
    let project = outputs.get("project")?;
    let status = project.get("status").and_then(Value::as_str)?;
    if status != "stopped" {
        return None;
    }

    let fork_path = default_session_recovery_fork_path(session_path, turn_number);
    let fork = path_ref_to_input_string(&fork_path);
    let failed_execution = project
        .get("executions")
        .and_then(Value::as_array)
        .and_then(|executions| executions.last())
        .filter(|execution| {
            !execution
                .get("completed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
    let failed_task_id = failed_execution
        .and_then(|execution| execution.get("task_id").and_then(Value::as_str))
        .map(str::to_string);
    let failed_execution = failed_execution.map(code_project_failed_execution_summary);
    let remaining_task_ids = project
        .get("remaining_task_ids")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let recovery = CodeSessionRecovery {
        reason: "project execution stopped before completion".to_string(),
        status: status.to_string(),
        fork: fork.clone(),
        turn_id: turn_id.to_string(),
        failed_task_id,
        failed_execution,
        remaining_task_ids,
        workspace_revert_argv: vec![
            "air".to_string(),
            "code-session".to_string(),
            fork,
            "--revert-workspace-turn".to_string(),
            turn_id.to_string(),
            "--apply-workspace".to_string(),
        ],
    };
    Some((recovery, fork_path))
}

fn code_project_failed_execution_summary(execution: &Value) -> Value {
    let mut summary = Map::new();
    for key in [
        "task_id",
        "title",
        "recipe",
        "depends_on",
        "completed",
        "error",
    ] {
        if let Some(value) = execution.get(key) {
            summary.insert(key.to_string(), value.clone());
        }
    }
    if let Some(acceptance) = execution.get("acceptance") {
        summary.insert("acceptance".to_string(), acceptance.clone());
    }
    summary.insert(
        "output_keys".to_string(),
        Value::Array(
            execution
                .get("outputs")
                .and_then(Value::as_object)
                .map(|outputs| {
                    outputs
                        .keys()
                        .map(|key| Value::String(key.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        ),
    );
    Value::Object(summary)
}

fn code_session_insert_recovery(outputs: &mut Value, recovery: &CodeSessionRecovery) -> Result<()> {
    let object = outputs
        .as_object_mut()
        .context("AIR code session recovery requires object outputs")?;
    object.insert(
        "recovery".to_string(),
        serde_json::to_value(recovery).context("failed to serialize code session recovery")?,
    );
    if let Some(project) = object.get_mut("project").and_then(Value::as_object_mut) {
        let artifacts = project
            .entry("artifacts")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(artifacts) = artifacts.as_array_mut() {
            let mut seen = artifacts
                .iter()
                .filter_map(|artifact| artifact.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<HashSet<_>>();
            push_project_artifact(
                artifacts,
                &mut seen,
                json!({
                    "id": format!("recovery-fork:{}", recovery.turn_id),
                    "kind": "recovery_fork",
                    "path": recovery.fork,
                    "turn_id": recovery.turn_id,
                    "failed_task_id": recovery.failed_task_id,
                }),
            );
        }
    }
    Ok(())
}

fn code_session_project_resume(state: &CodeSessionState) -> Option<CodeProjectResume> {
    let turn = code_session_latest_project_turn(state, "max_tasks_exhausted")?;
    let project_plan = turn.outputs.get("project_plan")?.clone();
    let executions = turn
        .outputs
        .pointer("/project/executions")
        .and_then(Value::as_array)?
        .iter()
        .filter(|execution| {
            execution
                .get("completed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    if executions.is_empty() {
        return None;
    }
    Some(CodeProjectResume {
        turn_id: turn.id.clone(),
        project_plan,
        executions,
    })
}

fn code_project_recovery_turn_input(
    base_input: &Map<String, Value>,
    state: &CodeSessionState,
) -> Option<Map<String, Value>> {
    let turn = code_session_latest_project_turn(state, "stopped")?;
    let task = base_input.get("task").and_then(Value::as_str)?;
    let mut input = code_session_turn_input(base_input, &state.turns);
    let recovery = turn.outputs.get("recovery").or_else(|| {
        turn.recovery
            .as_ref()
            .and_then(|_| turn.outputs.get("recovery"))
    });
    let failed_execution = recovery
        .and_then(|recovery| recovery.get("failed_execution"))
        .or_else(|| {
            turn.outputs
                .pointer("/project/executions")
                .and_then(Value::as_array)
                .and_then(|executions| executions.last())
        });
    let project = turn.outputs.get("project").cloned().unwrap_or(Value::Null);
    let recovery_context = json!({
        "failed_turn_id": turn.id,
        "project_status": turn.outputs.pointer("/project/status").and_then(Value::as_str),
        "failed_task_id": recovery.and_then(|recovery| recovery.get("failed_task_id")).cloned(),
        "failed_execution": failed_execution.cloned(),
        "remaining_task_ids": turn.outputs.pointer("/project/remaining_task_ids").cloned(),
        "recovery_fork": recovery.and_then(|recovery| recovery.get("fork")).cloned(),
        "workspace_revert_argv": recovery.and_then(|recovery| recovery.get("workspace_revert_argv")).cloned(),
        "project": project,
    });
    input.insert(
        "task".to_string(),
        Value::String(format!(
            "{task}\n\nAIR project recovery context from previous failed execution:\n{}",
            serde_json::to_string_pretty(&recovery_context).unwrap_or_default()
        )),
    );
    Some(input)
}

fn code_session_latest_project_turn<'a>(
    state: &'a CodeSessionState,
    expected_status: &str,
) -> Option<&'a CodeSessionTurn> {
    let turn = state.turns.iter().rev().find(|turn| {
        turn.recipe == "plan"
            && turn
                .outputs
                .pointer("/project/status")
                .and_then(Value::as_str)
                .is_some()
    })?;
    if turn
        .outputs
        .pointer("/project/status")
        .and_then(Value::as_str)
        == Some(expected_status)
    {
        Some(turn)
    } else {
        None
    }
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
            "model_call_start" | "tool_call_start" | "approval_start"
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

struct CodePrintExplainOptions<'a> {
    requested_recipe: CodeRecipe,
    resolved_recipe: CodeRecipe,
    profile: &'a Path,
    input: &'a Map<String, Value>,
    loop_enabled: bool,
    execute_plan: bool,
    max_iterations: usize,
    budget_limits: CodeBudgetLimits,
}

fn print_explain(options: CodePrintExplainOptions<'_>) -> Result<()> {
    let CodePrintExplainOptions {
        requested_recipe,
        resolved_recipe,
        profile,
        input,
        loop_enabled,
        execute_plan,
        max_iterations,
        budget_limits,
    } = options;
    let metadata = explain_metadata_for_profile(profile)?;
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
                "max_estimated_model_calls": total_model_calls,
                "max_estimated_tool_calls": total_tool_calls
            }
        },
        "budget_limit": budget_limit,
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

fn code_budget_limit_status(
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
    budget_limits: CodeBudgetLimits,
) -> Value {
    if let Some(Value::Object(mut violation)) = code_budget_limit_violation(
        max_estimated_model_calls,
        max_estimated_tool_calls,
        budget_limits,
    ) {
        violation.insert("exceeded".to_string(), Value::Bool(true));
        return Value::Object(violation);
    }
    json!({
        "max_estimated_model_calls": budget_limits.max_estimated_model_calls,
        "max_estimated_tool_calls": budget_limits.max_estimated_tool_calls,
        "attempted_model_calls": max_estimated_model_calls,
        "attempted_tool_calls": max_estimated_tool_calls,
        "model_exceeded": false,
        "tool_exceeded": false,
        "exceeded": false,
    })
}

fn code_budget_limit_violation(
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
    budget_limits: CodeBudgetLimits,
) -> Option<Value> {
    let model_exceeded = budget_limits
        .max_estimated_model_calls
        .map(|limit| max_estimated_model_calls > limit)
        .unwrap_or(false);
    let tool_exceeded = budget_limits
        .max_estimated_tool_calls
        .map(|limit| max_estimated_tool_calls > limit)
        .unwrap_or(false);
    if !model_exceeded && !tool_exceeded {
        return None;
    }
    Some(json!({
        "max_estimated_model_calls": budget_limits.max_estimated_model_calls,
        "max_estimated_tool_calls": budget_limits.max_estimated_tool_calls,
        "attempted_model_calls": max_estimated_model_calls,
        "attempted_tool_calls": max_estimated_tool_calls,
        "model_exceeded": model_exceeded,
        "tool_exceeded": tool_exceeded,
    }))
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
    budget_limits: CodeBudgetLimits,
    resume: Option<CodeProjectResume>,
}

#[derive(Clone, Debug)]
struct CodeProjectResume {
    turn_id: String,
    project_plan: Value,
    executions: Vec<Value>,
}

fn run_code_project(options: CodeProjectOptions) -> Result<Value> {
    if options.max_tasks == 0 {
        bail!("air code --execute-plan requires --max-iterations to be greater than 0");
    }

    let mut trace_files = Vec::new();
    let mut resumed_from_turn = None;
    let (project_plan, mut executions, mut prior_executions) =
        if let Some(resume) = options.resume.clone() {
            if options.log {
                eprintln!(
                    "[air-code-project] step=resume from_turn={}",
                    resume.turn_id
                );
            }
            resumed_from_turn = Some(resume.turn_id);
            (
                resume.project_plan,
                resume.executions.clone(),
                resume.executions,
            )
        } else {
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
            (project_plan, Vec::new(), Vec::new())
        };
    let tasks = project_plan
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let scheduled_tasks = match code_project_task_schedule(&tasks) {
        Ok(schedule) => schedule,
        Err(error) => {
            return Ok(json!({
                "project_plan": project_plan,
                "project": {
                    "status": "stopped",
                    "completed": false,
                    "max_tasks": options.max_tasks,
                    "executed_tasks": 0,
                    "remaining_task_ids": code_project_task_ids(&tasks),
                    "budget": {
                        "task_count": 0,
                        "max_estimated_model_calls": 0,
                        "max_estimated_tool_calls": 0,
                        "tasks": [],
                    },
                    "remaining_budget": {
                        "task_count": 0,
                        "max_estimated_model_calls": 0,
                        "max_estimated_tool_calls": 0,
                        "tasks": [],
                    },
                    "budget_limit": code_budget_limit_status(
                        0,
                        0,
                        options.budget_limits
                    ),
                    "memory": code_project_memory(&[]),
                    "artifacts": code_project_artifacts(&trace_files, &[]),
                    "acceptance": [],
                    "executions": [{
                        "completed": false,
                        "error": error.to_string(),
                    }],
                },
                "trace_files": trace_files,
            }));
        }
    };
    let scheduled_task_ids = scheduled_tasks
        .iter()
        .map(|task| Value::String(task.id.clone()))
        .collect::<Vec<_>>();
    let project_budget = code_project_budget_summary(&scheduled_tasks);
    let task_budget_by_id = project_budget
        .get("tasks")
        .and_then(Value::as_array)
        .map(|tasks| {
            tasks
                .iter()
                .filter_map(|budget| {
                    budget
                        .get("task_id")
                        .and_then(Value::as_str)
                        .map(|id| (id.to_string(), budget.clone()))
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    let mut acceptance_events = Vec::new();
    let mut acceptance_step = 0u32;
    let mut acceptance_tools: Option<ToolProviderChoice> = None;
    let mut completed = prior_executions_completed(&prior_executions);
    let mut stopped = false;
    let starting_execution_count = executions.len();
    let completed_task_ids = prior_executions
        .iter()
        .filter(|execution| {
            execution
                .get("completed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|execution| execution.get("task_id").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    let pending_tasks = scheduled_tasks
        .iter()
        .filter(|task| !completed_task_ids.contains(task.id.as_str()))
        .collect::<Vec<_>>();
    let pending_task_values = pending_tasks
        .iter()
        .map(|task| (*task).clone())
        .collect::<Vec<_>>();
    let remaining_budget = code_project_budget_summary(&pending_task_values);
    let (remaining_model_calls, remaining_tool_calls) = code_budget_value_counts(&remaining_budget);
    let budget_limit = code_budget_limit_status(
        remaining_model_calls,
        remaining_tool_calls,
        options.budget_limits,
    );
    if budget_limit
        .get("exceeded")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let remaining_task_ids = pending_tasks
            .iter()
            .map(|task| Value::String(task.id.clone()))
            .collect::<Vec<_>>();
        return Ok(json!({
            "project_plan": project_plan,
            "project": {
                "status": "budget_exceeded",
                "completed": false,
                "max_tasks": options.max_tasks,
                "executed_tasks": executions.len(),
                "executed_this_run": 0,
                "resumed_from_turn": resumed_from_turn,
                "scheduled_task_ids": scheduled_task_ids,
                "remaining_task_ids": remaining_task_ids,
                "budget": project_budget,
                "remaining_budget": remaining_budget,
                "budget_limit": budget_limit,
                "memory": code_project_memory(&executions),
                "artifacts": code_project_artifacts(&trace_files, &executions),
                "acceptance": [],
                "executions": executions,
            },
            "trace_files": trace_files,
        }));
    }
    for (index, task) in pending_tasks.iter().take(options.max_tasks).enumerate() {
        let task_number = starting_execution_count + index + 1;
        let task_id = task.id.clone();
        let title = task
            .definition
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let recipe = match task_recipe(&task.definition) {
            Ok(recipe) => recipe,
            Err(error) => {
                completed = false;
                stopped = true;
                executions.push(json!({
                    "task_id": task_id,
                    "title": title,
                    "completed": false,
                    "budget": task_budget_by_id.get(&task_id).cloned().unwrap_or(Value::Null),
                    "error": error.to_string(),
                }));
                break;
            }
        };
        let Some(input) = task.definition.get("input").and_then(Value::as_object) else {
            completed = false;
            stopped = true;
            executions.push(json!({
                "task_id": task_id,
                "title": title,
                "recipe": recipe_name(recipe),
                "budget": task_budget_by_id.get(&task_id).cloned().unwrap_or(Value::Null),
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
                let acceptance = run_acceptance_checks(
                    &mut acceptance_tools,
                    &options,
                    task.definition.get("acceptance"),
                    &format!("task:{task_id}"),
                    &mut acceptance_events,
                    &mut acceptance_step,
                )?;
                let acceptance_completed = acceptance_checks_completed(&acceptance);
                let task_completed =
                    code_outputs_complete(recipe, &outputs) && acceptance_completed;
                completed &= task_completed;
                let execution = json!({
                    "task_id": task_id,
                    "title": title,
                    "recipe": recipe_name(recipe),
                    "depends_on": task.depends_on.clone(),
                    "budget": task_budget_by_id.get(&task_id).cloned().unwrap_or(Value::Null),
                    "completed": task_completed,
                    "acceptance": acceptance,
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
                    "depends_on": task.depends_on.clone(),
                    "budget": task_budget_by_id.get(&task_id).cloned().unwrap_or(Value::Null),
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
    let executed_this_run = executed.saturating_sub(starting_execution_count);
    let project_acceptance = if !stopped && executed == scheduled_tasks.len() {
        let acceptance = run_acceptance_checks(
            &mut acceptance_tools,
            &options,
            project_plan.get("acceptance"),
            "project",
            &mut acceptance_events,
            &mut acceptance_step,
        )?;
        completed &= acceptance_checks_completed(&acceptance);
        acceptance
    } else {
        Vec::new()
    };
    if !acceptance_events.is_empty() {
        if let Some(path) = options
            .trace_out
            .as_ref()
            .map(|path| labeled_trace_path(path, "acceptance"))
        {
            write_trace(
                &path,
                &acceptance_events,
                options.trace_redact || !options.trace_raw,
            )?;
            trace_files.push(path_ref_to_input_string(&path));
        }
    }
    let executed_task_ids = executions
        .iter()
        .filter_map(|execution| execution.get("task_id").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    let remaining_task_ids = scheduled_tasks
        .iter()
        .filter(|task| !executed_task_ids.contains(task.id.as_str()))
        .map(|task| Value::String(task.id.clone()))
        .collect::<Vec<_>>();
    let status = if stopped {
        "stopped"
    } else if pending_tasks.len() > executed_this_run {
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
            "executed_this_run": executed_this_run,
            "resumed_from_turn": resumed_from_turn,
            "scheduled_task_ids": scheduled_task_ids,
            "remaining_task_ids": remaining_task_ids,
            "budget": project_budget,
            "remaining_budget": remaining_budget,
            "budget_limit": budget_limit,
            "memory": code_project_memory(&executions),
            "artifacts": code_project_artifacts(&trace_files, &executions),
            "acceptance": project_acceptance,
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
        "refactor" => Ok(CodeRecipe::Refactor),
        "open-refactor" => Ok(CodeRecipe::OpenRefactor),
        "build" => Ok(CodeRecipe::Build),
        other => bail!("unsupported planned task recipe {other:?}"),
    }
}

#[derive(Clone, Debug)]
struct CodeProjectScheduledTask {
    id: String,
    depends_on: Vec<String>,
    definition: Value,
}

fn code_project_task_ids(tasks: &[Value]) -> Vec<Value> {
    tasks
        .iter()
        .filter_map(|task| task.get("id").and_then(Value::as_str))
        .map(|id| Value::String(id.to_string()))
        .collect()
}

fn code_project_task_schedule(tasks: &[Value]) -> Result<Vec<CodeProjectScheduledTask>> {
    let mut parsed = Vec::new();
    let mut id_to_index = HashMap::new();
    for (index, task) in tasks.iter().enumerate() {
        let id = task
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .with_context(|| format!("planned task at index {index} is missing non-empty id"))?
            .to_string();
        if id_to_index.insert(id.clone(), index).is_some() {
            bail!("planned task id {id:?} is duplicated");
        }
        let depends_on = task
            .get("depends_on")
            .and_then(Value::as_array)
            .map(|deps| {
                deps.iter()
                    .enumerate()
                    .map(|(dep_index, dep)| {
                        dep.as_str()
                            .filter(|dep| !dep.trim().is_empty())
                            .map(str::to_string)
                            .with_context(|| {
                                format!(
                                    "planned task {id:?} depends_on[{dep_index}] is not a non-empty string"
                                )
                            })
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        parsed.push(CodeProjectScheduledTask {
            id,
            depends_on,
            definition: task.clone(),
        });
    }

    for task in &parsed {
        for dependency in &task.depends_on {
            if !id_to_index.contains_key(dependency) {
                bail!(
                    "planned task {:?} depends on missing task {:?}",
                    task.id,
                    dependency
                );
            }
        }
    }

    let mut remaining = (0..parsed.len()).collect::<Vec<_>>();
    let mut scheduled = Vec::new();
    let mut completed_ids = HashSet::new();
    while !remaining.is_empty() {
        let Some(ready_position) = remaining.iter().position(|index| {
            parsed[*index]
                .depends_on
                .iter()
                .all(|dependency| completed_ids.contains(dependency))
        }) else {
            let blocked = remaining
                .iter()
                .map(|index| parsed[*index].id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("planned task dependency cycle blocks: {blocked}");
        };
        let index = remaining.remove(ready_position);
        let task = parsed[index].clone();
        completed_ids.insert(task.id.clone());
        scheduled.push(task);
    }

    Ok(scheduled)
}

fn code_project_budget_summary(tasks: &[CodeProjectScheduledTask]) -> Value {
    let task_budgets = tasks
        .iter()
        .map(code_project_task_budget)
        .collect::<Vec<_>>();
    let max_estimated_model_calls = task_budgets
        .iter()
        .filter_map(|budget| budget.get("max_estimated_model_calls"))
        .filter_map(Value::as_u64)
        .sum::<u64>();
    let max_estimated_tool_calls = task_budgets
        .iter()
        .filter_map(|budget| budget.get("max_estimated_tool_calls"))
        .filter_map(Value::as_u64)
        .sum::<u64>();

    json!({
        "task_count": tasks.len(),
        "max_estimated_model_calls": max_estimated_model_calls,
        "max_estimated_tool_calls": max_estimated_tool_calls,
        "tasks": task_budgets,
    })
}

fn code_budget_value_counts(budget: &Value) -> (usize, usize) {
    let model_calls = budget
        .get("max_estimated_model_calls")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    let tool_calls = budget
        .get("max_estimated_tool_calls")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    (model_calls, tool_calls)
}

fn code_project_task_budget(task: &CodeProjectScheduledTask) -> Value {
    let recipe = match task_recipe(&task.definition) {
        Ok(recipe) => recipe,
        Err(error) => {
            return json!({
                "task_id": task.id.clone(),
                "depends_on": task.depends_on.clone(),
                "error": error.to_string(),
            });
        }
    };
    let profile = default_profile(recipe);
    let metadata_profile = code_profile_metadata_path(&profile);
    match explain_metadata_for_profile(&metadata_profile) {
        Ok(metadata) => json!({
            "task_id": task.id.clone(),
            "recipe": recipe_name(recipe),
            "profile": path_ref_to_input_string(&profile),
            "plan": path_ref_to_input_string(&metadata.plan),
            "store": path_ref_to_input_string(&metadata.store),
            "depends_on": task.depends_on.clone(),
            "capabilities": metadata.capabilities,
            "read_only": metadata.read_only,
            "writes_workspace": metadata.writes_workspace,
            "max_estimated_model_calls": metadata.max_estimated_model_calls,
            "max_estimated_tool_calls": metadata.max_estimated_tool_calls,
        }),
        Err(error) => json!({
            "task_id": task.id.clone(),
            "recipe": recipe_name(recipe),
            "profile": path_ref_to_input_string(&profile),
            "depends_on": task.depends_on.clone(),
            "error": error.to_string(),
        }),
    }
}

fn code_profile_metadata_path(profile: &Path) -> PathBuf {
    if profile.is_absolute() || profile.exists() {
        return profile.to_path_buf();
    }
    let workspace_profile = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(profile);
    if workspace_profile.exists() {
        workspace_profile
    } else {
        profile.to_path_buf()
    }
}

fn prior_executions_completed(executions: &[Value]) -> bool {
    executions.iter().all(|execution| {
        execution
            .get("completed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

fn code_project_task_input(
    base_input: &Map<String, Value>,
    prior_executions: &[Value],
) -> Map<String, Value> {
    let mut input = normalized_code_project_task_paths(base_input);
    ensure_repair_task_defaults(&mut input);
    if prior_executions.is_empty() {
        return input;
    }
    let task = input
        .get("task")
        .and_then(Value::as_str)
        .unwrap_or("execute planned project task");
    let memory = truncate_for_context(
        &serde_json::to_string_pretty(&code_project_memory(prior_executions)).unwrap_or_default(),
        default_context_budget_chars(),
    );
    let available_artifacts = truncate_for_context(
        &serde_json::to_string_pretty(&code_project_artifacts(&[], prior_executions))
            .unwrap_or_default(),
        default_context_budget_chars(),
    );
    input.insert(
        "task".to_string(),
        Value::String(format!(
            "{task}\n\nAIR project memory from previous tasks:\n{memory}\n\nAvailable project artifacts from previous tasks:\n{available_artifacts}"
        )),
    );
    input
}

fn ensure_repair_task_defaults(input: &mut Map<String, Value>) {
    if input.contains_key("target_path") {
        if !input.contains_key("target_search_pattern") {
            let query = input
                .get("query")
                .or_else(|| input.get("task"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            input.insert(
                "target_search_pattern".to_string(),
                Value::String(code_search_pattern(query)),
            );
        }
        input
            .entry("force_patch".to_string())
            .or_insert(Value::Bool(false));
    }
}

fn normalized_code_project_task_paths(base_input: &Map<String, Value>) -> Map<String, Value> {
    let mut input = base_input.clone();
    for field in ["target_path", "target_file"] {
        normalize_file_target_field(&mut input, field);
    }
    input
}

fn normalize_file_target_field(input: &mut Map<String, Value>, field: &str) {
    let Some(path) = input.get(field).and_then(Value::as_str) else {
        return;
    };
    let Some(file) = preferred_file_for_target(Path::new(path)) else {
        return;
    };
    input.insert(field.to_string(), Value::String(path_to_input_string(file)));
}

fn preferred_file_for_target(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return None;
    }
    if !path.is_dir() {
        return None;
    }
    preferred_file_in_directory(path)
}

fn preferred_file_in_directory(path: &Path) -> Option<PathBuf> {
    for candidate in [
        "lib.rs",
        "main.rs",
        "mod.rs",
        "index.ts",
        "index.tsx",
        "index.js",
        "index.jsx",
        "index.html",
        "README.md",
        "Cargo.toml",
        "package.json",
    ] {
        let candidate = path.join(candidate);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut files = collect_candidate_files(path, 3, 64);
    files.sort();
    files.into_iter().next()
}

fn collect_candidate_files(path: &Path, max_depth: usize, max_files: usize) -> Vec<PathBuf> {
    if max_depth == 0 || max_files == 0 {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut entries = entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());

    let mut files = Vec::new();
    for entry in entries {
        let entry_path = entry.path();
        if ignored_code_agent_path(&entry_path) {
            continue;
        }
        if entry_path.is_file() && is_likely_code_agent_context_file(&entry_path) {
            files.push(entry_path);
            if files.len() >= max_files {
                break;
            }
        } else if entry_path.is_dir() {
            files.extend(collect_candidate_files(
                &entry_path,
                max_depth.saturating_sub(1),
                max_files.saturating_sub(files.len()),
            ));
            if files.len() >= max_files {
                break;
            }
        }
    }
    files
}

fn ignored_code_agent_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with('.')
                || matches!(
                    name,
                    "target" | "node_modules" | "dist" | "build" | "__pycache__"
                )
        })
}

fn is_likely_code_agent_context_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "rs" | "toml"
                    | "yaml"
                    | "yml"
                    | "json"
                    | "md"
                    | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "py"
                    | "go"
                    | "java"
                    | "html"
                    | "css"
            )
        })
}

fn code_project_memory(executions: &[Value]) -> Value {
    let tasks = executions
        .iter()
        .map(code_project_execution_memory)
        .collect::<Vec<_>>();
    let mut changed_files = Vec::new();
    let mut artifact_ids = Vec::new();
    let mut artifact_kinds = Vec::new();
    for task in &tasks {
        collect_string_array_field(task.get("changed_files"), &mut changed_files);
        collect_string_array_field(task.get("artifact_ids"), &mut artifact_ids);
        collect_string_array_field(task.get("artifact_kinds"), &mut artifact_kinds);
    }
    changed_files.sort();
    changed_files.dedup();
    artifact_ids.sort();
    artifact_ids.dedup();
    artifact_kinds.sort();
    artifact_kinds.dedup();

    json!({
        "task_count": executions.len(),
        "completed_task_count": executions
            .iter()
            .filter(|execution| execution.get("completed").and_then(Value::as_bool).unwrap_or(false))
            .count(),
        "failed_task_count": executions
            .iter()
            .filter(|execution| !execution.get("completed").and_then(Value::as_bool).unwrap_or(false))
            .count(),
        "changed_files": changed_files,
        "artifact_ids": artifact_ids,
        "artifact_kinds": artifact_kinds,
        "tasks": tasks,
    })
}

fn code_project_artifacts(trace_files: &[String], executions: &[Value]) -> Value {
    let mut artifacts = Vec::new();
    let mut seen = HashSet::new();
    for trace_file in trace_files {
        push_project_artifact(
            &mut artifacts,
            &mut seen,
            json!({
                "id": format!("trace:{trace_file}"),
                "kind": "trace_jsonl",
                "path": trace_file,
            }),
        );
    }
    for execution in executions {
        let task_id = execution
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if let Some(outputs) = execution.get("outputs") {
            collect_project_output_artifacts(
                outputs,
                task_id,
                "$.outputs",
                &mut artifacts,
                &mut seen,
            );
            for path in code_project_changed_files(outputs) {
                push_project_artifact(
                    &mut artifacts,
                    &mut seen,
                    json!({
                        "id": format!("changed-file:{path}"),
                        "kind": "changed_file",
                        "path": path,
                        "task_id": task_id,
                    }),
                );
            }
        }
    }
    Value::Array(artifacts)
}

fn collect_project_output_artifacts(
    value: &Value,
    task_id: &str,
    source: &str,
    artifacts: &mut Vec<Value>,
    seen: &mut HashSet<String>,
) {
    match value {
        Value::Object(object) => {
            if let Some(values) = object.get("artifacts").and_then(Value::as_array) {
                for (index, artifact) in values.iter().enumerate() {
                    if let Some(mut artifact) = normalize_project_artifact(artifact) {
                        if let Some(object) = artifact.as_object_mut() {
                            object
                                .insert("task_id".to_string(), Value::String(task_id.to_string()));
                            object.insert(
                                "source".to_string(),
                                Value::String(format!("{source}.artifacts[{index}]")),
                            );
                        }
                        push_project_artifact(artifacts, seen, artifact);
                    }
                }
            }
            for (key, value) in object {
                collect_project_output_artifacts(
                    value,
                    task_id,
                    &format!("{source}.{key}"),
                    artifacts,
                    seen,
                );
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_project_output_artifacts(
                    value,
                    task_id,
                    &format!("{source}[{index}]"),
                    artifacts,
                    seen,
                );
            }
        }
        _ => {}
    }
}

fn normalize_project_artifact(artifact: &Value) -> Option<Value> {
    let object = artifact.as_object()?;
    let id = object.get("id").and_then(Value::as_str)?;
    let mut normalized = object.clone();
    normalized
        .entry("kind".to_string())
        .or_insert_with(|| Value::String("artifact".to_string()));
    normalized.insert("id".to_string(), Value::String(id.to_string()));
    Some(Value::Object(normalized))
}

fn push_project_artifact(artifacts: &mut Vec<Value>, seen: &mut HashSet<String>, artifact: Value) {
    let Some(id) = artifact.get("id").and_then(Value::as_str) else {
        return;
    };
    if seen.insert(id.to_string()) {
        artifacts.push(artifact);
    }
}

fn code_project_execution_memory(execution: &Value) -> Value {
    let mut memory = Map::new();
    for key in [
        "task_id",
        "title",
        "recipe",
        "depends_on",
        "completed",
        "error",
    ] {
        if let Some(value) = execution.get(key) {
            memory.insert(key.to_string(), value.clone());
        }
    }
    if let Some(acceptance) = execution.get("acceptance").and_then(Value::as_array) {
        memory.insert(
            "acceptance".to_string(),
            Value::Array(
                acceptance
                    .iter()
                    .map(code_project_acceptance_memory)
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if let Some(outputs) = execution.get("outputs") {
        memory.insert(
            "output_keys".to_string(),
            Value::Array(
                value_keys(Some(outputs))
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        memory.insert("outputs".to_string(), code_project_outputs_memory(outputs));
        let changed_files = code_project_changed_files(outputs);
        if !changed_files.is_empty() {
            memory.insert(
                "changed_files".to_string(),
                Value::Array(changed_files.into_iter().map(Value::String).collect()),
            );
        }
        let artifact_ids = collect_artifact_field_values(outputs, "id");
        if !artifact_ids.is_empty() {
            memory.insert(
                "artifact_ids".to_string(),
                Value::Array(artifact_ids.into_iter().map(Value::String).collect()),
            );
        }
        let artifact_kinds = collect_artifact_field_values(outputs, "kind");
        if !artifact_kinds.is_empty() {
            memory.insert(
                "artifact_kinds".to_string(),
                Value::Array(artifact_kinds.into_iter().map(Value::String).collect()),
            );
        }
    }
    Value::Object(memory)
}

fn code_project_acceptance_memory(acceptance: &Value) -> Value {
    let mut memory = Map::new();
    for key in ["id", "scope", "command", "expected", "success", "error"] {
        if let Some(value) = acceptance.get(key) {
            memory.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(memory)
}

fn code_project_outputs_memory(outputs: &Value) -> Value {
    let mut memory = Map::new();
    if let Some(exploration) = outputs.get("exploration") {
        copy_selected_output_fields(
            &mut memory,
            "exploration",
            exploration,
            &[
                "summary",
                "findings",
                "relevant_files",
                "source_ids",
                "next_steps",
            ],
        );
    }
    if let Some(review) = outputs.get("review") {
        copy_selected_output_fields(
            &mut memory,
            "review",
            review,
            &[
                "summary",
                "findings",
                "search_quality",
                "source_ids",
                "recommended_actions",
            ],
        );
    }
    if let Some(repair) = outputs.get("repair") {
        copy_selected_output_fields(
            &mut memory,
            "repair",
            repair,
            &[
                "target_path",
                "initial_success",
                "final_success",
                "patch_applied",
                "changed_files",
                "workspace_changed_files",
                "preexisting_changed_files",
                "diagnostics",
                "summary",
            ],
        );
        if let Some(diff) = repair.get("workspace_diff") {
            copy_selected_output_fields(
                &mut memory,
                "repair_workspace_diff",
                diff,
                &["repo", "bytes", "truncated", "artifacts"],
            );
        }
    }
    if let Some(build) = outputs.get("build") {
        copy_selected_output_fields(
            &mut memory,
            "build",
            build,
            &[
                "path",
                "bytes",
                "test_success",
                "audit_success",
                "screenshots",
                "revised",
            ],
        );
    }
    if memory.is_empty() {
        collect_summary_fields(outputs, "$", &mut memory);
    }
    Value::Object(memory)
}

fn copy_selected_output_fields(
    memory: &mut Map<String, Value>,
    namespace: &str,
    value: &Value,
    fields: &[&str],
) {
    let mut object = Map::new();
    for field in fields {
        if let Some(value) = value.get(*field) {
            object.insert((*field).to_string(), value.clone());
        }
    }
    if !object.is_empty() {
        memory.insert(namespace.to_string(), Value::Object(object));
    }
}

fn collect_summary_fields(value: &Value, path: &str, output: &mut Map<String, Value>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let next = if path == "$" {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                if matches!(key.as_str(), "summary" | "reason" | "recommended_action") {
                    output.insert(next.clone(), value.clone());
                }
                collect_summary_fields(value, &next, output);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_summary_fields(value, &format!("{path}[{index}]"), output);
            }
        }
        _ => {}
    }
}

fn code_project_changed_files(outputs: &Value) -> Vec<String> {
    let mut files = Vec::new();
    collect_path_values(
        outputs
            .get("repair")
            .and_then(|repair| repair.get("changed_files")),
        &mut files,
    );
    collect_path_values(
        outputs
            .get("repair")
            .and_then(|repair| repair.get("workspace_changed_files")),
        &mut files,
    );
    collect_path_values(
        outputs
            .get("repair")
            .and_then(|repair| repair.get("workspace_diff"))
            .and_then(|diff| diff.get("artifacts")),
        &mut files,
    );
    files.sort();
    files.dedup();
    files
}

fn collect_path_values(value: Option<&Value>, output: &mut Vec<String>) {
    match value {
        Some(Value::Array(values)) => {
            for value in values {
                collect_path_values(Some(value), output);
            }
        }
        Some(Value::Object(object)) => {
            if let Some(path) = object.get("path").and_then(Value::as_str) {
                output.push(path.to_string());
            }
        }
        Some(Value::String(path)) => output.push(path.clone()),
        _ => {}
    }
}

fn collect_artifact_field_values(value: &Value, field: &str) -> Vec<String> {
    let mut values = Vec::new();
    collect_artifact_field_values_into(value, field, &mut values);
    values.sort();
    values.dedup();
    values
}

fn collect_artifact_field_values_into(value: &Value, field: &str, output: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(artifacts) = object.get("artifacts").and_then(Value::as_array) {
                for artifact in artifacts {
                    if let Some(value) = artifact.get(field).and_then(Value::as_str) {
                        output.push(value.to_string());
                    }
                }
            }
            for value in object.values() {
                collect_artifact_field_values_into(value, field, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_artifact_field_values_into(value, field, output);
            }
        }
        _ => {}
    }
}

fn collect_string_array_field(value: Option<&Value>, output: &mut Vec<String>) {
    let Some(Value::Array(values)) = value else {
        return;
    };
    output.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
}

fn run_acceptance_checks(
    tools: &mut Option<ToolProviderChoice>,
    options: &CodeProjectOptions,
    checks: Option<&Value>,
    scope: &str,
    events: &mut Vec<TraceEvent>,
    step: &mut u32,
) -> Result<Vec<Value>> {
    let checks = acceptance_checks(checks);
    if checks.is_empty() {
        return Ok(Vec::new());
    }
    let tool_config =
        resolve_code_project_tool_config(&options.plan_profile, options.tool_config.clone())?;
    let tools = tools.get_or_insert(ToolProviderChoice::from_config(tool_config, false)?);
    let mut results = Vec::new();
    for check in checks {
        *step += 1;
        let input = check.input.clone();
        let output = tools.call_tool("test.run", &input);
        match output {
            Ok(output) => {
                let success = output
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                events.push(TraceEvent {
                    agent: "air-code-project".to_string(),
                    step: *step,
                    rule: scope.to_string(),
                    action: "tool_call".to_string(),
                    input: Some(input),
                    output: Some(output.clone()),
                    meta: Some(json!({
                        "tool": "test.run",
                        "acceptance_id": check.id,
                        "scope": scope,
                    })),
                    status: if success {
                        TraceStatus::Ok
                    } else {
                        TraceStatus::Error
                    },
                    error: if success {
                        None
                    } else {
                        Some(format!("acceptance check {} failed", check.id))
                    },
                });
                results.push(json!({
                    "id": check.id,
                    "scope": scope,
                    "command": check.command,
                    "expected": check.expected,
                    "success": success,
                    "output": output,
                }));
            }
            Err(error) => {
                let error = error.to_string();
                events.push(TraceEvent {
                    agent: "air-code-project".to_string(),
                    step: *step,
                    rule: scope.to_string(),
                    action: "tool_call".to_string(),
                    input: Some(input),
                    output: None,
                    meta: Some(json!({
                        "tool": "test.run",
                        "acceptance_id": check.id,
                        "scope": scope,
                    })),
                    status: TraceStatus::Error,
                    error: Some(error.clone()),
                });
                results.push(json!({
                    "id": check.id,
                    "scope": scope,
                    "command": check.command,
                    "expected": check.expected,
                    "success": false,
                    "error": error,
                }));
            }
        }
    }
    Ok(results)
}

#[derive(Clone, Debug)]
struct AcceptanceCheck {
    id: String,
    command: String,
    expected: String,
    input: Value,
}

fn acceptance_checks(value: Option<&Value>) -> Vec<AcceptanceCheck> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Object(object) => {
                let command = object.get("command")?.as_str()?.to_string();
                let id = object
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(&command)
                    .to_string();
                let expected = object
                    .get("expected")
                    .and_then(Value::as_str)
                    .unwrap_or("command succeeds")
                    .to_string();
                let mut input = Map::new();
                input.insert("command".to_string(), Value::String(command.clone()));
                if let Some(Value::Object(parameters)) = object.get("parameters") {
                    for (key, value) in parameters {
                        input.insert(key.clone(), value.clone());
                    }
                }
                Some(AcceptanceCheck {
                    id,
                    command,
                    expected,
                    input: Value::Object(input),
                })
            }
            _ => None,
        })
        .collect()
}

fn acceptance_checks_completed(checks: &[Value]) -> bool {
    checks.iter().all(|check| {
        check
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

fn resolve_code_project_tool_config(
    profile: &PathBuf,
    explicit: Option<PathBuf>,
) -> Result<Option<PathBuf>> {
    if explicit.is_some() {
        return Ok(explicit);
    }
    let profile_config = read_run_plan_profile(profile)?;
    Ok(profile_config
        .tool_config
        .as_ref()
        .map(|path| resolve_profile_path(profile, path)))
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
        fields.push(format!(
            "files={}",
            code_session_join_limited(&turn.summary.files, 12)
        ));
    }
    if !turn.summary.artifact_kinds.is_empty() {
        fields.push(format!(
            "artifact_kinds={}",
            turn.summary.artifact_kinds.join(",")
        ));
    }
    if !turn.patch_sets.is_empty() {
        let changed_files = turn
            .patch_sets
            .iter()
            .flat_map(|patch_set| patch_set.changed_files.iter())
            .filter_map(|file| file.get("path").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>();
        fields.push(format!("patch_sets={}", turn.patch_sets.len()));
        if !changed_files.is_empty() {
            fields.push(format!(
                "patch_files={}",
                code_session_join_limited(&changed_files, 12)
            ));
        }
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

fn code_session_join_limited(values: &[String], limit: usize) -> String {
    if values.len() <= limit {
        return values.join(",");
    }
    let mut items = values.iter().take(limit).cloned().collect::<Vec<_>>();
    items.push(format!("+{} more", values.len() - limit));
    items.join(",")
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
        CodeRecipe::Repair | CodeRecipe::Refactor | CodeRecipe::OpenRefactor => outputs
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
    force_patch: bool,
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
        force_patch,
    } = options;

    let recipe = resolve_recipe(
        &task,
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
        CodeRecipe::Repair | CodeRecipe::Refactor => {
            let target = required_path(target, "--target", recipe)?;
            let test = required_string(test, "--test", recipe)?;
            let query = query.unwrap_or_else(|| task.clone());
            let target_search_pattern = code_search_pattern(&query);
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query));
            input.insert(
                "target_search_pattern".to_string(),
                Value::String(target_search_pattern),
            );
            input.insert(
                "target_path".to_string(),
                Value::String(path_to_input_string(target)),
            );
            input.insert("related_files".to_string(), path_array(related));
            input.insert("test_command".to_string(), Value::String(test));
            input.insert(
                "force_patch".to_string(),
                Value::Bool(force_patch || recipe == CodeRecipe::Refactor),
            );
            Ok(input)
        }
        CodeRecipe::OpenRefactor => {
            let query = query.unwrap_or_else(|| task.clone());
            let target_search_pattern = code_search_pattern(&query);
            let allowed_test_commands = match test {
                Some(test) => vec![test],
                None => vec![
                    "repair_fixture_test".to_string(),
                    "repair_multifile_test".to_string(),
                    "refactor_fixture_test".to_string(),
                ],
            };
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query));
            input.insert(
                "target_search_pattern".to_string(),
                Value::String(target_search_pattern),
            );
            input.insert(
                "allowed_test_commands".to_string(),
                string_array(allowed_test_commands),
            );
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
        CodeRecipe::Refactor => PathBuf::from("examples/code-agent/refactor-core.air-profile.yaml"),
        CodeRecipe::OpenRefactor => {
            PathBuf::from("examples/code-agent/open-refactor.air-profile.yaml")
        }
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
        CodeRecipe::Refactor => "refactor",
        CodeRecipe::OpenRefactor => "open-refactor",
        CodeRecipe::Build => "build",
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_recipe(
    task: &str,
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
    let asks_refactor = task
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token.eq_ignore_ascii_case("refactor"));
    if asks_refactor {
        if test.is_some() && target.is_some() {
            return CodeRecipe::Refactor;
        }
        if test.is_none() && target.is_none() {
            return CodeRecipe::OpenRefactor;
        }
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

fn code_search_pattern(query: &str) -> String {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in query.chars() {
        if character == '_' || character.is_ascii_alphanumeric() {
            current.push(character);
        } else if !current.is_empty() {
            push_search_token(&mut tokens, &current);
            current.clear();
        }
    }
    if !current.is_empty() {
        push_search_token(&mut tokens, &current);
    }

    if tokens.is_empty() {
        "TODO_DO_NOT_MATCH_EMPTY_CODE_SEARCH_PATTERN".to_string()
    } else {
        tokens.join("|")
    }
}

fn push_search_token(tokens: &mut Vec<String>, token: &str) {
    if token.len() < 3 || tokens.iter().any(|existing| existing == token) {
        return;
    }
    tokens.push(token.to_string());
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
            force_patch: false,
        })
        .unwrap();

        assert_eq!(input["task"], Value::String("fix it".to_string()));
        assert_eq!(input["query"], Value::String("fix it".to_string()));
        assert_eq!(
            input["target_search_pattern"],
            Value::String("fix".to_string())
        );
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
            force_patch: false,
        })
        .unwrap();

        assert_eq!(input["test_command"], Value::String("unit".to_string()));
        assert_eq!(
            input["target_path"],
            Value::String("src/lib.rs".to_string())
        );
    }

    #[test]
    fn repair_input_builds_code_search_pattern_from_query() {
        let input = build_input(CodeInputOptions {
            task: "Refactor provider".to_string(),
            recipe: CodeRecipe::Repair,
            target: Some(PathBuf::from("src/lib.rs")),
            test: Some("unit".to_string()),
            query: Some("EchoTools ToolProviderChoice provider module air-tools".to_string()),
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
            force_patch: true,
        })
        .unwrap();

        assert_eq!(
            input["target_search_pattern"],
            Value::String("EchoTools|ToolProviderChoice|provider|module|air|tools".to_string())
        );
        assert_eq!(input["force_patch"], Value::Bool(true));
    }

    #[test]
    fn refactor_input_forces_patch_even_when_flag_is_absent() {
        let input = build_input(CodeInputOptions {
            task: "refactor provider".to_string(),
            recipe: CodeRecipe::Refactor,
            target: Some(PathBuf::from("src/lib.rs")),
            test: Some("unit".to_string()),
            query: None,
            related: vec![PathBuf::from("src/lib_test.rs")],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
            force_patch: false,
        })
        .unwrap();

        assert_eq!(input["force_patch"], Value::Bool(true));
        assert_eq!(
            input["target_path"],
            Value::String("src/lib.rs".to_string())
        );
        assert_eq!(
            input["related_files"],
            Value::Array(vec![Value::String("src/lib_test.rs".to_string())])
        );
    }

    #[test]
    fn auto_recipe_selects_refactor_when_task_and_test_request_it() {
        let input = build_input(CodeInputOptions {
            task: "refactor the provider and keep tests passing".to_string(),
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
            force_patch: false,
        })
        .unwrap();

        assert_eq!(input["force_patch"], Value::Bool(true));
        assert_eq!(input["test_command"], Value::String("unit".to_string()));
    }

    #[test]
    fn auto_recipe_selects_open_refactor_when_refactor_has_no_target_or_test() {
        let input = build_input(CodeInputOptions {
            task: "refactor the implementation to make it cleaner".to_string(),
            recipe: CodeRecipe::Auto,
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
            force_patch: false,
        })
        .unwrap();

        assert_eq!(
            input["query"],
            Value::String("refactor the implementation to make it cleaner".to_string())
        );
        assert_eq!(
            input["target_search_pattern"],
            Value::String("refactor|the|implementation|make|cleaner".to_string())
        );
        assert_eq!(
            input["allowed_test_commands"],
            Value::Array(vec![
                Value::String("repair_fixture_test".to_string()),
                Value::String("repair_multifile_test".to_string()),
                Value::String("refactor_fixture_test".to_string()),
            ])
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
            force_patch: false,
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
            force_patch: false,
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
            force_patch: false,
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
            force_patch: false,
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
            force_patch: false,
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
        assert!(metadata.plan.ends_with("code-repair.air-plan.yaml"));
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
    fn default_session_recovery_fork_path_uses_sibling_fork_directory() {
        assert_eq!(
            default_session_recovery_fork_path(Path::new("target/generated/code_session.json"), 3),
            PathBuf::from("target/generated/code_session.forks/turn3.failed.json")
        );
    }

    #[test]
    fn project_recovery_only_for_stopped_execute_plan() {
        let stopped = json!({
            "project": {
                "status": "stopped",
                "remaining_task_ids": ["t2"],
                "executions": [
                    {
                        "task_id": "t1",
                        "title": "Failing task",
                        "recipe": "explore",
                        "completed": false,
                        "acceptance": [{"id": "unit", "success": false}],
                        "outputs": {"exploration": {"summary": "partial"}}
                    }
                ]
            }
        });
        let recovery = code_session_recovery_for_outputs(
            CodeRecipe::Plan,
            true,
            Path::new("target/generated/code_session.json"),
            1,
            "turn-000001",
            &stopped,
        )
        .unwrap();

        assert_eq!(recovery.0.status, "stopped");
        assert_eq!(recovery.0.failed_task_id, Some("t1".to_string()));
        assert_eq!(recovery.0.remaining_task_ids, vec!["t2"]);
        assert_eq!(
            recovery.0.failed_execution.as_ref().unwrap()["acceptance"][0]["id"],
            Value::String("unit".to_string())
        );
        assert_eq!(
            recovery.0.failed_execution.as_ref().unwrap()["output_keys"][0],
            Value::String("exploration".to_string())
        );
        assert_eq!(
            recovery.1,
            PathBuf::from("target/generated/code_session.forks/turn1.failed.json")
        );
        assert!(code_session_recovery_for_outputs(
            CodeRecipe::Plan,
            true,
            Path::new("target/generated/code_session.json"),
            1,
            "turn-000001",
            &json!({"project": {"status": "max_tasks_exhausted"}}),
        )
        .is_none());
        assert!(code_session_recovery_for_outputs(
            CodeRecipe::Explore,
            false,
            Path::new("target/generated/code_session.json"),
            1,
            "turn-000001",
            &stopped,
        )
        .is_none());
    }

    #[test]
    fn session_project_resume_uses_latest_exhausted_plan_turn() {
        let mut state = CodeSessionState::default();
        state.append_turn(CodeSessionTurn {
            id: "turn-000001".to_string(),
            time: CodeSessionTurnTime::default(),
            task: "run two tasks".to_string(),
            recipe: "plan".to_string(),
            profile: "profile".to_string(),
            input: json!({}),
            completed: false,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({
                "project_plan": {
                    "tasks": [
                        {"id": "t1", "depends_on": []},
                        {"id": "t2", "depends_on": ["t1"]}
                    ]
                },
                "project": {
                    "status": "max_tasks_exhausted",
                    "executions": [
                        {"task_id": "t1", "completed": true},
                        {"task_id": "bad", "completed": false}
                    ]
                }
            }),
        });

        let resume = code_session_project_resume(&state).unwrap();

        assert_eq!(resume.turn_id, "turn-000001");
        assert_eq!(
            resume.project_plan["tasks"][1]["id"],
            Value::String("t2".to_string())
        );
        assert_eq!(resume.executions.len(), 1);
        assert_eq!(
            resume.executions[0]["task_id"],
            Value::String("t1".to_string())
        );
    }

    #[test]
    fn session_project_resume_ignores_older_exhausted_after_newer_stopped() {
        let mut state = CodeSessionState::default();
        state.append_turn(CodeSessionTurn {
            id: "turn-000001".to_string(),
            time: CodeSessionTurnTime::default(),
            task: "run two tasks".to_string(),
            recipe: "plan".to_string(),
            profile: "profile".to_string(),
            input: json!({}),
            completed: false,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({
                "project_plan": {"tasks": [{"id": "t1", "depends_on": []}]},
                "project": {
                    "status": "max_tasks_exhausted",
                    "executions": [{"task_id": "t1", "completed": true}]
                }
            }),
        });
        state.append_turn(CodeSessionTurn {
            id: "turn-000002".to_string(),
            time: CodeSessionTurnTime::default(),
            task: "failed recovery".to_string(),
            recipe: "plan".to_string(),
            profile: "profile".to_string(),
            input: json!({}),
            completed: false,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({
                "project": {
                    "status": "stopped",
                    "executions": [{"task_id": "fix", "completed": false}]
                }
            }),
        });

        assert!(code_session_project_resume(&state).is_none());
        assert!(code_session_latest_project_turn(&state, "stopped").is_some());
    }

    #[test]
    fn project_recovery_turn_input_adds_failed_execution_context() {
        let mut input = Map::new();
        input.insert(
            "task".to_string(),
            Value::String("continue project".to_string()),
        );
        input.insert(
            "query".to_string(),
            Value::String("project recovery".to_string()),
        );
        let mut state = CodeSessionState::default();
        state.append_turn(CodeSessionTurn {
            id: "turn-000001".to_string(),
            time: CodeSessionTurnTime::default(),
            task: "run project".to_string(),
            recipe: "plan".to_string(),
            profile: "profile".to_string(),
            input: json!({}),
            completed: false,
            trace_files: Vec::new(),
            summary: CodeSessionTurnSummary::default(),
            parts: Vec::new(),
            patch_sets: Vec::new(),
            recovery: None,
            outputs: json!({
                "project": {
                    "status": "stopped",
                    "remaining_task_ids": ["t2"],
                    "executions": [
                        {
                            "task_id": "t1",
                            "completed": false,
                            "error": "acceptance failed",
                            "acceptance": [{"id": "unit", "success": false}]
                        }
                    ]
                },
                "recovery": {
                    "failed_task_id": "t1",
                    "fork": "target/generated/session.forks/turn1.failed.json",
                    "workspace_revert_argv": ["air", "code-session"]
                }
            }),
        });

        let next = code_project_recovery_turn_input(&input, &state).unwrap();

        let task = next["task"].as_str().unwrap();
        assert!(task.starts_with("continue project"));
        assert!(task.contains("AIR project recovery context from previous failed execution"));
        assert!(task.contains("\"failed_task_id\": \"t1\""));
        assert!(task.contains("\"remaining_task_ids\""));
        assert!(task.contains("acceptance failed"));
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
            "repair": {
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
        assert_eq!(patch_sets[0].source, "$.repair");
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
            "outputs": {
                "exploration": {
                    "summary": "found routing code",
                    "artifacts": [{"id": "repo-search:routing", "kind": "repo_search"}]
                }
            }
        })];

        let next = code_project_task_input(&input, &previous);

        assert_eq!(next["target_path"], Value::String("src/lib.rs".to_string()));
        assert_eq!(
            next["target_search_pattern"],
            Value::String("inspect|the|next|file".to_string())
        );
        assert_eq!(next["force_patch"], Value::Bool(false));
        let task = next["task"].as_str().unwrap();
        assert!(task.contains("inspect the next file"));
        assert!(task.contains("AIR project memory from previous tasks"));
        assert!(task.contains("Available project artifacts from previous tasks"));
        assert!(task.contains("found routing code"));
        assert!(task.contains("repo-search:routing"));
    }

    #[test]
    fn project_task_input_resolves_directory_targets_to_files() {
        let dir = std::env::temp_dir().join(format!(
            "air-code-project-dir-target-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let lib = dir.join("lib.rs");
        fs::write(&lib, "pub fn tool() {}\n").unwrap();

        let mut input = Map::new();
        input.insert(
            "task".to_string(),
            Value::String("explore tool".to_string()),
        );
        input.insert(
            "target_path".to_string(),
            Value::String(path_ref_to_input_string(&dir)),
        );

        let next = code_project_task_input(&input, &[]);

        assert_eq!(
            next["target_path"],
            Value::String(path_ref_to_input_string(&lib))
        );
        assert_eq!(
            next["target_search_pattern"],
            Value::String("explore|tool".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_task_input_preserves_explicit_target_search_pattern() {
        let mut input = Map::new();
        input.insert("task".to_string(), Value::String("fix it".to_string()));
        input.insert(
            "target_path".to_string(),
            Value::String("src/lib.rs".to_string()),
        );
        input.insert(
            "target_search_pattern".to_string(),
            Value::String("ExplicitSymbol".to_string()),
        );

        let next = code_project_task_input(&input, &[]);

        assert_eq!(
            next["target_search_pattern"],
            Value::String("ExplicitSymbol".to_string())
        );
    }

    #[test]
    fn project_task_input_preserves_explicit_force_patch() {
        let mut input = Map::new();
        input.insert("task".to_string(), Value::String("fix it".to_string()));
        input.insert(
            "target_path".to_string(),
            Value::String("src/lib.rs".to_string()),
        );
        input.insert("force_patch".to_string(), Value::Bool(true));

        let next = code_project_task_input(&input, &[]);

        assert_eq!(next["force_patch"], Value::Bool(true));
    }

    #[test]
    fn project_memory_extracts_stable_task_facts() {
        let executions = vec![json!({
            "task_id": "fix",
            "title": "Fix math",
            "recipe": "repair",
            "depends_on": ["inspect"],
            "completed": true,
            "acceptance": [{"id": "unit", "success": true, "output": {"large": "omitted"}}],
            "outputs": {
                "repair": {
                    "target_path": "examples/math.js",
                    "final_success": true,
                    "patch_applied": true,
                    "workspace_changed_files": [{"path": "examples/math.js"}],
                    "workspace_diff": {
                        "bytes": 333,
                        "truncated": false,
                        "artifacts": [{"id": "git-diff:examples/math.js", "kind": "file_patch", "path": "examples/math.js"}]
                    }
                }
            }
        })];

        let memory = code_project_memory(&executions);

        assert_eq!(memory["task_count"], Value::from(1));
        assert_eq!(memory["completed_task_count"], Value::from(1));
        assert_eq!(
            memory["changed_files"][0],
            Value::String("examples/math.js".to_string())
        );
        assert_eq!(
            memory["artifact_ids"][0],
            Value::String("git-diff:examples/math.js".to_string())
        );
        assert_eq!(
            memory["tasks"][0]["outputs"]["repair"]["final_success"],
            Value::Bool(true)
        );
        assert_eq!(memory["tasks"][0]["acceptance"][0].get("output"), None);
    }

    #[test]
    fn project_artifacts_index_traces_outputs_and_changed_files() {
        let traces = vec![
            "target/generated/project.trace.plan.jsonl".to_string(),
            "target/generated/project.trace.task1.jsonl".to_string(),
        ];
        let executions = vec![json!({
            "task_id": "fix",
            "outputs": {
                "repair": {
                    "workspace_changed_files": [{"path": "examples/math.js"}],
                    "workspace_diff": {
                        "artifacts": [{"id": "git-diff:examples/math.js", "kind": "file_patch", "path": "examples/math.js"}]
                    }
                }
            }
        })];

        let artifacts = code_project_artifacts(&traces, &executions);
        let artifact_items = artifacts.as_array().unwrap();
        let ids = artifact_items
            .iter()
            .filter_map(|artifact| artifact.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert!(ids.contains(&"trace:target/generated/project.trace.plan.jsonl"));
        assert!(ids.contains(&"trace:target/generated/project.trace.task1.jsonl"));
        assert!(ids.contains(&"git-diff:examples/math.js"));
        assert!(ids.contains(&"changed-file:examples/math.js"));
        let patch = artifact_items
            .iter()
            .find(|artifact| {
                artifact.get("id").and_then(Value::as_str) == Some("git-diff:examples/math.js")
            })
            .unwrap();
        assert_eq!(patch["task_id"], Value::String("fix".to_string()));
    }

    #[test]
    fn project_task_schedule_respects_dependencies() {
        let tasks = vec![
            json!({
                "id": "package",
                "depends_on": ["build"],
                "recipe": "explore",
                "input": {}
            }),
            json!({
                "id": "build",
                "depends_on": ["plan"],
                "recipe": "explore",
                "input": {}
            }),
            json!({
                "id": "plan",
                "depends_on": [],
                "recipe": "explore",
                "input": {}
            }),
        ];

        let schedule = code_project_task_schedule(&tasks).unwrap();
        let ids = schedule
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["plan", "build", "package"]);
    }

    #[test]
    fn project_budget_summary_reports_task_profiles_and_totals() {
        let tasks = vec![
            json!({
                "id": "inspect",
                "depends_on": [],
                "recipe": "explore",
                "input": {}
            }),
            json!({
                "id": "fix",
                "depends_on": ["inspect"],
                "recipe": "repair",
                "input": {}
            }),
        ];
        let schedule = code_project_task_schedule(&tasks).unwrap();

        let budget = code_project_budget_summary(&schedule);
        let task_budgets = budget["tasks"].as_array().unwrap();
        let model_total = task_budgets
            .iter()
            .map(|item| item["max_estimated_model_calls"].as_u64().unwrap())
            .sum::<u64>();
        let tool_total = task_budgets
            .iter()
            .map(|item| item["max_estimated_tool_calls"].as_u64().unwrap())
            .sum::<u64>();

        assert_eq!(budget["task_count"], Value::from(2));
        assert_eq!(
            budget["max_estimated_model_calls"],
            Value::from(model_total)
        );
        assert_eq!(budget["max_estimated_tool_calls"], Value::from(tool_total));
        assert_eq!(
            task_budgets[0]["task_id"],
            Value::String("inspect".to_string())
        );
        assert_eq!(
            task_budgets[0]["recipe"],
            Value::String("explore".to_string())
        );
        assert_eq!(task_budgets[1]["task_id"], Value::String("fix".to_string()));
        assert_eq!(
            task_budgets[1]["recipe"],
            Value::String("repair".to_string())
        );
        assert_eq!(
            task_budgets[1]["depends_on"],
            Value::Array(vec![Value::String("inspect".to_string())])
        );
        assert!(task_budgets[0]["read_only"].as_bool().unwrap());
        assert!(task_budgets[1]["writes_workspace"].as_bool().unwrap());
        assert!(model_total > 0);
        assert!(tool_total > 0);
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
    fn project_task_schedule_rejects_missing_dependency() {
        let error = code_project_task_schedule(&[json!({
            "id": "build",
            "depends_on": ["plan"],
            "recipe": "explore",
            "input": {}
        })])
        .unwrap_err();

        assert!(error.to_string().contains("depends on missing task"));
    }

    #[test]
    fn project_task_schedule_rejects_cycles() {
        let error = code_project_task_schedule(&[
            json!({
                "id": "a",
                "depends_on": ["b"],
                "recipe": "explore",
                "input": {}
            }),
            json!({
                "id": "b",
                "depends_on": ["a"],
                "recipe": "explore",
                "input": {}
            }),
        ])
        .unwrap_err();

        assert!(error.to_string().contains("dependency cycle"));
    }

    #[test]
    fn acceptance_checks_build_test_run_inputs() {
        let checks = acceptance_checks(Some(&json!([
            {
                "id": "unit",
                "command": "air_tools_filter",
                "parameters": {"test_filter": "command_run"},
                "expected": "command_run tests pass"
            },
            "legacy descriptive acceptance item"
        ])));

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].id, "unit");
        assert_eq!(checks[0].command, "air_tools_filter");
        assert_eq!(checks[0].expected, "command_run tests pass");
        assert_eq!(
            checks[0].input["command"],
            Value::String("air_tools_filter".to_string())
        );
        assert_eq!(
            checks[0].input["test_filter"],
            Value::String("command_run".to_string())
        );
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
            patch_sets: Vec::new(),
            recovery: None,
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
    fn session_parts_recover_model_from_start_event_when_completion_meta_is_truncated() {
        let events = vec![
            serde_json::from_value::<TraceEvent>(json!({
                "agent": "planner",
                "step": 1,
                "rule": "plan",
                "action": "model_call_start",
                "input": {},
                "output": null,
                "meta": {
                    "model": "project_planner",
                    "output": "project_plan",
                    "timeout_seconds": 120
                },
                "status": "ok",
                "error": null
            }))
            .unwrap(),
            serde_json::from_value::<TraceEvent>(json!({
                "agent": "planner",
                "step": 1,
                "rule": "plan",
                "action": "model_call",
                "input": {},
                "output": {"project_plan": {"summary": "large output omitted"}},
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

        assert_eq!(part.model, Some("project_planner".to_string()));
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
                recipe: "repair".to_string(),
                profile: "examples/code-agent/repair-core.air-profile.yaml".to_string(),
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
            patch_sets: Vec::new(),
            recovery: None,
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
    fn session_turn_index_accepts_id_or_number() {
        let mut state = CodeSessionState::default();
        state.append_turn(CodeSessionTurn {
            id: "turn-000001".to_string(),
            time: CodeSessionTurnTime::default(),
            task: "one".to_string(),
            recipe: "explore".to_string(),
            profile: "profile".to_string(),
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
            recipe: "review".to_string(),
            profile: "profile".to_string(),
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
            recipe: "explore".to_string(),
            profile: "profile".to_string(),
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
            force_patch: false,
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
            force_patch: false,
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
            force_patch: false,
        })
        .unwrap();

        assert_eq!(
            input["related_files"],
            Value::Array(vec![Value::String("scripts/search.cjs".to_string())])
        );
    }
}
