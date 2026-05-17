use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::run_plan::{run_plan_capture, write_trace, RunPlanOptions};
use air_runtime::{read_trace_jsonl, system_return_event};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

    let cwd = std::env::current_dir().context("resolve current directory")?;
    let before = WorkspaceSnapshot::capture(&cwd)?;
    let preexisting_changed_files = git_changed_files(&cwd).unwrap_or_default();
    let trace_out_for_patch = trace_out.clone();
    let trace_redact_for_patch = trace_redact || !trace_raw;

    let mut outputs = run_plan_capture(RunPlanOptions {
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
    })?;

    let after = WorkspaceSnapshot::capture(&cwd)?;
    let delta = before.delta(&after)?;
    patch_code_output_with_workspace_delta(&mut outputs, &delta, &preexisting_changed_files);
    if let Some(trace_path) = trace_out_for_patch {
        patch_trace_return(&trace_path, &outputs, trace_redact_for_patch)?;
    }
    println!("{}", serde_json::to_string_pretty(&outputs)?);
    Ok(())
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

#[derive(Debug, Clone, Default)]
struct WorkspaceSnapshot {
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone)]
struct WorkspaceDelta {
    changed_files: Vec<String>,
    diff: String,
}

impl WorkspaceSnapshot {
    fn capture(root: &Path) -> Result<Self> {
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

    fn delta(&self, after: &Self) -> Result<WorkspaceDelta> {
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
}

fn patch_code_output_with_workspace_delta(
    outputs: &mut Value,
    delta: &WorkspaceDelta,
    preexisting_changed_files: &[String],
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
    edit.insert(
        "workspace_diff".to_string(),
        json!({
            "provider": "air-code-baseline",
            "command": "workspace snapshot diff",
            "success": true,
            "diff": delta.diff,
            "bytes": delta.diff.len(),
            "truncated": false,
        }),
    );
}

fn patch_trace_return(path: &Path, outputs: &Value, trace_redact: bool) -> Result<()> {
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

fn git_changed_files(root: &Path) -> Result<Vec<String>> {
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
                "act",
                "verification-passed",
                "verification-failed",
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
                "apply_patch",
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
            assert!(condition.contains("observation[0].tool == \"bash\""));
            assert!(condition.contains("observation[1].tool == \"bash\""));
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
    }

    #[test]
    fn edit_loop_requires_verification_before_completion() {
        let module = code_edit_loop_module();
        let rules = module["workflow"]["rules"].as_sequence().unwrap();

        let needs_verification = rules
            .iter()
            .find(|rule| rule["id"].as_str() == Some("complete-needs-verification"))
            .unwrap();
        let needs_verification_condition = needs_verification["when"].as_str().unwrap();
        assert!(needs_verification_condition.contains("verification_status != \"passed\""));
        assert!(!needs_verification_condition.contains("patch_applied != true"));

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

        patch_code_output_with_workspace_delta(&mut outputs, &delta, &preexisting);

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
        assert_eq!(
            edit["workspace_diff"]["provider"],
            json!("air-code-baseline")
        );
        assert!(edit["workspace_diff"]["diff"]
            .as_str()
            .unwrap()
            .contains("crates/air-tools/src/lib.rs"));
    }
}
