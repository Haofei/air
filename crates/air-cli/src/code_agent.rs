use crate::models::ModelReplayOptions;
use crate::run_plan::{run_plan_capture, RunPlanOptions};
use air_code_artifact::{
    build_code_run_artifact, code_run_path_rewrites, code_run_trace_path, derive_code_run_verdict,
    git_changed_files, patch_code_output_with_workspace_delta, patch_trace_return,
    path_content_identity, replay_code_run_artifact, write_code_run_artifact, CodeRunDescriptor,
    CodeRunSkill, CodeRunVerdictConstraints, WorkspaceSnapshot,
};
use anyhow::{bail, Context, Result};
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
    let mut outputs = run_plan_capture(RunPlanOptions {
        plan: None,
        profile: Some(profile),
        store: None,
        input: None,
        input_values: Some(input),
        model_config,
        trace_out: effective_trace_out.clone(),
        trace_redact,
        trace_raw,
        state_out: None,
        checkpoint_out: None,
        jit_cache: None,
        parallel: false,
        log,
        example_tools: false,
        tool_config,
        model_replay,
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
    use std::fs;

    fn code_edit_loop_module() -> serde_yaml::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("skills/code-agent/code-edit-loop.air.yaml");
        serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

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
                "bootstrap-approval",
                "bootstrap",
                "summarize-at-step-limit",
                "choose",
                "verify-command",
                "verify",
                "act",
                "act-missing-complete-decision",
                "act-missing-tool-calls-decision",
                "verify-act",
                "verify-act-missing-complete-decision",
                "verify-act-missing-tool-calls-decision",
                "verify-command-required",
                "verify-command-mutated-workspace",
                "tool-error-retry",
                "verification-passed-after-patch",
                "verification-passed",
                "verification-failed-again",
                "verification-failed",
                "verification-reset-after-write",
                "complete-needs-verification",
                "verify-complete-needs-command",
                "complete-verified",
                "complete-verified-needs-patch",
                "summarize-post-act-at-step-limit",
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
            vec![
                "question",
                "bash",
                "grep",
                "glob",
                "lsp",
                "task",
                "read_range",
                "read_contains",
                "edit",
                "webfetch",
                "todowrite",
                "todoread",
                "skill"
            ]
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
        assert_eq!(keys.len(), 4);
        assert!(input.get("task").is_some());
        assert!(input.get("observations").is_some());
        assert!(input.get("available_skills").is_some());
        assert!(input.get("allowed_tools").is_some());
        assert!(input.get("tool_schemas").is_none());
    }

    #[test]
    fn edit_loop_keeps_opencode_sized_observation_window() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();
        let choose = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("choose"))
            .unwrap();
        assert_eq!(
            choose["actions"][0]["input"]["object"]["observations"]["max_bytes"],
            serde_yaml::Value::Number(50000.into())
        );
        assert!(rules
            .iter()
            .all(|rule| rule["id"].as_str() != Some("choose-verification")));
    }

    #[test]
    fn edit_loop_summary_includes_untracked_files() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();
        let summarize = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("summarize"))
            .unwrap();
        let calls = summarize["actions"][0]["input"]["array"]
            .as_sequence()
            .unwrap();
        let diff_command = calls[0]["object"]["input"]["literal"]["command"]
            .as_str()
            .unwrap();
        let names_command = calls[1]["object"]["input"]["literal"]["command"]
            .as_str()
            .unwrap();

        assert!(diff_command.contains("git rev-parse --is-inside-work-tree"));
        assert!(diff_command.contains("git diff --no-index -- /dev/null"));
        assert!(diff_command.contains("git diff -- ."));
        assert!(diff_command.contains("git ls-files --others --exclude-standard -- ."));
        assert!(diff_command.contains("AIR non-git workspace"));
        assert!(names_command.contains("git rev-parse --is-inside-work-tree"));
        assert!(names_command.contains("git diff --name-only -- ."));
        assert!(names_command.contains("git ls-files --others --exclude-standard -- ."));
        assert!(names_command.contains("find . -type f"));
        assert!(names_command.contains("-print | sed 's#^\\./##' | sort"));
        assert!(!names_command.contains("shasum -a 256"));

        let bootstrap = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("bootstrap"))
            .unwrap();
        let baseline_command = bootstrap["actions"][0]["input"]["array"][0]["object"]["input"]
            ["literal"]["command"]
            .as_str()
            .unwrap();
        assert!(baseline_command.contains("-print | sed 's#^\\./##' | sort"));
        assert!(!baseline_command.contains("shasum -a 256"));
    }

    #[test]
    fn edit_loop_keeps_model_driven_after_tool_execution() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();
        let act = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("act"))
            .unwrap();
        let continue_after_act = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("continue-after-act"))
            .unwrap();

        assert_eq!(
            act["actions"][2]["values"]["phase"],
            serde_yaml::Value::String("post_act".to_string())
        );
        assert!(act["actions"][0].get("allowed_tools").is_some());
        assert_eq!(
            continue_after_act["actions"][0]["values"]["phase"],
            serde_yaml::Value::String("choose".to_string())
        );
        for removed_rule in ["edit-applied", "write-applied", "apply-patch-applied"] {
            assert!(rules
                .iter()
                .all(|rule| rule["id"].as_str() != Some(removed_rule)));
        }
        assert!(rules
            .iter()
            .all(|rule| rule["id"].as_str() != Some("continue-verification-after-tool-update")));
    }

    #[test]
    fn edit_loop_recognizes_batched_bash_verification() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();
        for id in ["verification-passed", "verification-failed"] {
            let rule = rules
                .iter()
                .find(|rule| rule["id"].as_str() == Some(id))
                .unwrap();
            let condition = rule["when"].as_str().unwrap();
            assert!(condition.contains("observation_summary.verification_status_update"));
            assert!(!condition.contains("observation[0]"));
            assert!(!condition.contains("observation[1]"));
        }

        let verification_passed = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("verification-passed"))
            .unwrap();
        assert_eq!(
            verification_passed["actions"][0]["values"]["verification_status"],
            serde_yaml::Value::String("passed".to_string())
        );
        assert_eq!(
            verification_passed["actions"][0]["values"]["verification_required"],
            serde_yaml::Value::Bool(false)
        );
        assert_eq!(
            verification_passed["actions"][0]["values"]["phase"],
            serde_yaml::Value::String("choose".to_string())
        );

        let reset_after_write = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("verification-reset-after-write"))
            .unwrap();
        assert!(reset_after_write["when"]
            .as_str()
            .unwrap()
            .contains("verification_required != true"));
        assert!(reset_after_write["when"]
            .as_str()
            .unwrap()
            .contains("verification_status_update == \"unknown\""));
        assert_eq!(
            reset_after_write["actions"][0]["values"]["verification_status"],
            serde_yaml::Value::String("unknown".to_string())
        );

        let ids = rules
            .iter()
            .map(|rule| rule["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        let tool_error = ids.iter().position(|id| *id == "tool-error-retry").unwrap();
        let passed = ids
            .iter()
            .position(|id| *id == "verification-passed")
            .unwrap();
        assert!(tool_error < passed);
        let tool_error_rule = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("tool-error-retry"))
            .unwrap();
        assert!(tool_error_rule["when"]
            .as_str()
            .unwrap()
            .contains("verification_status_update != \"failed\""));
    }

    #[test]
    fn edit_loop_requires_verification_before_completion() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();
        let phase_enum = module["state"]["phase"]["enum"].as_sequence().unwrap();
        assert!(phase_enum.contains(&serde_yaml::Value::String("bootstrap_approval".to_string())));
        assert!(phase_enum.contains(&serde_yaml::Value::String("bootstrap".to_string())));
        assert!(phase_enum.contains(&serde_yaml::Value::String("verify".to_string())));
        assert!(phase_enum.contains(&serde_yaml::Value::String("verify_act".to_string())));

        let needs_verification = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("complete-needs-verification"))
            .unwrap();
        let needs_verification_condition = needs_verification["when"].as_str().unwrap();
        assert!(needs_verification_condition.contains("verification_status != \"passed\""));
        assert!(!needs_verification_condition.contains("patch_applied != true"));
        assert_eq!(
            needs_verification["actions"][1]["values"]["phase"],
            serde_yaml::Value::String("verify".to_string())
        );
        assert_eq!(
            needs_verification["actions"][1]["values"]["verification_required"],
            serde_yaml::Value::Bool(true)
        );

        let verify = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("verify"))
            .unwrap();
        assert!(verify["when"]
            .as_str()
            .unwrap()
            .contains("verification_command == \"\""));
        assert_eq!(
            verify["actions"][0]["input"]["object"]["verification_gate"]["literal"]["required"],
            serde_yaml::Value::Bool(true)
        );
        assert_eq!(
            verify["actions"][1]["values"]["phase"],
            serde_yaml::Value::String("verify_act".to_string())
        );

        let verify_command = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("verify-command"))
            .unwrap();
        assert!(verify_command["when"]
            .as_str()
            .unwrap()
            .contains("verification_command != \"\""));
        assert_eq!(
            verify_command["actions"][0]["allowed_tools"],
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("bash".to_string())])
        );
        assert_eq!(
            verify_command["actions"][0]["input"]["array"][0]["object"]["input"]["object"]
                ["command"]["ref"],
            serde_yaml::Value::String("verification_command".to_string())
        );

        let verify_act = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("verify-act"))
            .unwrap();
        assert_eq!(
            verify_act["actions"][0]["allowed_tools"],
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("bash".to_string())])
        );

        let verify_command_required = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("verify-command-required"))
            .unwrap();
        let verify_command_required_condition = verify_command_required["when"].as_str().unwrap();
        assert!(verify_command_required_condition.contains("verification_required == true"));
        assert!(verify_command_required_condition
            .contains("verification_status_update == \"unchanged\""));
        assert_eq!(
            verify_command_required["actions"][1]["values"]["phase"],
            serde_yaml::Value::String("summarize".to_string())
        );

        let verify_command_mutated = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("verify-command-mutated-workspace"))
            .unwrap();
        let verify_command_mutated_condition = verify_command_mutated["when"].as_str().unwrap();
        assert!(verify_command_mutated_condition.contains("verification_required == true"));
        assert!(
            verify_command_mutated_condition.contains("verification_status_update == \"unknown\"")
        );
        assert_eq!(
            verify_command_mutated["actions"][1]["values"]["phase"],
            serde_yaml::Value::String("summarize".to_string())
        );
        let ids = rules
            .iter()
            .map(|rule| rule["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        let mutated = ids
            .iter()
            .position(|id| *id == "verify-command-mutated-workspace")
            .unwrap();
        let reset = ids
            .iter()
            .position(|id| *id == "verification-reset-after-write")
            .unwrap();
        assert!(mutated < reset);

        let complete_verified = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("complete-verified"))
            .unwrap();
        let complete_verified_condition = complete_verified["when"].as_str().unwrap();
        assert!(complete_verified_condition.contains("verification_status == \"passed\""));
        assert!(!complete_verified_condition.contains("patch_applied == true"));
    }

    #[test]
    fn edit_loop_step_limit_intercepts_non_choose_phases() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();
        let summarize_at_step_limit = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("summarize-at-step-limit"))
            .unwrap();
        let condition = summarize_at_step_limit["when"].as_str().unwrap();
        assert!(condition.contains("phase != \"init\""));
        assert!(condition.contains("phase != \"bootstrap_approval\""));
        assert!(condition.contains("phase != \"bootstrap\""));
        assert!(condition.contains("phase != \"post_act\""));
        assert!(condition.contains("phase != \"summarize\""));
        assert!(condition.contains("phase != \"done\""));
        assert!(condition.contains("_air.is_last_action_step == true"));

        let summarize_post_act = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("summarize-post-act-at-step-limit"))
            .unwrap();
        assert_eq!(
            summarize_post_act["actions"][1]["values"]["phase"],
            serde_yaml::Value::String("summarize".to_string())
        );
    }

    #[test]
    fn edit_loop_derives_patch_applied_from_final_diff() {
        let module = code_edit_loop_module();
        let state = module["state"].as_mapping().unwrap();
        assert!(!state.contains_key("patch_applied"));

        let summarize = module["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("summarize"))
            .unwrap();
        let patch_applied = &summarize["actions"][1]["values"]["edit"]["object"]["patch_applied"];
        assert!(patch_applied.get("not").is_some());
        assert!(patch_applied["not"].get("is_empty").is_some());
        let patch_lines = patch_applied["not"]["is_empty"]["line_difference"]
            .as_sequence()
            .unwrap();
        assert_eq!(
            patch_lines[0]["split_lines"]["ref"],
            serde_yaml::Value::String("final_diff_result[1].output.log".to_string())
        );
        assert_eq!(
            patch_lines[1]["split_lines"]["ref"],
            serde_yaml::Value::String("preexisting_diff_result[0].output.log".to_string())
        );
        let changed_files = &summarize["actions"][1]["values"]["edit"]["object"]["changed_files"];
        assert!(changed_files.get("line_difference").is_some());
        let complete_needs_patch = module["workflow"]["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|rule| rule["id"].as_str() == Some("complete-verified-needs-patch"))
            .unwrap();
        assert!(complete_needs_patch["when"]
            .as_str()
            .unwrap()
            .contains("requires_edit == true"));
        let preexisting =
            &summarize["actions"][1]["values"]["edit"]["object"]["preexisting_changed_files"];
        assert_eq!(
            preexisting["path_objects"]["split_lines"]["ref"],
            serde_yaml::Value::String("preexisting_diff_result[0].output.log".to_string())
        );
        let final_success = &summarize["actions"][1]["values"]["edit"]["object"]["final_success"];
        assert_eq!(
            final_success["equals"][1]["literal"],
            serde_yaml::Value::Sequence(vec![
                serde_yaml::Value::String("passed".to_string()),
                serde_yaml::Value::Bool(true),
            ])
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
