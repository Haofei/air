use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::run_plan::{run_plan, RunPlanOptions};
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
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
}

pub(crate) fn code(options: CodeOptions) -> Result<()> {
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
    } = options;

    if task.trim().is_empty() {
        bail!("air code task must not be empty");
    }

    let profile = profile.unwrap_or_else(|| PathBuf::from(DEFAULT_CODE_PROFILE));
    let input = build_input(task);

    if explain {
        print_explain(&profile, &input)?;
        return Ok(());
    }

    run_plan(RunPlanOptions {
        plan: None,
        profile: Some(profile),
        store: None,
        input: None,
        input_values: Some(input),
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        state_out: None,
        checkpoint_out: None,
        jit_cache: None,
        parallel: false,
        log,
        example_tools: false,
        tool_config,
    })
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
                "choose-verification",
                "act",
                "edit-applied",
                "write-applied",
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
            vec![
                "question",
                "bash",
                "read",
                "glob",
                "grep",
                "edit",
                "write",
                "task",
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
        for id in ["choose", "choose-verification"] {
            let rule = rules
                .iter()
                .find(|rule| rule["id"].as_str() == Some(id))
                .unwrap();
            assert_eq!(
                rule["actions"][0]["input"]["object"]["observations"]["max_bytes"],
                serde_yaml::Value::Number(600000.into())
            );
        }
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
    fn edit_loop_keeps_multi_edit_refactors_in_choose_phase() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();
        let edit_applied = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("edit-applied"))
            .unwrap();
        let write_applied = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("write-applied"))
            .unwrap();

        assert_eq!(
            edit_applied["actions"][0]["values"]["phase"],
            serde_yaml::Value::String("choose".to_string())
        );
        assert_eq!(
            write_applied["actions"][0]["values"]["phase"],
            serde_yaml::Value::String("choose".to_string())
        );
        assert!(rules
            .iter()
            .all(|rule| rule["id"].as_str() != Some("continue-verification-after-tool-update")));
    }
}
