use crate::models::ModelReplayOptions;
use crate::native_loop::{run_native_loop, NativeLoopKind, NativeLoopOptions};
use air_code_artifact::{
    build_code_run_artifact, code_run_path_rewrites, code_run_trace_path, derive_code_run_verdict,
    git_changed_files, patch_code_output_with_workspace_delta, patch_trace_return,
    path_content_identity, replay_code_run_artifact, write_code_run_artifact, CodeRunDescriptor,
    CodeRunSkill, CodeRunVerdictConstraints, WorkspaceSnapshot,
};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const DEFAULT_CODE_PROFILE: &str = "skills/code-agent/edit.air-profile.yaml";

pub(crate) struct CodeOptions {
    pub(crate) task: String,
    pub(crate) verification_command: Option<String>,
    pub(crate) artifact_task: Option<String>,
    pub(crate) skill: Option<CodeRunSkill>,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
    pub(crate) tool_config: Option<PathBuf>,
    pub(crate) artifact_out: Option<PathBuf>,
    pub(crate) artifact_extra: BTreeMap<String, Value>,
    pub(crate) verdict_constraints: CodeRunVerdictConstraints,
    pub(crate) replay_artifact: Option<PathBuf>,
    pub(crate) replay_from: Option<usize>,
}

pub(crate) fn run_code_agent(options: CodeOptions) -> Result<Value> {
    let CodeOptions {
        task,
        verification_command,
        artifact_task,
        skill,
        profile,
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        log,
        tool_config,
        artifact_out,
        artifact_extra,
        verdict_constraints,
        replay_artifact,
        replay_from,
    } = options;

    if task.trim().is_empty() {
        bail!("air run task must not be empty");
    }

    let profile = profile.unwrap_or_else(default_code_profile_path);
    let profile_config = read_native_code_profile(&profile).ok();
    let model_config = model_config.or_else(|| {
        profile_config
            .as_ref()
            .and_then(|profile_config| profile_config.model_config.as_ref())
            .map(|path| resolve_profile_path(&profile, path))
    });
    let tool_config = tool_config.or_else(|| {
        profile_config
            .as_ref()
            .and_then(|profile_config| profile_config.tool_config.as_ref())
            .map(|path| resolve_profile_path(&profile, path))
    });
    let kind = native_loop_kind(skill.as_ref(), &profile);
    let requires_edit = task_requires_edit(&task);
    let input = build_input(task, verification_command, requires_edit);
    let mut verdict_constraints = verdict_constraints;
    if !requires_edit {
        verdict_constraints.require_patch = false;
    }
    let descriptor_task = artifact_task.unwrap_or_else(|| {
        input
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });

    let cwd = std::env::current_dir().context("resolve current directory")?;
    if let Some(artifact_dir) = replay_artifact.as_ref().filter(|_| replay_from.is_none()) {
        return replay_code_run_artifact(artifact_dir, &cwd);
    }
    let before = WorkspaceSnapshot::capture(&cwd)?;
    let preexisting_changed_files = git_changed_files(&cwd).unwrap_or_default();
    let descriptor = CodeRunDescriptor {
        task: descriptor_task,
        skill,
        profile: path_content_identity(&profile)?,
        model_config: model_config
            .as_ref()
            .map(|path| path_content_identity(path))
            .transpose()?,
        tool_config: tool_config
            .as_ref()
            .map(|path| path_content_identity(path))
            .transpose()?,
        extra: artifact_extra,
    };
    let artifact_trace_path = artifact_out.as_ref().map(|dir| dir.join("trace.jsonl"));
    if let Some(artifact_dir) = &artifact_out {
        fs::create_dir_all(artifact_dir)
            .with_context(|| format!("create {}", artifact_dir.display()))?;
    }
    let model_replay = replay_artifact
        .as_ref()
        .map(|artifact_dir| {
            Ok::<_, anyhow::Error>(ModelReplayOptions {
                trace: code_run_trace_path(artifact_dir)?,
                live_from_event: replay_from,
                path_rewrites: code_run_path_rewrites(artifact_dir, &cwd)?,
            })
        })
        .transpose()?;
    let implicit_trace_out = if trace_out.is_none() && artifact_trace_path.is_none() {
        Some(implicit_code_trace_path())
    } else {
        None
    };
    let effective_trace_out = trace_out
        .clone()
        .or_else(|| artifact_trace_path.clone())
        .or_else(|| implicit_trace_out.clone());
    let trace_out_for_patch = trace_out.clone();
    let trace_redact_for_patch = trace_redact || !trace_raw;
    let task_for_run = input
        .get("task")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let verification_command_for_run = input
        .get("verification_command")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let mut outputs = run_native_loop(NativeLoopOptions {
        kind,
        task: task_for_run,
        verification_command: verification_command_for_run,
        requires_edit,
        model_config,
        tool_config,
        model_replay,
        trace_out: effective_trace_out.clone(),
        trace_redact,
        trace_raw,
        log,
    })?;

    let after = WorkspaceSnapshot::capture(&cwd)?;
    let delta = before.delta(&after)?;
    let verdict = derive_code_run_verdict(
        &outputs,
        &delta,
        effective_trace_out.as_deref(),
        &verdict_constraints,
    );
    patch_code_output_with_workspace_delta(
        &mut outputs,
        &delta,
        &preexisting_changed_files,
        &verdict,
    );
    if let Some(trace_path) = trace_out_for_patch.or(artifact_trace_path) {
        patch_trace_return(&trace_path, &outputs, trace_redact_for_patch)?;
    }
    if let Some(artifact_dir) = artifact_out {
        let artifact = build_code_run_artifact(
            descriptor,
            &before,
            &after,
            delta,
            preexisting_changed_files,
            verdict.failure_reason.clone(),
            Some(verdict),
        )?;
        write_code_run_artifact(
            &artifact_dir,
            &artifact,
            &outputs,
            effective_trace_out.as_deref(),
        )?;
    }
    if let Some(path) = implicit_trace_out {
        let _ = fs::remove_file(path);
    }
    Ok(outputs)
}

fn default_code_profile_path() -> PathBuf {
    if let Some(path) = find_default_code_profile_from_exe() {
        return path;
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(path) = find_default_code_profile_from(&cwd) {
            return path;
        }
    }
    #[cfg(debug_assertions)]
    {
        let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(DEFAULT_CODE_PROFILE);
        if dev_path.exists() {
            return dev_path;
        }
    }
    PathBuf::from(DEFAULT_CODE_PROFILE)
}

fn find_default_code_profile_from_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let start = exe.parent()?;
    find_default_code_profile_from(start)
}

fn find_default_code_profile_from(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(DEFAULT_CODE_PROFILE);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn implicit_code_trace_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "air-code-trace-{}-{nanos}.jsonl",
        std::process::id()
    ))
}

fn native_loop_kind(skill: Option<&CodeRunSkill>, profile: &Path) -> NativeLoopKind {
    if let Some(skill) = skill {
        match skill.id.as_str() {
            "review-agent" => return NativeLoopKind::Review,
            "bench-agent" => return NativeLoopKind::Bench,
            "project-scout" | "project-scout-agent" => return NativeLoopKind::ProjectScout,
            _ => {}
        }
    }
    match profile
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
    {
        "explore.air-profile.yaml" => NativeLoopKind::Explore,
        "project-scout.air-profile.yaml" => NativeLoopKind::ProjectScout,
        "review.air-profile.yaml" => NativeLoopKind::Review,
        "bench.air-profile.yaml" => NativeLoopKind::Bench,
        _ => NativeLoopKind::CodeEdit,
    }
}

#[derive(Debug, Deserialize)]
struct NativeCodeProfile {
    #[serde(default)]
    model_config: Option<PathBuf>,

    #[serde(default)]
    tool_config: Option<PathBuf>,
}

fn read_native_code_profile(path: &Path) -> Result<NativeCodeProfile> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read native code profile {}", path.display()))?;
    serde_yaml::from_str(&source).with_context(|| {
        format!(
            "failed to parse native code profile YAML {}",
            path.display()
        )
    })
}

fn resolve_profile_path(profile_path: &Path, path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path.clone();
    }
    profile_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(path)
}

pub(crate) fn build_input(
    task: String,
    verification_command: Option<String>,
    requires_edit: bool,
) -> Map<String, Value> {
    let mut input = Map::new();
    input.insert("task".to_string(), Value::String(task));
    input.insert(
        "verification_command".to_string(),
        Value::String(verification_command.unwrap_or_default()),
    );
    input.insert("requires_edit".to_string(), Value::Bool(requires_edit));
    input
}

fn task_requires_edit(task: &str) -> bool {
    let normalized = task.to_ascii_lowercase();
    let verification_only_markers = [
        "run tests",
        "run the tests",
        "run cargo test",
        "run npm test",
        "run pytest",
        "run verification",
        "verify only",
        "verify the build",
        "check the build",
        "check whether",
        "does it build",
        "without changing",
        "without editing",
        "do not change",
        "don't change",
        "no code changes",
        "no changes",
    ];
    !verification_only_markers
        .iter()
        .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use air_code_artifact::WorkspaceDelta;
    use serde_json::{json, Value};

    #[test]
    fn code_input_is_just_the_task() {
        let input = build_input("refactor a helper".to_string(), None, true);

        assert_eq!(input.len(), 3);
        assert_eq!(
            input["task"],
            Value::String("refactor a helper".to_string())
        );
        assert_eq!(input["verification_command"], Value::String(String::new()));
        assert_eq!(input["requires_edit"], Value::Bool(true));
        assert!(!task_requires_edit("run tests without changing files"));
        assert!(task_requires_edit("fix the parser failure"));
    }

    #[test]
    fn default_code_profile_is_minimal_edit_profile() {
        assert_eq!(
            PathBuf::from(DEFAULT_CODE_PROFILE),
            PathBuf::from("skills/code-agent/edit.air-profile.yaml")
        );
    }

    #[test]
    fn profile_selects_native_loop_kind() {
        assert_eq!(
            native_loop_kind(
                None,
                Path::new("skills/code-agent/explore.air-profile.yaml")
            ),
            NativeLoopKind::Explore
        );
        assert_eq!(
            native_loop_kind(
                None,
                Path::new("skills/review-agent/review.air-profile.yaml")
            ),
            NativeLoopKind::Review
        );
        assert_eq!(
            native_loop_kind(None, Path::new("skills/code-agent/edit.air-profile.yaml")),
            NativeLoopKind::CodeEdit
        );
    }

    #[test]
    fn code_output_workspace_diff_uses_run_delta() {
        let mut outputs = json!({
            "edit": {
                "changed_files": ["preexisting.rs"],
                "workspace_changed_files": ["preexisting.rs"],
                "preexisting_changed_files": [],
                "patch_applied": true,
                "workspace_diff": {
                    "provider": "bash",
                    "diff": "preexisting diff"
                },
                "final_success": true
            }
        });
        let delta = WorkspaceDelta {
            changed_files: vec!["crates/air-tools/src/lib.rs".to_string()],
            diff: "--- a/crates/air-tools/src/lib.rs\n+++ b/crates/air-tools/src/lib.rs\n"
                .to_string(),
        };
        let preexisting = vec!["crates/air-cli/src/code_agent.rs".to_string()];

        let verdict = derive_code_run_verdict(
            &outputs,
            &delta,
            None,
            &CodeRunVerdictConstraints::code_edit(),
        );

        patch_code_output_with_workspace_delta(&mut outputs, &delta, &preexisting, &verdict);

        let edit = &outputs["edit"];
        assert_eq!(
            edit["changed_files"],
            json!(["crates/air-tools/src/lib.rs"])
        );
        assert_eq!(
            edit["preexisting_changed_files"],
            json!([{"path": "crates/air-cli/src/code_agent.rs"}])
        );
        assert_eq!(edit["patch_applied"], json!(true));
        assert_eq!(edit["final_success"], json!(true));
        assert_eq!(
            edit["workspace_diff"]["provider"],
            json!("air-code-artifact")
        );
        assert!(edit["workspace_diff"]["diff"]
            .as_str()
            .unwrap()
            .contains("crates/air-tools/src/lib.rs"));
    }

    #[test]
    fn code_output_marks_no_patch_as_failed_even_when_verified() {
        let mut outputs = json!({
            "edit": {
                "changed_files": [],
                "workspace_changed_files": [],
                "preexisting_changed_files": [],
                "patch_applied": false,
                "workspace_diff": {
                    "provider": "bash",
                    "diff": ""
                },
                "final_success": true
            }
        });
        let delta = WorkspaceDelta {
            changed_files: vec![],
            diff: String::new(),
        };
        let verdict = derive_code_run_verdict(
            &outputs,
            &delta,
            None,
            &CodeRunVerdictConstraints::code_edit(),
        );

        patch_code_output_with_workspace_delta(&mut outputs, &delta, &[], &verdict);

        assert_eq!(outputs["edit"]["patch_applied"], json!(false));
        assert_eq!(outputs["edit"]["final_success"], json!(false));
        assert_eq!(
            outputs["edit"]["failure_reason"]["category"],
            json!("no_patch_applied")
        );
    }
}
