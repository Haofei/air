use crate::run_plan::write_trace;
use air_runtime::{read_trace_jsonl, system_return_event, TraceStatus};
use anyhow::{bail, Context, Result};
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const CODE_RUN_ARTIFACT_SCHEMA: &str = "air.code_run_artifact.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CodeRunArtifact {
    pub(crate) schema: String,
    pub(crate) fingerprint: String,
    pub(crate) task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skill: Option<CodeRunSkill>,
    pub(crate) profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model_config: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_config: Option<String>,
    pub(crate) mode: CodeRunMode,
    pub(crate) snapshot: CodeRunSnapshot,
    pub(crate) delta: WorkspaceDelta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verdict: Option<CodeRunVerdict>,
    pub(crate) files: CodeRunArtifactFiles,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) failure_reason: Option<FailureReason>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CodeRunMode {
    Executed,
    Replayed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CodeRunSnapshot {
    pub(crate) before: WorkspaceSnapshotSummary,
    pub(crate) after: WorkspaceSnapshotSummary,
    pub(crate) preexisting_changed_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CodeRunArtifactFiles {
    pub(crate) output: String,
    pub(crate) trace: String,
    pub(crate) diff: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subagents: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CodeRunSkill {
    pub(crate) id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) manifest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) manifest_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) audit_risk: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) effective_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FailureReason {
    pub(crate) category: FailureCategory,
    pub(crate) message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) details: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureCategory {
    AgentError,
    VerificationFailed,
    DiffConstraintFailed,
    NoPatchApplied,
    PolicyViolation,
    Timeout,
    ProviderRateLimited,
    ReplayMismatch,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceSnapshot {
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkspaceSnapshotSummary {
    pub(crate) files: Vec<FileSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FileSnapshot {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkspaceDelta {
    pub(crate) changed_files: Vec<String>,
    pub(crate) diff: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CodeRunVerdict {
    pub(crate) patch_applied: bool,
    pub(crate) verification_ran: bool,
    pub(crate) verification_passed: bool,
    pub(crate) changed_files: Vec<String>,
    #[serde(default)]
    pub(crate) workspace_clean_ok: bool,
    pub(crate) allowed_files_ok: bool,
    pub(crate) required_files_ok: bool,
    pub(crate) forbidden_files_ok: bool,
    pub(crate) required_diff_ok: bool,
    pub(crate) max_diff_lines_ok: bool,
    pub(crate) final_success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure_reason: Option<FailureReason>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CodeRunVerdictConstraints {
    pub(crate) require_patch: bool,
    pub(crate) require_verification: bool,
    pub(crate) forbid_workspace_changes: bool,
    pub(crate) allowed_changed_files: Vec<String>,
    pub(crate) required_changed_files: Vec<String>,
    pub(crate) forbidden_changed_files: Vec<String>,
    pub(crate) required_diff_contains: Vec<String>,
    pub(crate) max_diff_lines: Option<usize>,
}

impl CodeRunVerdictConstraints {
    pub(crate) fn code_edit() -> Self {
        Self {
            require_patch: true,
            require_verification: true,
            ..Self::default()
        }
    }

    pub(crate) fn review() -> Self {
        Self {
            forbid_workspace_changes: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CodeRunVerificationFacts {
    pub(crate) verification_ran: bool,
    pub(crate) verification_passed: bool,
}

impl WorkspaceSnapshot {
    pub(crate) fn capture(root: &Path) -> Result<Self> {
        let mut files = BTreeMap::new();
        for relative in workspace_file_list(root)? {
            let path = root.join(&relative);
            if !path.is_file() {
                continue;
            }
            let content = match fs::read(&path) {
                Ok(content) => content,
                Err(_) => continue,
            };
            files.insert(relative, content);
        }
        Ok(Self { files })
    }

    pub(crate) fn delta(&self, after: &Self) -> Result<WorkspaceDelta> {
        let changed_files = self
            .files
            .keys()
            .chain(after.files.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|path| self.files.get(*path) != after.files.get(*path))
            .cloned()
            .collect::<Vec<_>>();
        let diff = render_workspace_delta_diff(self, after, &changed_files)?;
        Ok(WorkspaceDelta {
            changed_files,
            diff,
        })
    }

    pub(crate) fn summary(&self) -> WorkspaceSnapshotSummary {
        WorkspaceSnapshotSummary {
            files: self
                .files
                .iter()
                .map(|(path, content)| FileSnapshot {
                    path: path.clone(),
                    sha256: sha256_hex(content),
                    bytes: content.len(),
                })
                .collect(),
        }
    }
}

impl WorkspaceSnapshotSummary {
    pub(crate) fn matches_snapshot(&self, snapshot: &WorkspaceSnapshot) -> bool {
        let actual = snapshot.summary();
        self.files.len() == actual.files.len()
            && self
                .files
                .iter()
                .zip(actual.files.iter())
                .all(|(left, right)| left.path == right.path && left.sha256 == right.sha256)
    }
}

pub(crate) fn patch_code_output_with_workspace_delta(
    outputs: &mut Value,
    delta: &WorkspaceDelta,
    preexisting_changed_files: &[String],
    verdict: &CodeRunVerdict,
) {
    let Some(edit) = outputs.get_mut("edit").and_then(Value::as_object_mut) else {
        return;
    };
    let changed_files = Value::Array(
        delta
            .changed_files
            .iter()
            .cloned()
            .map(Value::String)
            .collect(),
    );
    edit.insert("changed_files".to_string(), changed_files.clone());
    edit.insert("workspace_changed_files".to_string(), changed_files);
    edit.insert(
        "preexisting_changed_files".to_string(),
        Value::Array(
            preexisting_changed_files
                .iter()
                .map(|path| json!({ "path": path }))
                .collect(),
        ),
    );
    edit.insert(
        "patch_applied".to_string(),
        Value::Bool(verdict.patch_applied),
    );
    edit.insert(
        "final_success".to_string(),
        Value::Bool(verdict.final_success),
    );
    edit.insert("verdict".to_string(), json!(verdict));
    edit.insert(
        "workspace_diff".to_string(),
        json!({
            "provider": "air-code-artifact",
            "command": "workspace snapshot diff",
            "success": true,
            "diff": delta.diff,
            "bytes": delta.diff.len(),
            "truncated": false,
        }),
    );
    if let Some(reason) = verdict.failure_reason.as_ref() {
        edit.insert("failure_reason".to_string(), json!(reason));
    }
}

pub(crate) fn derive_code_run_verdict(
    outputs: &Value,
    delta: &WorkspaceDelta,
    trace_path: Option<&Path>,
    constraints: &CodeRunVerdictConstraints,
) -> CodeRunVerdict {
    let facts = trace_path
        .and_then(verification_facts_from_trace)
        .unwrap_or_else(|| verification_facts_from_output(outputs));
    derive_code_run_verdict_from_facts(delta, facts, constraints)
}

pub(crate) fn derive_code_run_verdict_from_facts(
    delta: &WorkspaceDelta,
    facts: CodeRunVerificationFacts,
    constraints: &CodeRunVerdictConstraints,
) -> CodeRunVerdict {
    let patch_applied = !delta.changed_files.is_empty();
    let workspace_clean_ok = !constraints.forbid_workspace_changes || !patch_applied;
    let allowed_files_ok = constraints.allowed_changed_files.is_empty()
        || delta
            .changed_files
            .iter()
            .all(|path| constraints.allowed_changed_files.contains(path));
    let required_files_ok = constraints
        .required_changed_files
        .iter()
        .all(|path| delta.changed_files.contains(path));
    let forbidden_files_ok = constraints
        .forbidden_changed_files
        .iter()
        .all(|path| !delta.changed_files.contains(path));
    let required_diff_ok = constraints
        .required_diff_contains
        .iter()
        .all(|needle| delta.diff.contains(needle));
    let max_diff_lines_ok = constraints
        .max_diff_lines
        .is_none_or(|limit| code_diff_line_count(&delta.diff) <= limit);
    let patch_ok = !constraints.require_patch || patch_applied;
    let verification_ok =
        !constraints.require_verification || (facts.verification_ran && facts.verification_passed);
    let final_success = workspace_clean_ok
        && patch_ok
        && verification_ok
        && allowed_files_ok
        && required_files_ok
        && forbidden_files_ok
        && required_diff_ok
        && max_diff_lines_ok;
    let mut verdict = CodeRunVerdict {
        patch_applied,
        verification_ran: facts.verification_ran,
        verification_passed: facts.verification_passed,
        changed_files: delta.changed_files.clone(),
        workspace_clean_ok,
        allowed_files_ok,
        required_files_ok,
        forbidden_files_ok,
        required_diff_ok,
        max_diff_lines_ok,
        final_success,
        failure_reason: None,
    };
    verdict.failure_reason = code_verdict_failure_reason(&verdict, constraints);
    verdict
}

fn verification_facts_from_trace(path: &Path) -> Option<CodeRunVerificationFacts> {
    let events = read_trace_jsonl(path).ok()?;
    let mut verification_ran = false;
    let mut verification_passed = false;
    for event in events {
        if event.action != "tool_batch_dispatch_item" || event.status != TraceStatus::Ok {
            continue;
        }
        let output = event.output.as_ref()?;
        if output
            .get("workspace_changed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            verification_passed = false;
        }
        if !is_verification_tool_output(output, tool_name_from_trace_event(&event).as_deref()) {
            continue;
        }
        verification_ran = true;
        let success = output
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let workspace_changed = output
            .get("workspace_changed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if success && !workspace_changed {
            verification_passed = true;
        } else if !success {
            verification_passed = false;
        }
    }
    Some(CodeRunVerificationFacts {
        verification_ran,
        verification_passed,
    })
}

fn verification_facts_from_output(outputs: &Value) -> CodeRunVerificationFacts {
    let fallback_passed = outputs
        .pointer("/edit/final_success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    CodeRunVerificationFacts {
        verification_ran: fallback_passed,
        verification_passed: fallback_passed,
    }
}

fn tool_name_from_trace_event(event: &air_runtime::TraceEvent) -> Option<String> {
    event
        .meta
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("tool"))
        .or_else(|| {
            event
                .output
                .as_ref()
                .and_then(|output| output.pointer("/output/tool"))
        })
        .or_else(|| {
            event
                .input
                .as_ref()
                .and_then(|input| input.get("tool").or_else(|| input.get("name")))
        })
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn is_verification_tool_output(output: &Value, tool: Option<&str>) -> bool {
    if tool == Some("test") {
        return true;
    }
    output
        .get("verification")
        .or_else(|| output.pointer("/output/verification"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn code_verdict_failure_reason(
    verdict: &CodeRunVerdict,
    constraints: &CodeRunVerdictConstraints,
) -> Option<FailureReason> {
    if verdict.final_success {
        return None;
    }
    if constraints.require_patch && !verdict.patch_applied {
        return Some(FailureReason {
            category: FailureCategory::NoPatchApplied,
            message: "code agent finished without changing workspace files".to_string(),
            details: BTreeMap::new(),
        });
    }
    if constraints.require_verification && !verdict.verification_ran {
        return Some(FailureReason {
            category: FailureCategory::VerificationFailed,
            message: "code agent finished without running verification".to_string(),
            details: BTreeMap::new(),
        });
    }
    if constraints.require_verification && !verdict.verification_passed {
        return Some(FailureReason {
            category: FailureCategory::VerificationFailed,
            message: "code agent verification did not pass".to_string(),
            details: BTreeMap::new(),
        });
    }
    if constraints.forbid_workspace_changes && !verdict.workspace_clean_ok {
        return Some(FailureReason {
            category: FailureCategory::PolicyViolation,
            message: "read-only agent changed workspace files".to_string(),
            details: BTreeMap::from([("changed_files".to_string(), json!(verdict.changed_files))]),
        });
    }
    let mut violations = Vec::new();
    if !verdict.allowed_files_ok {
        violations.push("changed file outside allowed_changed_files".to_string());
    }
    if !verdict.required_files_ok {
        violations.push("required changed file missing".to_string());
    }
    if !verdict.forbidden_files_ok {
        violations.push("forbidden file changed".to_string());
    }
    if !verdict.required_diff_ok {
        violations.push("required diff text missing".to_string());
    }
    if !verdict.max_diff_lines_ok {
        violations.push("diff exceeds max_diff_lines".to_string());
    }
    if !violations.is_empty() {
        return Some(FailureReason {
            category: FailureCategory::DiffConstraintFailed,
            message: "code run diff constraints failed".to_string(),
            details: BTreeMap::from([("violations".to_string(), json!(violations))]),
        });
    }
    Some(FailureReason {
        category: FailureCategory::AgentError,
        message: "code run did not satisfy AIR verdict requirements".to_string(),
        details: BTreeMap::new(),
    })
}

fn code_diff_line_count(diff: &str) -> usize {
    diff.lines()
        .filter(|line| {
            !line.starts_with("diff ")
                && !line.starts_with("index ")
                && !line.starts_with("--- ")
                && !line.starts_with("+++ ")
                && !line.starts_with("@@")
        })
        .count()
}

pub(crate) fn patch_trace_return(path: &Path, outputs: &Value, trace_redact: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut events = read_trace_jsonl(path)
        .map_err(|error| anyhow::anyhow!("read trace for code output patch: {error}"))?;
    let mut output_object = match outputs {
        Value::Object(object) => object.clone(),
        _ => return Ok(()),
    };
    let replacement = system_return_event(std::mem::take(&mut output_object));
    if let Some(event) = events
        .iter_mut()
        .rev()
        .find(|event| event.agent == "$system" && event.action == "return")
    {
        *event = replacement;
    } else {
        events.push(replacement);
    }
    write_trace(path, &events, trace_redact)
}

pub(crate) fn git_changed_files(root: &Path) -> Result<Vec<String>> {
    if !root.join(".git").exists() {
        return Ok(Vec::new());
    }
    let mut files = BTreeSet::new();
    for args in [
        ["diff", "--name-only", "--cached", "--"].as_slice(),
        ["diff", "--name-only", "--"].as_slice(),
        ["ls-files", "--deleted", "--"].as_slice(),
        ["ls-files", "--others", "--exclude-standard"].as_slice(),
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .with_context(|| format!("run git {}", args.join(" ")))?;
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let path = line.trim().replace('\\', "/");
            if !path.is_empty() && !ignored_workspace_path(&path) {
                files.insert(path);
            }
        }
    }
    Ok(files.into_iter().collect())
}

pub(crate) fn path_content_identity(path: &Path) -> Result<String> {
    let content = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(format!("sha256:{}", sha256_hex(&content)))
}

pub(crate) fn build_code_run_artifact(
    descriptor: CodeRunDescriptor,
    before: &WorkspaceSnapshot,
    after: &WorkspaceSnapshot,
    delta: WorkspaceDelta,
    preexisting_changed_files: Vec<String>,
    failure_reason: Option<FailureReason>,
    verdict: Option<CodeRunVerdict>,
) -> Result<CodeRunArtifact> {
    let fingerprint = descriptor.fingerprint(before)?;
    Ok(CodeRunArtifact {
        schema: CODE_RUN_ARTIFACT_SCHEMA.to_string(),
        fingerprint,
        task: descriptor.task,
        skill: descriptor.skill,
        profile: descriptor.profile,
        model_config: descriptor.model_config,
        tool_config: descriptor.tool_config,
        mode: CodeRunMode::Executed,
        snapshot: CodeRunSnapshot {
            before: before.summary(),
            after: after.summary(),
            preexisting_changed_files,
        },
        delta,
        verdict,
        files: CodeRunArtifactFiles {
            output: "output.json".to_string(),
            trace: "trace.jsonl".to_string(),
            diff: "diff.patch".to_string(),
            subagents: Some("subagents".to_string()),
        },
        failure_reason,
        extra: descriptor.extra,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct CodeRunDescriptor {
    pub(crate) task: String,
    pub(crate) skill: Option<CodeRunSkill>,
    pub(crate) profile: String,
    pub(crate) model_config: Option<String>,
    pub(crate) tool_config: Option<String>,
    pub(crate) extra: BTreeMap<String, Value>,
}

impl CodeRunDescriptor {
    pub(crate) fn fingerprint(&self, before: &WorkspaceSnapshot) -> Result<String> {
        let value = json!({
            "schema": "air.code_run_fingerprint.v1",
            "task": self.task,
            "skill": self.skill,
            "profile": self.profile,
            "model_config": self.model_config,
            "tool_config": self.tool_config,
            "workspace_before": before.summary(),
            "extra": self.extra,
        });
        canonical_sha256(&value)
    }
}

pub(crate) fn write_code_run_artifact(
    artifact_dir: &Path,
    artifact: &CodeRunArtifact,
    output: &Value,
    trace_path: Option<&Path>,
) -> Result<()> {
    fs::create_dir_all(artifact_dir)
        .with_context(|| format!("create {}", artifact_dir.display()))?;
    fs::write(
        artifact_dir.join("artifact.json"),
        serde_json::to_vec_pretty(artifact)?,
    )
    .with_context(|| format!("write {}", artifact_dir.join("artifact.json").display()))?;
    fs::write(
        artifact_dir.join(&artifact.files.output),
        serde_json::to_vec_pretty(output)?,
    )
    .with_context(|| {
        format!(
            "write {}",
            artifact_dir.join(&artifact.files.output).display()
        )
    })?;
    fs::write(
        artifact_dir.join(&artifact.files.diff),
        artifact.delta.diff.as_bytes(),
    )
    .with_context(|| {
        format!(
            "write {}",
            artifact_dir.join(&artifact.files.diff).display()
        )
    })?;
    if let Some(trace_path) = trace_path {
        if trace_path.exists() {
            let destination = artifact_dir.join(&artifact.files.trace);
            let rewrites = if let Some(subagents) = &artifact.files.subagents {
                copy_subagent_artifacts_from_trace(trace_path, &artifact_dir.join(subagents))?
            } else {
                BTreeMap::new()
            };
            if rewrites.is_empty() {
                if !same_file(trace_path, &destination) {
                    fs::copy(trace_path, &destination).with_context(|| {
                        format!("copy {} to {}", trace_path.display(), destination.display())
                    })?;
                }
            } else {
                let mut events = read_trace_jsonl(trace_path)
                    .map_err(|error| anyhow::anyhow!("read trace for artifact copy: {error}"))?;
                for event in &mut events {
                    if let Some(input) = &mut event.input {
                        rewrite_subagent_paths(input, &rewrites);
                    }
                    if let Some(output) = &mut event.output {
                        rewrite_subagent_paths(output, &rewrites);
                    }
                    if let Some(meta) = &mut event.meta {
                        rewrite_subagent_paths(meta, &rewrites);
                    }
                }
                write_trace(&destination, &events, false)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn write_timeout_code_run_artifact(
    artifact_dir: &Path,
    task: &str,
    timeout_seconds: u64,
) -> Result<()> {
    if artifact_dir.join("artifact.json").exists() {
        return Ok(());
    }
    let before = WorkspaceSnapshot::default();
    let descriptor = CodeRunDescriptor {
        task: task.to_string(),
        skill: None,
        profile: "air-run-timeout-supervisor".to_string(),
        model_config: None,
        tool_config: None,
        extra: BTreeMap::from([("timeout_seconds".to_string(), Value::from(timeout_seconds))]),
    };
    let failure_reason = FailureReason {
        category: FailureCategory::Timeout,
        message: format!("air run exceeded wall-clock timeout of {timeout_seconds} seconds"),
        details: BTreeMap::from([("timeout_seconds".to_string(), Value::from(timeout_seconds))]),
    };
    let verdict = CodeRunVerdict {
        patch_applied: false,
        verification_ran: false,
        verification_passed: false,
        changed_files: Vec::new(),
        workspace_clean_ok: true,
        allowed_files_ok: true,
        required_files_ok: false,
        forbidden_files_ok: true,
        required_diff_ok: false,
        max_diff_lines_ok: true,
        final_success: false,
        failure_reason: Some(failure_reason.clone()),
    };
    let artifact = build_code_run_artifact(
        descriptor,
        &before,
        &before,
        WorkspaceDelta {
            changed_files: Vec::new(),
            diff: String::new(),
        },
        Vec::new(),
        Some(failure_reason),
        Some(verdict),
    )?;
    let output = json!({
        "edit": {
            "patch_applied": false,
            "verification_ran": false,
            "verification_passed": false,
            "changed_files": [],
            "final_success": false,
            "failure_reason": {
                "category": "timeout",
                "message": format!("air run exceeded wall-clock timeout of {timeout_seconds} seconds")
            }
        }
    });
    write_code_run_artifact(artifact_dir, &artifact, &output, None)?;
    fs::write(artifact_dir.join(&artifact.files.trace), b"").with_context(|| {
        format!(
            "write {}",
            artifact_dir.join(&artifact.files.trace).display()
        )
    })
}

fn copy_subagent_artifacts_from_trace(
    trace_path: &Path,
    destination_root: &Path,
) -> Result<BTreeMap<String, String>> {
    let events = read_trace_jsonl(trace_path)
        .map_err(|error| anyhow::anyhow!("read trace for subagent artifact copy: {error}"))?;
    let mut path_sets = Vec::new();
    for event in events {
        if let Some(output) = event.output {
            collect_subagent_path_sets(&output, &mut path_sets);
        }
    }

    let mut rewrites = BTreeMap::new();
    for (index, paths) in path_sets.into_iter().enumerate() {
        let subagent_dir = destination_root.join(index.to_string());
        fs::create_dir_all(&subagent_dir)
            .with_context(|| format!("create {}", subagent_dir.display()))?;
        if let Some(trace) = paths.child_trace_path {
            let destination = subagent_dir.join("trace.jsonl");
            if copy_file_if_exists(Path::new(&trace), &destination)? {
                rewrites.insert(trace, destination.display().to_string());
            }
        }
        if let Some(output) = paths.child_output_path {
            let destination = subagent_dir.join("output.json");
            if copy_file_if_exists(Path::new(&output), &destination)? {
                rewrites.insert(output, destination.display().to_string());
            }
        }
        if let Some(artifact) = paths.child_artifact_path {
            let destination = subagent_dir.join("artifact");
            if Path::new(&artifact).exists() {
                copy_dir_all(Path::new(&artifact), &destination)?;
                rewrites.insert(artifact, destination.display().to_string());
            }
        }
    }
    Ok(rewrites)
}

#[derive(Debug, Default)]
struct SubagentPathSet {
    child_trace_path: Option<String>,
    child_output_path: Option<String>,
    child_artifact_path: Option<String>,
}

fn collect_subagent_path_sets(value: &Value, sets: &mut Vec<SubagentPathSet>) {
    match value {
        Value::Object(object) => {
            let paths = SubagentPathSet {
                child_trace_path: object
                    .get("child_trace_path")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                child_output_path: object
                    .get("child_output_path")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                child_artifact_path: object
                    .get("child_artifact_path")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            };
            if paths.child_trace_path.is_some()
                || paths.child_output_path.is_some()
                || paths.child_artifact_path.is_some()
            {
                sets.push(paths);
            }
            for value in object.values() {
                collect_subagent_path_sets(value, sets);
            }
        }
        Value::Array(array) => {
            for value in array {
                collect_subagent_path_sets(value, sets);
            }
        }
        _ => {}
    }
}

fn rewrite_subagent_paths(value: &mut Value, rewrites: &BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(replacement) = rewrites.get(text) {
                *text = replacement.clone();
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                rewrite_subagent_paths(value, rewrites);
            }
        }
        Value::Array(array) => {
            for value in array {
                rewrite_subagent_paths(value, rewrites);
            }
        }
        _ => {}
    }
}

fn copy_file_if_exists(source: &Path, destination: &Path) -> Result<bool> {
    if !source.exists() {
        return Ok(false);
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::copy(source, destination)
        .with_context(|| format!("copy {} to {}", source.display(), destination.display()))?;
    Ok(true)
}

fn copy_dir_all(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copy {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

pub(crate) fn read_code_run_artifact(artifact_dir: &Path) -> Result<CodeRunArtifact> {
    let artifact_path = artifact_dir.join("artifact.json");
    serde_json::from_slice(
        &fs::read(&artifact_path).with_context(|| format!("read {}", artifact_path.display()))?,
    )
    .with_context(|| format!("parse {}", artifact_path.display()))
}

pub(crate) fn code_run_trace_path(artifact_dir: &Path) -> Result<PathBuf> {
    let artifact = read_code_run_artifact(artifact_dir)?;
    Ok(artifact_dir.join(&artifact.files.trace))
}

pub(crate) fn code_run_path_rewrites(
    artifact_dir: &Path,
    current_root: &Path,
) -> Result<Vec<(String, String)>> {
    let artifact = read_code_run_artifact(artifact_dir)?;
    let trace_path = artifact_dir.join(&artifact.files.trace);
    let trace = fs::read_to_string(&trace_path)
        .with_context(|| format!("read {}", trace_path.display()))?;
    let current_root = current_root
        .canonicalize()
        .unwrap_or_else(|_| current_root.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    let mut rewrites = BTreeSet::new();
    for file in artifact.snapshot.before.files {
        let suffix = format!("/{}", file.path.replace('\\', "/"));
        let mut search_from = 0;
        while let Some(offset) = trace[search_from..].find(&suffix) {
            let suffix_start = search_from + offset;
            let Some(root_start) = trace[..suffix_start].rfind('"') else {
                search_from = suffix_start + suffix.len();
                continue;
            };
            let old_root = trace[root_start + 1..suffix_start].replace("\\/", "/");
            if old_root.starts_with('/') && old_root != current_root {
                rewrites.insert((old_root, current_root.clone()));
            }
            search_from = suffix_start + suffix.len();
        }
    }
    Ok(rewrites.into_iter().collect())
}

pub(crate) fn replay_code_run_artifact(artifact_dir: &Path, workdir: &Path) -> Result<Value> {
    let artifact = read_code_run_artifact(artifact_dir)?;
    let before = WorkspaceSnapshot::capture(workdir)?;
    if !artifact.snapshot.before.matches_snapshot(&before) {
        bail!("code run artifact replay mismatch: workspace snapshot does not match artifact");
    }
    let diff_path = artifact_dir.join(&artifact.files.diff);
    let status = Command::new("git")
        .args(["apply", "--allow-empty", "--whitespace=nowarn"])
        .arg(&diff_path)
        .current_dir(workdir)
        .status()
        .with_context(|| format!("apply cached diff {}", diff_path.display()))?;
    if !status.success() {
        bail!("code run artifact replay failed: git apply rejected cached diff");
    }
    let output_path = artifact_dir.join(&artifact.files.output);
    serde_json::from_slice(
        &fs::read(&output_path).with_context(|| format!("read {}", output_path.display()))?,
    )
    .with_context(|| format!("parse {}", output_path.display()))
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn workspace_file_list(root: &Path) -> Result<Vec<String>> {
    if root.join(".git").exists() {
        let output = Command::new("git")
            .args(["ls-files", "-co", "--exclude-standard", "-z"])
            .current_dir(root)
            .output()
            .context("run git ls-files for workspace snapshot")?;
        if output.status.success() {
            return Ok(output
                .stdout
                .split(|byte| *byte == 0)
                .filter_map(|entry| std::str::from_utf8(entry).ok())
                .map(str::trim)
                .filter(|path| !path.is_empty() && !ignored_workspace_path(path))
                .map(|path| path.replace('\\', "/"))
                .collect());
        }
    }
    let mut paths = Vec::new();
    collect_workspace_files(root, root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_workspace_files(root: &Path, dir: &Path, paths: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read directory {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if ignored_workspace_path(&relative) {
            continue;
        }
        if path.is_dir() {
            collect_workspace_files(root, &path, paths)?;
        } else if path.is_file() {
            paths.push(relative);
        }
    }
    Ok(())
}

fn ignored_workspace_path(path: &str) -> bool {
    path == ".git"
        || path == ".air"
        || path == "target"
        || path == "node_modules"
        || path == "__pycache__"
        || path.starts_with(".git/")
        || path.starts_with(".air/")
        || path.ends_with("/.git")
        || path.ends_with("/.air")
        || path.contains("/.git/")
        || path.contains("/.air/")
        || path.starts_with("target/")
        || path.starts_with("node_modules/")
        || path.starts_with("__pycache__/")
        || path.ends_with(".pyc")
        || path.contains("/__pycache__/")
}

fn render_workspace_delta_diff(
    before: &WorkspaceSnapshot,
    after: &WorkspaceSnapshot,
    changed_files: &[String],
) -> Result<String> {
    if changed_files.is_empty() {
        return Ok(String::new());
    }
    let temp_dir = std::env::temp_dir().join(format!(
        "air-code-diff-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir)?;
    let mut parts = Vec::new();
    for (index, path) in changed_files.iter().enumerate() {
        let before_path = temp_dir.join(format!("{index}.before"));
        let after_path = temp_dir.join(format!("{index}.after"));
        fs::write(
            &before_path,
            before.files.get(path).map(Vec::as_slice).unwrap_or(&[]),
        )?;
        fs::write(
            &after_path,
            after.files.get(path).map(Vec::as_slice).unwrap_or(&[]),
        )?;
        let output = Command::new("diff")
            .args([
                "-u",
                "--label",
                &format!("a/{path}"),
                "--label",
                &format!("b/{path}"),
            ])
            .arg(&before_path)
            .arg(&after_path)
            .output()
            .with_context(|| format!("render diff for {path}"))?;
        if !output.stdout.is_empty() {
            parts.push(String::from_utf8_lossy(&output.stdout).to_string());
        } else if !output.stderr.is_empty() {
            parts.push(String::from_utf8_lossy(&output.stderr).to_string());
        }
    }
    let _ = fs::remove_dir_all(&temp_dir);
    Ok(parts.join(""))
}

fn canonical_sha256<T: Serialize>(value: &T) -> Result<String> {
    let encoded = serde_json::to_vec(value).context("serialize fingerprint payload")?;
    Ok(format!("sha256:{}", sha256_hex(&encoded)))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = digest::digest(&digest::SHA256, bytes);
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn code_run_artifact_replays_workspace_delta_without_model_calls() {
        let temp_dir = unique_temp_dir("air-code-artifact-replay");
        let workdir = temp_dir.join("work");
        let artifact_dir = temp_dir.join("artifact");
        let source_path = workdir.join("src/lib.rs");
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, "pub fn value() -> i32 {\n    1\n}\n").unwrap();

        let before = WorkspaceSnapshot::capture(&workdir).unwrap();
        fs::write(&source_path, "pub fn value() -> i32 {\n    2\n}\n").unwrap();
        let after = WorkspaceSnapshot::capture(&workdir).unwrap();
        let delta = before.delta(&after).unwrap();
        let descriptor = CodeRunDescriptor {
            task: "change value".to_string(),
            skill: None,
            profile: "profile@sha256:test".to_string(),
            model_config: None,
            tool_config: None,
            extra: BTreeMap::new(),
        };
        let artifact =
            build_code_run_artifact(descriptor, &before, &after, delta, Vec::new(), None, None)
                .unwrap();
        let output = json!({"edit": {"final_success": true}});
        write_code_run_artifact(&artifact_dir, &artifact, &output, None).unwrap();

        fs::write(&source_path, "pub fn value() -> i32 {\n    1\n}\n").unwrap();
        let replayed = replay_code_run_artifact(&artifact_dir, &workdir).unwrap();

        assert_eq!(replayed, output);
        assert_eq!(
            fs::read_to_string(&source_path).unwrap(),
            "pub fn value() -> i32 {\n    2\n}\n"
        );

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn workspace_snapshot_ignores_nested_air_tool_outputs() {
        let temp_dir = unique_temp_dir("air-code-artifact-ignore-air");
        let workdir = temp_dir.join("work");
        fs::create_dir_all(workdir.join("nested/.air/tool-output")).unwrap();
        fs::create_dir_all(workdir.join("src")).unwrap();
        fs::write(workdir.join("src/lib.rs"), "pub fn value() -> i32 { 1 }\n").unwrap();
        fs::write(
            workdir.join("nested/.air/tool-output/bash.log"),
            "large tool log\n",
        )
        .unwrap();

        let snapshot = WorkspaceSnapshot::capture(&workdir).unwrap();
        let files = snapshot.summary().files;

        assert!(files.iter().any(|file| file.path == "src/lib.rs"));
        assert!(files.iter().all(|file| !file.path.contains("/.air/")));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn code_run_verdict_ignores_model_success_without_verification_trace() {
        let temp_dir = unique_temp_dir("air-code-verdict-no-verification");
        fs::create_dir_all(&temp_dir).unwrap();
        let trace_path = temp_dir.join("trace.jsonl");
        fs::write(
            &trace_path,
            "{\"agent\":\"code\",\"step\":1,\"rule\":\"done\",\"action\":\"return\",\"status\":\"ok\"}\n",
        )
        .unwrap();
        let delta = WorkspaceDelta {
            changed_files: vec!["src/lib.rs".to_string()],
            diff: "+pub fn helper() {}\n".to_string(),
        };
        let outputs = json!({"edit": {"final_success": true}});

        let verdict = derive_code_run_verdict(
            &outputs,
            &delta,
            Some(&trace_path),
            &CodeRunVerdictConstraints::code_edit(),
        );

        assert!(verdict.patch_applied);
        assert!(!verdict.verification_ran);
        assert!(!verdict.final_success);
        assert_eq!(
            verdict.failure_reason.unwrap().category,
            FailureCategory::VerificationFailed
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn code_run_verdict_accepts_fresh_verification_and_patch() {
        let temp_dir = unique_temp_dir("air-code-verdict-passed");
        fs::create_dir_all(&temp_dir).unwrap();
        let trace_path = temp_dir.join("trace.jsonl");
        fs::write(
            &trace_path,
            "{\"agent\":\"code\",\"step\":1,\"rule\":\"act\",\"action\":\"tool_batch_dispatch_item\",\"status\":\"ok\",\"meta\":{\"tool\":\"bash\"},\"output\":{\"verification\":true,\"success\":true,\"workspace_changed\":false}}\n",
        )
        .unwrap();
        let delta = WorkspaceDelta {
            changed_files: vec!["src/lib.rs".to_string()],
            diff: "+pub fn normalize_key_char() {}\n".to_string(),
        };

        let verdict = derive_code_run_verdict(
            &json!({"edit": {"final_success": false}}),
            &delta,
            Some(&trace_path),
            &CodeRunVerdictConstraints {
                required_changed_files: vec!["src/lib.rs".to_string()],
                required_diff_contains: vec!["normalize_key_char".to_string()],
                ..CodeRunVerdictConstraints::code_edit()
            },
        );

        assert!(verdict.verification_ran);
        assert!(verdict.verification_passed);
        assert!(verdict.final_success);
        assert!(verdict.failure_reason.is_none());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn code_run_verdict_rejects_forbidden_file_changes() {
        let delta = WorkspaceDelta {
            changed_files: vec!["Cargo.toml".to_string()],
            diff: "+serde = \"1\"\n".to_string(),
        };
        let verdict = derive_code_run_verdict_from_facts(
            &delta,
            CodeRunVerificationFacts {
                verification_ran: true,
                verification_passed: true,
            },
            &CodeRunVerdictConstraints {
                forbidden_changed_files: vec!["Cargo.toml".to_string()],
                ..CodeRunVerdictConstraints::code_edit()
            },
        );

        assert!(!verdict.forbidden_files_ok);
        assert!(!verdict.final_success);
        assert_eq!(
            verdict.failure_reason.unwrap().category,
            FailureCategory::DiffConstraintFailed
        );
    }

    #[test]
    fn review_verdict_rejects_workspace_changes() {
        let delta = WorkspaceDelta {
            changed_files: vec!["src/lib.rs".to_string()],
            diff: "+review should not edit\n".to_string(),
        };
        let verdict = derive_code_run_verdict_from_facts(
            &delta,
            CodeRunVerificationFacts::default(),
            &CodeRunVerdictConstraints::review(),
        );

        assert!(!verdict.workspace_clean_ok);
        assert!(!verdict.final_success);
        assert_eq!(
            verdict.failure_reason.unwrap().category,
            FailureCategory::PolicyViolation
        );
    }

    #[test]
    fn code_run_artifact_copies_subagent_trace_and_rewrites_parent_trace() {
        let temp_dir = unique_temp_dir("air-code-artifact-subagent");
        let workdir = temp_dir.join("work");
        let artifact_dir = temp_dir.join("artifact");
        let source_path = workdir.join("src/lib.rs");
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, "pub fn value() -> i32 { 1 }\n").unwrap();

        let child_dir = workdir.join(".air/subagents/task-1");
        fs::create_dir_all(&child_dir).unwrap();
        let child_trace = child_dir.join("trace.jsonl");
        let child_output = child_dir.join("output.json");
        fs::write(
            &child_trace,
            "{\"agent\":\"child\",\"step\":1,\"rule\":\"choose\",\"action\":\"model_call\",\"status\":\"ok\"}\n",
        )
        .unwrap();
        fs::write(&child_output, "{\"result\":\"handoff\"}\n").unwrap();

        let parent_trace = temp_dir.join("trace.jsonl");
        fs::write(
            &parent_trace,
            format!(
                "{{\"agent\":\"parent\",\"step\":1,\"rule\":\"act\",\"action\":\"tool_batch_dispatch_item\",\"status\":\"ok\",\"output\":{{\"child_trace_path\":\"{}\",\"child_output_path\":\"{}\"}}}}\n",
                child_trace.display(),
                child_output.display()
            ),
        )
        .unwrap();

        let before = WorkspaceSnapshot::capture(&workdir).unwrap();
        let after = WorkspaceSnapshot::capture(&workdir).unwrap();
        let delta = before.delta(&after).unwrap();
        let descriptor = CodeRunDescriptor {
            task: "inspect".to_string(),
            skill: None,
            profile: "profile@sha256:test".to_string(),
            model_config: None,
            tool_config: None,
            extra: BTreeMap::new(),
        };
        let artifact =
            build_code_run_artifact(descriptor, &before, &after, delta, Vec::new(), None, None)
                .unwrap();

        write_code_run_artifact(
            &artifact_dir,
            &artifact,
            &json!({"edit": {}}),
            Some(&parent_trace),
        )
        .unwrap();

        let copied_child_trace = artifact_dir.join("subagents/0/trace.jsonl");
        let copied_child_output = artifact_dir.join("subagents/0/output.json");
        assert!(copied_child_trace.exists());
        assert!(copied_child_output.exists());
        let copied_parent_trace = fs::read_to_string(artifact_dir.join("trace.jsonl")).unwrap();
        assert!(copied_parent_trace.contains(&copied_child_trace.display().to_string()));
        assert!(copied_parent_trace.contains(&copied_child_output.display().to_string()));
        assert!(!copied_parent_trace.contains(&child_trace.display().to_string()));

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn timeout_artifact_records_timeout_verdict() {
        let temp_dir = unique_temp_dir("air-code-artifact-timeout");
        let artifact_dir = temp_dir.join("artifact");

        write_timeout_code_run_artifact(&artifact_dir, "slow task", 1).unwrap();
        let artifact = read_code_run_artifact(&artifact_dir).unwrap();

        assert_eq!(artifact.task, "slow task");
        assert_eq!(
            artifact.failure_reason.as_ref().unwrap().category,
            FailureCategory::Timeout
        );
        assert_eq!(
            artifact
                .verdict
                .as_ref()
                .unwrap()
                .failure_reason
                .as_ref()
                .unwrap()
                .category,
            FailureCategory::Timeout
        );
        assert!(!artifact.verdict.as_ref().unwrap().final_success);
        assert!(artifact_dir.join("trace.jsonl").exists());
        let _ = fs::remove_dir_all(temp_dir);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }
}
