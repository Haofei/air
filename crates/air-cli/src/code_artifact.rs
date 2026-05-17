use crate::run_plan::write_trace;
use air_runtime::{read_trace_jsonl, system_return_event};
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
    pub(crate) profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model_config: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_config: Option<String>,
    pub(crate) mode: CodeRunMode,
    pub(crate) snapshot: CodeRunSnapshot,
    pub(crate) delta: WorkspaceDelta,
    pub(crate) files: CodeRunArtifactFiles,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) failure_reason: Option<FailureReason>,
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
    failure_reason: Option<&FailureReason>,
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
        Value::Bool(!delta.changed_files.is_empty()),
    );
    if edit
        .get("final_success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && delta.changed_files.is_empty()
    {
        edit.insert("final_success".to_string(), Value::Bool(false));
    }
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
    if let Some(reason) = failure_reason {
        edit.insert("failure_reason".to_string(), json!(reason));
    }
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
) -> Result<CodeRunArtifact> {
    let fingerprint = descriptor.fingerprint(before)?;
    Ok(CodeRunArtifact {
        schema: CODE_RUN_ARTIFACT_SCHEMA.to_string(),
        fingerprint,
        task: descriptor.task,
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
        files: CodeRunArtifactFiles {
            output: "output.json".to_string(),
            trace: "trace.jsonl".to_string(),
            diff: "diff.patch".to_string(),
        },
        failure_reason,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct CodeRunDescriptor {
    pub(crate) task: String,
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
            if same_file(trace_path, &destination) {
                return Ok(());
            }
            fs::copy(trace_path, &destination).with_context(|| {
                format!("copy {} to {}", trace_path.display(), destination.display())
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
            profile: "profile@sha256:test".to_string(),
            model_config: None,
            tool_config: None,
            extra: BTreeMap::new(),
        };
        let artifact =
            build_code_run_artifact(descriptor, &before, &after, delta, Vec::new(), None).unwrap();
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
