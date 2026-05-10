use crate::code_pack::{CodeAgentCompletion, CodeAgentRouteDecision};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CodeSessionState {
    #[serde(default = "code_session_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) turns: Vec<CodeSessionTurn>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeSessionTurn {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) time: CodeSessionTurnTime,
    pub(crate) task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) requested_recipe: Option<String>,
    pub(crate) recipe: String,
    pub(crate) profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pack: Option<CodeSessionTurnPack>,
    pub(crate) input: Value,
    pub(crate) completed: bool,
    #[serde(default)]
    pub(crate) trace_files: Vec<String>,
    #[serde(default)]
    pub(crate) summary: CodeSessionTurnSummary,
    #[serde(default)]
    pub(crate) parts: Vec<CodeSessionPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) patch_sets: Vec<CodeSessionPatchSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recovery: Option<CodeSessionRecovery>,
    pub(crate) outputs: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeSessionTurnPack {
    pub(crate) path: String,
    pub(crate) recipe: String,
    pub(crate) default_profile: String,
    pub(crate) profile_override: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) intent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completion: Option<CodeAgentCompletion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) routing_decision: Option<CodeAgentRouteDecision>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CodeSessionTurnTime {
    pub(crate) created: u64,
    pub(crate) updated: u64,
}

impl CodeSessionTurnTime {
    pub(crate) fn now() -> Self {
        let now = unix_millis();
        Self {
            created: now,
            updated: now,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CodeSessionTurnSummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) approvals: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifact_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifact_kinds: Vec<String>,
    pub(crate) model_call_count: usize,
    pub(crate) tool_call_count: usize,
    pub(crate) approval_count: usize,
    pub(crate) error_count: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CodeSessionPatchSet {
    pub(crate) source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace_clean_before: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace_clean_after: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) preexisting_changed_files: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) changed_files: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) workspace_changed_files: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) diff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) diff_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) diff_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifact_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeSessionRecovery {
    pub(crate) reason: String,
    pub(crate) status: String,
    pub(crate) fork: String,
    pub(crate) turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failed_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failed_execution: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) remaining_task_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) workspace_revert_argv: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeSessionPart {
    pub(crate) kind: String,
    pub(crate) trace_file: String,
    pub(crate) agent: String,
    pub(crate) step: u32,
    pub(crate) rule: String,
    pub(crate) action: String,
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) approval_for: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifact_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifact_kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) input_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) output_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CodeSessionWorkspaceRevertSummary {
    pub(crate) turn_id: String,
    pub(crate) applied: bool,
    pub(crate) success: bool,
    pub(crate) patch_set_count: usize,
    pub(crate) results: Vec<CodeSessionWorkspaceRevertResult>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CodeSessionWorkspaceRevertResult {
    pub(crate) source: String,
    pub(crate) repo: Option<String>,
    pub(crate) checked: bool,
    pub(crate) applied: bool,
    pub(crate) success: bool,
    pub(crate) status: Option<i32>,
    pub(crate) log: String,
}

pub(crate) fn code_session_turn_id(turn_number: usize) -> String {
    format!("turn-{turn_number:06}")
}

pub(crate) fn code_session_turn_index(state: &CodeSessionState, target: &str) -> Result<usize> {
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

pub(crate) fn code_session_workspace_revert(
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

impl CodeSessionState {
    pub(crate) fn read(path: &Path) -> Result<Self> {
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

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub(crate) fn append_turn(&mut self, turn: CodeSessionTurn) {
        self.version = code_session_version();
        self.turns.push(turn);
    }
}

fn code_session_version() -> u32 {
    1
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
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
