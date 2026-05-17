use crate::code_artifact::{
    build_code_run_artifact, code_run_path_rewrites, code_run_trace_path, git_changed_files,
    patch_code_output_with_workspace_delta, patch_trace_return, path_content_identity,
    replay_code_run_artifact, write_code_run_artifact, CodeRunDescriptor, FailureCategory,
    FailureReason, WorkspaceDelta, WorkspaceSnapshot,
};
use crate::models::ModelReplayOptions;
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::run_plan::{run_plan_capture, RunPlanOptions};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CODE_PROFILE: &str = "examples/code-agent/edit.air-profile.yaml";

pub(crate) struct CodeOptions {
    pub(crate) task: String,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
    pub(crate) explain: bool,
    pub(crate) tool_config: Option<PathBuf>,
    pub(crate) artifact_out: Option<PathBuf>,
    pub(crate) artifact_extra: BTreeMap<String, Value>,
    pub(crate) replay_artifact: Option<PathBuf>,
    pub(crate) replay_from: Option<usize>,
}

pub(crate) fn code(options: CodeOptions) -> Result<()> {
    let explain = options.explain;
    let outputs = run_code_agent(options)?;
    if !explain {
        println!("{}", serde_json::to_string_pretty(&outputs)?);
    }
    Ok(())
}

pub(crate) fn run_code_agent(options: CodeOptions) -> Result<Value> {
    let CodeOptions {
        task,
        profile,
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        log,
        explain,
        tool_config,
        artifact_out,
        artifact_extra,
        replay_artifact,
        replay_from,
    } = options;

    if task.trim().is_empty() {
        bail!("air code task must not be empty");
    }

    let profile = profile.unwrap_or_else(|| PathBuf::from(DEFAULT_CODE_PROFILE));
    let input = build_input(task);

    if explain {
        print_explain(&profile, &input)?;
        return Ok(Value::Null);
    }

    let cwd = std::env::current_dir().context("resolve current directory")?;
    if let Some(artifact_dir) = replay_artifact.as_ref().filter(|_| replay_from.is_none()) {
        return replay_code_run_artifact(artifact_dir, &cwd);
    }
    let before = WorkspaceSnapshot::capture(&cwd)?;
    let preexisting_changed_files = git_changed_files(&cwd).unwrap_or_default();
    let descriptor = CodeRunDescriptor {
        task: input
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
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
    let effective_trace_out = trace_out.clone().or_else(|| artifact_trace_path.clone());
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
    let failure_reason = derive_code_failure_reason(&outputs, &delta);
    patch_code_output_with_workspace_delta(
        &mut outputs,
        &delta,
        &preexisting_changed_files,
        failure_reason.as_ref(),
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
            failure_reason,
        )?;
        write_code_run_artifact(
            &artifact_dir,
            &artifact,
            &outputs,
            effective_trace_out.as_deref(),
        )?;
    }
    Ok(outputs)
}

fn print_explain(profile: &Path, input: &Map<String, Value>) -> Result<()> {
    let metadata = explain_metadata_for_profile(profile)?;
    let explanation = json!({
        "command": "code",
        "will_run": false,
        "profile": path_ref_to_input_string(profile),
        "plan": path_ref_to_input_string(&metadata.plan),
        "store": path_ref_to_input_string(&metadata.store),
        "capabilities": metadata.capabilities,
        "read_only": metadata.read_only,
        "writes_workspace": metadata.writes_workspace,
        "input": Value::Object(input.clone()),
    });
    serde_json::to_writer_pretty(std::io::stdout(), &explanation)?;
    println!();
    Ok(())
}

fn build_input(task: String) -> Map<String, Value> {
    let mut input = Map::new();
    input.insert("task".to_string(), Value::String(task));
    input
}

fn path_ref_to_input_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn derive_code_failure_reason(outputs: &Value, delta: &WorkspaceDelta) -> Option<FailureReason> {
    let edit = outputs.get("edit")?;
    if delta.changed_files.is_empty() {
        return Some(FailureReason {
            category: FailureCategory::NoPatchApplied,
            message: "code agent finished without changing workspace files".to_string(),
            details: BTreeMap::new(),
        });
    }
    if edit
        .get("final_success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    Some(FailureReason {
        category: FailureCategory::AgentError,
        message: "code agent did not report final_success=true".to_string(),
        details: BTreeMap::new(),
    })
}

struct CodeExplainMetadata {
    plan: PathBuf,
    store: PathBuf,
    capabilities: Vec<String>,
    read_only: bool,
    writes_workspace: bool,
}

fn explain_metadata_for_profile(profile: &Path) -> Result<CodeExplainMetadata> {
    let profile_path = profile.to_path_buf();
    let profile = read_run_plan_profile(&profile_path)?;
    let plan_path = resolve_profile_path(&profile_path, &profile.plan);
    let store_path = resolve_profile_path(&profile_path, &profile.store);
    let plan = air_linker::parse_run_plan_file(&plan_path)?;
    let mut capabilities = plan.requires.capabilities;
    capabilities.sort();
    capabilities.dedup();
    let writes_workspace = capabilities
        .iter()
        .any(|capability| capability == "file.write");
    Ok(CodeExplainMetadata {
        plan: plan_path,
        store: store_path,
        capabilities,
        read_only: !writes_workspace,
        writes_workspace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;

    fn code_edit_loop_module() -> serde_yaml::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/code-edit-loop.air.yaml");
        serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn code_input_is_just_the_task() {
        let input = build_input("refactor a helper".to_string());

        assert_eq!(input.len(), 1);
        assert_eq!(
            input["task"],
            Value::String("refactor a helper".to_string())
        );
    }

    #[test]
    fn default_code_profile_is_minimal_edit_profile() {
        assert_eq!(
            PathBuf::from(DEFAULT_CODE_PROFILE),
            PathBuf::from("examples/code-agent/edit.air-profile.yaml")
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
                "verify",
                "act",
                "verification-passed",
                "verification-failed",
                "verification-reset-after-write",
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
            vec![
                "question",
                "bash",
                "grep",
                "glob",
                "lsp",
                "task",
                "read",
                "edit",
                "write",
                "apply_patch",
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
        assert_eq!(keys.len(), 3);
        assert!(input.get("task").is_some());
        assert!(input.get("observations").is_some());
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
            serde_yaml::Value::Number(600000.into())
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

        assert!(diff_command.contains("git diff --no-index -- /dev/null"));
        assert!(diff_command.contains("git ls-files --others --exclude-standard"));
        assert!(names_command.contains("git diff --name-only --"));
        assert!(names_command.contains("git ls-files --others --exclude-standard"));
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
            .contains("verification_status_update == \"unknown\""));
        assert_eq!(
            reset_after_write["actions"][0]["values"]["verification_status"],
            serde_yaml::Value::String("unknown".to_string())
        );
    }

    #[test]
    fn edit_loop_requires_verification_before_completion() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();
        let phase_enum = module["state"]["phase"]["enum"].as_sequence().unwrap();
        assert!(phase_enum.contains(&serde_yaml::Value::String("verify".to_string())));

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

        let verify = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("verify"))
            .unwrap();
        assert_eq!(
            verify["actions"][0]["input"]["object"]["allowed_tools"]["literal"],
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("bash".to_string())])
        );

        let complete_verified = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("complete-verified"))
            .unwrap();
        let complete_verified_condition = complete_verified["when"].as_str().unwrap();
        assert!(complete_verified_condition.contains("verification_status == \"passed\""));
        assert!(!complete_verified_condition.contains("patch_applied == true"));
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
        assert_eq!(
            patch_applied["not"]["is_empty"]["split_lines"]["ref"],
            serde_yaml::Value::String("final_diff_result[1].output.log".to_string())
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

        patch_code_output_with_workspace_delta(&mut outputs, &delta, &preexisting, None);

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
        let failure = derive_code_failure_reason(&outputs, &delta);

        patch_code_output_with_workspace_delta(&mut outputs, &delta, &[], failure.as_ref());

        assert_eq!(outputs["edit"]["patch_applied"], json!(false));
        assert_eq!(outputs["edit"]["final_success"], json!(false));
        assert_eq!(
            outputs["edit"]["failure_reason"]["category"],
            json!("no_patch_applied")
        );
    }
}
