use crate::code_agent::{run_code_agent, CodeOptions};
use crate::code_artifact::{
    build_code_run_artifact, read_code_run_artifact, write_code_run_artifact, CodeRunDescriptor,
    FailureCategory, FailureReason, WorkspaceSnapshot,
};
use crate::models::ModelProviderChoice;
use crate::run_plan::{run_plan_capture, RunPlanOptions};
use crate::skill::{
    prepare_skill_composition, resolve_skill_run_metadata, route_skills_value_for_task,
};
use air_runtime::ModelProvider;
use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod manifest;
use manifest::*;
pub(crate) use manifest::{
    default_project_file, BenchProjectOptions, ProjectPlanOptions, ProjectRunOptions,
    ProjectStatusOptions, ProjectVerifyOptions,
};

fn project_manifest_dir(cwd: &Path, output: Option<&Path>) -> Result<PathBuf> {
    let manifest_path = output
        .map(|path| absolutize(cwd, path.to_path_buf()))
        .unwrap_or_else(|| cwd.join(DEFAULT_PROJECT_FILE));
    Ok(manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("project manifest path has no parent"))?
        .to_path_buf())
}

fn project_defaults_for_manifest(
    cwd: &Path,
    manifest_dir: &Path,
    model_config: Option<&Path>,
) -> Result<ProjectDefaults> {
    let model_config = model_config
        .map(|path| absolutize(cwd, path.to_path_buf()))
        .unwrap_or_else(|| cwd.join(DEFAULT_MODEL_CONFIG));
    Ok(ProjectDefaults {
        profile: path_for_manifest(manifest_dir, &cwd.join(DEFAULT_CODE_PROFILE))?,
        model_config: path_for_manifest(manifest_dir, &model_config)?,
        tool_config: path_for_manifest(manifest_dir, &cwd.join(DEFAULT_TOOL_CONFIG))?,
        artifact_dir: PathBuf::from(DEFAULT_ARTIFACT_DIR),
    })
}

pub(crate) fn project_plan(options: ProjectPlanOptions) -> Result<()> {
    if options.goal.trim().is_empty() {
        bail!("air project plan goal must not be empty");
    }
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let manifest_dir = project_manifest_dir(&cwd, options.output.as_deref())?;
    let defaults =
        project_defaults_for_manifest(&cwd, &manifest_dir, options.model_config.as_deref())?;
    let manifest = if options.template {
        project_template_manifest(options.goal, defaults)
    } else {
        let model_config = options
            .model_config
            .as_ref()
            .map(|path| absolutize(&cwd, path.clone()))
            .unwrap_or_else(|| cwd.join(DEFAULT_MODEL_CONFIG));
        eprintln!("[air project] exploring repository for project plan");
        let exploration = project_explorer_handoff(&options.goal, &cwd, &model_config)?;
        eprintln!("[air project] calling project planner model");
        let request = project_planner_request(&options.goal, &cwd, &exploration)?;
        let response =
            call_project_planner_model(model_config, &options.planner_model, &request)
                .with_context(|| format!("call project planner model {}", options.planner_model))?;
        let mut manifest = parse_project_planner_response(response)
            .context("parse project planner response as AIR project manifest")?;
        normalize_project_manifest(&mut manifest, &options.goal, defaults);
        validate_manifest(&manifest)?;
        manifest
    };
    write_project_manifest(options.output.as_deref(), &manifest)
}

fn project_explorer_handoff(goal: &str, repo_root: &Path, model_config: &Path) -> Result<Value> {
    let trace_path = repo_root
        .join(DEFAULT_ARTIFACT_DIR)
        .join("planning")
        .join("project-scout.trace.jsonl");
    if let Some(parent) = trace_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut input = serde_json::Map::new();
    input.insert(
        "task".to_string(),
        Value::String(project_scout_prompt(goal)),
    );
    let profile = repo_root.join(DEFAULT_PROJECT_SCOUT_PROFILE);
    let output = with_current_dir(repo_root, || {
        run_plan_capture(RunPlanOptions {
            plan: None,
            profile: Some(profile),
            store: None,
            input: None,
            input_values: Some(input),
            model_config: Some(model_config.to_path_buf()),
            trace_out: Some(trace_path.clone()),
            trace_redact: false,
            trace_raw: true,
            state_out: None,
            checkpoint_out: None,
            jit_cache: None,
            parallel: false,
            log: false,
            example_tools: false,
            tool_config: None,
            model_replay: None,
        })
    })
    .context("run project scout")?;
    Ok(project_explorer_handoff_value(&trace_path, &output))
}

fn project_scout_prompt(goal: &str) -> String {
    format!(
        "You are the non-mutating project scout for AIR project planning.\n\nProject goal: {goal}\n\nYour job is project-level scouting, not implementation. Explore freely enough to split this project into real code-agent tasks, but stop before inspecting implementation details that the later task agents can inspect themselves.\n\nLarge projects can contain hundreds of thousands of files, so do not scan the whole repository. Use targeted glob, grep, LSP, and bounded span inspection to find evidence for this specific goal. Prefer path and symbol search before inspecting file content. Inspect bounded spans only to identify architectural boundaries, public API surfaces, existing module layout, test/verification entry points, or dependency order.\n\nGood scout evidence is enough to answer:\n- what package/root is relevant\n- what files or symbol groups define natural task boundaries\n- what task should run before another task\n- what focused command can verify each task\n- what files each task should be allowed to touch\n- what risks or unknowns the task agent must handle\n\nDo not try to understand every function body. Do not collect a complete function list. Do not inspect tests or helper internals unless they change task boundaries or verification. Once you can propose credible task boundaries, finish with a concise handoff for the project planner.\n\nFinal handoff format:\n- relevant files or directories and why\n- proposed task boundaries small enough for one code edit loop each\n- dependency order\n- focused verification commands\n- allowed file scopes for each task\n- risks or unknowns\n\nThe next model will turn your handoff into an AIR project DAG. The later code agents will inspect exact file spans during each task."
    )
}

fn project_explorer_handoff_value(trace_path: &Path, output: &Value) -> Value {
    if let Some(answer) = output
        .pointer("/result/answer")
        .and_then(Value::as_str)
        .or_else(|| output.pointer("/answer").and_then(Value::as_str))
    {
        json!({
            "kind": "project_scout_handoff",
            "trace_path": trace_path.display().to_string(),
            "answer": compact_text(answer, 16_000),
        })
    } else {
        json!({
            "kind": "project_scout_handoff",
            "trace_path": trace_path.display().to_string(),
            "raw_output": compact_text(&output.to_string(), 16_000),
        })
    }
}

fn call_project_planner_model(
    model_config: PathBuf,
    planner_model: &str,
    request: &Value,
) -> Result<Value> {
    let mut provider = ModelProviderChoice::from_config_file_with_provider_io(model_config, false)?;
    Ok(provider.call_model(planner_model, request)?)
}

fn project_template_manifest(goal: String, defaults: ProjectDefaults) -> ProjectManifest {
    ProjectManifest {
        schema: PROJECT_SCHEMA.to_string(),
        project: ProjectMetadata {
            name: "air-project".to_string(),
            goal: goal.clone(),
        },
        defaults,
        tasks: vec![ProjectTask {
            id: "task_001".to_string(),
            goal,
            depends_on: Vec::new(),
            skills: Vec::new(),
            allowed_files: Vec::new(),
            forbidden_files: Vec::new(),
            verification: vec![ProjectVerificationCommand {
                command: "cargo test -q".to_string(),
                description: Some("Run project tests".to_string()),
            }],
            success_conditions: ProjectSuccessConditions::default(),
            max_changed_files: None,
            max_diff_lines: None,
        }],
    }
}

fn write_project_manifest(output: Option<&Path>, manifest: &ProjectManifest) -> Result<()> {
    let yaml = serde_yaml::to_string(&manifest).context("serialize AIR project manifest")?;
    if let Some(output) = output {
        if let Some(parent) = output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(output, yaml).with_context(|| format!("write {}", output.display()))?;
        println!("project_file: {}", output.display());
    } else {
        print!("{yaml}");
    }
    Ok(())
}

pub(crate) fn project_run(options: ProjectRunOptions) -> Result<()> {
    let context = ProjectContext::load(&options.file)?;
    let selected = selected_task_ids(&context.manifest, options.task.as_deref())?;
    let explicit_task = options.task.is_some();
    let mut state = context.read_state()?;
    let mut outputs = Vec::new();

    for task_id in selected {
        if !explicit_task
            && state
                .tasks
                .get(&task_id)
                .is_some_and(|state| matches!(state.status, ProjectTaskStatus::Passed))
        {
            continue;
        }
        let task = context.task(&task_id)?;
        ensure_dependencies_passed(&state, task)?;
        eprintln!("[air project] running {}", task.id);
        let task_state = run_project_task(&context, task, options.log)?;
        let output = ProjectTaskRunOutput::from_state(&task.id, &task_state);
        let failed = matches!(task_state.status, ProjectTaskStatus::Failed);
        state.tasks.insert(task.id.clone(), task_state);
        context.write_state(&state)?;
        outputs.push(output);
        if failed {
            break;
        }
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&ProjectRunOutput {
            project: context.manifest.project.name,
            state_path: context.state_path.display().to_string(),
            tasks: outputs,
        })?
    );
    Ok(())
}

pub(crate) fn project_status(options: ProjectStatusOptions) -> Result<()> {
    let context = ProjectContext::load(&options.file)?;
    let state = context.read_state()?;
    let tasks = context
        .manifest
        .tasks
        .iter()
        .map(|task| {
            let task_state = state.tasks.get(&task.id);
            let status = task_state
                .map(|state| match state.status {
                    ProjectTaskStatus::Passed => "passed",
                    ProjectTaskStatus::Failed => "failed",
                })
                .unwrap_or("pending");
            json!({
                "id": task.id,
                "status": status,
                "depends_on": task.depends_on,
                "skills": task.skills,
                "artifact_path": task_state.map(|state| state.artifact_path.clone()),
                "artifact_snapshot_matches_current": task_state.map(|state| state.artifact_snapshot_matches_current),
                "worktree_path": task_state.and_then(|state| state.worktree_path.clone()),
                "patch_path": task_state.and_then(|state| state.patch_path.clone()),
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "project": context.manifest.project.name,
            "goal": context.manifest.project.goal,
            "state_path": context.state_path,
            "tasks": tasks,
        }))?
    );
    Ok(())
}

pub(crate) fn project_verify(options: ProjectVerifyOptions) -> Result<()> {
    let context = ProjectContext::load(&options.file)?;
    let selected = selected_task_ids(&context.manifest, options.task.as_deref())?;
    let mut state = context.read_state()?;
    let mut outputs = Vec::new();

    for task_id in selected {
        let task = context.task(&task_id)?;
        let artifact_dir = context.task_artifact_dir(&task.id);
        let task_state = evaluate_project_task(&context.manifest_dir, &artifact_dir, task)?;
        let output = ProjectTaskRunOutput::from_state(&task.id, &task_state);
        state.tasks.insert(task.id.clone(), task_state);
        outputs.push(output);
    }
    context.write_state(&state)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&ProjectRunOutput {
            project: context.manifest.project.name,
            state_path: context.state_path.display().to_string(),
            tasks: outputs,
        })?
    );
    Ok(())
}

pub(crate) fn bench_project(options: BenchProjectOptions) -> Result<()> {
    let repo_root = std::env::current_dir().context("resolve current directory")?;
    let suite_path = options
        .suite
        .unwrap_or_else(|| repo_root.join(DEFAULT_PROJECT_BENCH_SUITE));
    let suite_path = absolutize(&repo_root, suite_path);
    let suite_dir = suite_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("project bench suite has no parent"))?
        .to_path_buf();
    let suite: ProjectBenchSuite = serde_json::from_slice(
        &fs::read(&suite_path)
            .with_context(|| format!("read project bench suite {}", suite_path.display()))?,
    )
    .with_context(|| format!("parse project bench suite {}", suite_path.display()))?;
    if suite.cases.is_empty() {
        bail!("project bench suite {} has no cases", suite_path.display());
    }
    let run_dir = options.out_dir.unwrap_or_else(|| {
        repo_root
            .join("target/generated/project-bench")
            .join(timestamp_id())
    });
    let run_dir = absolutize(&repo_root, run_dir);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;

    let cases = suite
        .cases
        .iter()
        .map(|case| run_project_bench_case(&suite_dir, &run_dir, case))
        .collect::<Vec<_>>();
    let mut case_runs = Vec::new();
    for case in cases {
        case_runs.push(case?);
    }
    let passed = case_runs.iter().filter(|case| case.pass).count();
    let run = ProjectBenchRun {
        suite: suite.name,
        description: suite.description,
        run_dir: run_dir.display().to_string(),
        summary: ProjectBenchSummary {
            total: case_runs.len(),
            passed,
            failed: case_runs.len().saturating_sub(passed),
        },
        cases: case_runs,
    };
    let run_json = run_dir.join("run.json");
    fs::write(&run_json, serde_json::to_vec_pretty(&run)?)
        .with_context(|| format!("write {}", run_json.display()))?;
    println!("{}", serde_json::to_string_pretty(&run.summary)?);
    println!("run_json: {}", run_json.display());
    Ok(())
}

fn run_project_bench_case(
    _suite_dir: &Path,
    run_dir: &Path,
    case: &ProjectBenchCase,
) -> Result<ProjectBenchCaseRun> {
    match case {
        ProjectBenchCase::DagOrder {
            id,
            manifest,
            expected_order,
        } => {
            let actual = topological_task_ids(manifest)?;
            Ok(ProjectBenchCaseRun {
                id: id.clone(),
                kind: "dag_order".to_string(),
                pass: &actual == expected_order,
                details: json!({
                    "expected_order": expected_order,
                    "actual_order": actual,
                }),
                error: None,
            })
        }
        ProjectBenchCase::DependencyFailure {
            id,
            manifest,
            failed_task,
            target_task,
        } => {
            let task = manifest
                .tasks
                .iter()
                .find(|task| task.id == *target_task)
                .ok_or_else(|| anyhow::anyhow!("unknown target_task {target_task}"))?;
            let state = ProjectState {
                schema: PROJECT_STATE_SCHEMA.to_string(),
                project: manifest.project.name.clone(),
                tasks: BTreeMap::from([(
                    failed_task.clone(),
                    failed_project_bench_task_state(failed_task),
                )]),
            };
            let error = ensure_dependencies_passed(&state, task)
                .expect_err("dependency failure bench case should fail")
                .to_string();
            Ok(ProjectBenchCaseRun {
                id: id.clone(),
                kind: "dependency_failure".to_string(),
                pass: error.contains(failed_task),
                details: json!({ "error": error }),
                error: None,
            })
        }
        ProjectBenchCase::ConstraintCheck {
            id,
            task,
            changed_files,
            diff,
            expected_pass,
            expected_violation_contains,
        } => {
            let result = check_task_constraints(task, changed_files, diff);
            let violation_matches = expected_violation_contains.as_ref().is_none_or(|needle| {
                result
                    .violations
                    .iter()
                    .any(|violation| violation.contains(needle))
            });
            Ok(ProjectBenchCaseRun {
                id: id.clone(),
                kind: "constraint_check".to_string(),
                pass: result.passed == *expected_pass && violation_matches,
                details: json!({
                    "expected_pass": expected_pass,
                    "actual_pass": result.passed,
                    "violations": result.violations,
                }),
                error: None,
            })
        }
        ProjectBenchCase::MissingArtifact { id, task } => {
            let case_dir = run_dir.join(id);
            let workspace = case_dir.join("workspace");
            let artifact = case_dir.join("artifact");
            fs::create_dir_all(&workspace)
                .with_context(|| format!("create {}", workspace.display()))?;
            let state = evaluate_project_task(&workspace, &artifact, task)?;
            let replay_mismatch = state
                .failure_reason
                .as_ref()
                .is_some_and(|reason| reason.category == FailureCategory::ReplayMismatch);
            Ok(ProjectBenchCaseRun {
                id: id.clone(),
                kind: "missing_artifact".to_string(),
                pass: matches!(state.status, ProjectTaskStatus::Failed) && replay_mismatch,
                details: json!({
                    "status": state.status,
                    "failure_reason": state.failure_reason,
                }),
                error: None,
            })
        }
        ProjectBenchCase::StaleArtifact { id, task } => {
            let case_dir = run_dir.join(id);
            let workspace = case_dir.join("workspace");
            let artifact_dir = case_dir.join("artifact");
            fs::create_dir_all(workspace.join("src"))
                .with_context(|| format!("create {}", workspace.display()))?;
            fs::write(
                workspace.join("src/lib.rs"),
                "pub fn value() -> i32 { 1 }\n",
            )?;
            let before = WorkspaceSnapshot::capture(&workspace)?;
            fs::write(
                workspace.join("src/lib.rs"),
                "pub fn value() -> i32 { 2 }\n",
            )?;
            let after = WorkspaceSnapshot::capture(&workspace)?;
            let delta = before.delta(&after)?;
            let artifact = build_code_run_artifact(
                CodeRunDescriptor {
                    task: task.goal.clone(),
                    skill: None,
                    profile: "project-bench".to_string(),
                    model_config: None,
                    tool_config: None,
                    extra: BTreeMap::new(),
                },
                &before,
                &after,
                delta,
                Vec::new(),
                None,
                None,
            )?;
            write_code_run_artifact(&artifact_dir, &artifact, &json!({}), None)?;
            fs::write(
                workspace.join("src/lib.rs"),
                "pub fn value() -> i32 { 3 }\n",
            )?;
            let state = evaluate_project_task(&workspace, &artifact_dir, task)?;
            Ok(ProjectBenchCaseRun {
                id: id.clone(),
                kind: "stale_artifact".to_string(),
                pass: matches!(state.status, ProjectTaskStatus::Failed)
                    && state
                        .failure_reason
                        .as_ref()
                        .is_some_and(|reason| reason.category == FailureCategory::ReplayMismatch),
                details: json!({
                    "artifact_snapshot_matches_current": state.artifact_snapshot_matches_current,
                    "status": state.status,
                    "failure_reason": state.failure_reason,
                }),
                error: None,
            })
        }
    }
}

fn failed_project_bench_task_state(id: &str) -> ProjectTaskState {
    ProjectTaskState {
        status: ProjectTaskStatus::Failed,
        artifact_path: format!("bench/{id}/artifact"),
        changed_files: Vec::new(),
        artifact_snapshot_matches_current: false,
        worktree_path: None,
        patch_path: None,
        verification: Vec::new(),
        constraints: ProjectConstraintResult {
            passed: false,
            violations: vec!["bench failure".to_string()],
        },
        failure_reason: Some(FailureReason {
            category: FailureCategory::AgentError,
            message: "bench dependency failed".to_string(),
            details: BTreeMap::new(),
        }),
    }
}

struct ProjectContext {
    manifest_path: PathBuf,
    manifest_dir: PathBuf,
    manifest: ProjectManifest,
    artifact_dir: PathBuf,
    state_path: PathBuf,
}

impl ProjectContext {
    fn load(path: &Path) -> Result<Self> {
        let cwd = std::env::current_dir().context("resolve current directory")?;
        let manifest_path = absolutize(&cwd, path.to_path_buf());
        let manifest_dir = manifest_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("project file has no parent: {}", path.display()))?
            .to_path_buf();
        let manifest: ProjectManifest = serde_yaml::from_slice(
            &fs::read(&manifest_path)
                .with_context(|| format!("read {}", manifest_path.display()))?,
        )
        .with_context(|| format!("parse {}", manifest_path.display()))?;
        validate_manifest(&manifest)?;
        let artifact_dir = resolve_manifest_path(&manifest_dir, &manifest.defaults.artifact_dir);
        let state_path = artifact_dir.join("state.json");
        Ok(Self {
            manifest_path,
            manifest_dir,
            manifest,
            artifact_dir,
            state_path,
        })
    }

    fn task(&self, id: &str) -> Result<&ProjectTask> {
        self.manifest
            .tasks
            .iter()
            .find(|task| task.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown project task {id}"))
    }

    fn task_artifact_dir(&self, id: &str) -> PathBuf {
        self.artifact_dir.join("tasks").join(id).join("artifact")
    }

    fn task_worktree_dir(&self, id: &str) -> PathBuf {
        self.artifact_dir.join("tasks").join(id).join("worktree")
    }

    fn read_state(&self) -> Result<ProjectState> {
        if !self.state_path.exists() {
            return Ok(ProjectState {
                schema: PROJECT_STATE_SCHEMA.to_string(),
                project: self.manifest.project.name.clone(),
                tasks: BTreeMap::new(),
            });
        }
        let state: ProjectState = serde_json::from_slice(
            &fs::read(&self.state_path)
                .with_context(|| format!("read {}", self.state_path.display()))?,
        )
        .with_context(|| format!("parse {}", self.state_path.display()))?;
        if state.schema != PROJECT_STATE_SCHEMA {
            bail!(
                "project state schema mismatch: expected {}, got {}",
                PROJECT_STATE_SCHEMA,
                state.schema
            );
        }
        if state.project != self.manifest.project.name {
            bail!(
                "project state belongs to {}, but manifest project is {}",
                state.project,
                self.manifest.project.name
            );
        }
        Ok(state)
    }

    fn write_state(&self, state: &ProjectState) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&self.state_path, serde_json::to_vec_pretty(state)?)
            .with_context(|| format!("write {}", self.state_path.display()))
    }
}

impl ProjectTaskRunOutput {
    fn from_state(id: &str, state: &ProjectTaskState) -> Self {
        let missing_artifact = state
            .failure_reason
            .as_ref()
            .is_some_and(|reason| reason.category == FailureCategory::ReplayMismatch);
        Self {
            id: id.to_string(),
            status: state.status.clone(),
            artifact_path: state.artifact_path.clone(),
            changed_files: state.changed_files.clone(),
            artifact_snapshot_matches_current: state.artifact_snapshot_matches_current,
            worktree_path: state.worktree_path.clone(),
            patch_path: state.patch_path.clone(),
            verification_passed: !missing_artifact
                && state.verification.iter().all(|result| result.success),
            constraints_passed: state.constraints.passed,
            failure_reason: state.failure_reason.clone(),
        }
    }
}

fn project_planner_request(goal: &str, repo_root: &Path, exploration: &Value) -> Result<Value> {
    let available_skills =
        route_skills_value_for_task(goal, 5).unwrap_or_else(|_| json!({ "instructions": [] }));
    Ok(json!({
        "goal": goal.trim(),
        "explorer_handoff": exploration,
        "available_instruction_skills": available_skills,
        "repo": {
            "root": repo_root.display().to_string()
        },
        "output_contract": {
            "schema": PROJECT_SCHEMA,
            "project": {
                "name": "short-kebab-case-name",
                "goal": "original goal"
            },
            "defaults": {
                "profile": DEFAULT_CODE_PROFILE,
                "model_config": DEFAULT_MODEL_CONFIG,
                "tool_config": DEFAULT_TOOL_CONFIG,
                "artifact_dir": DEFAULT_ARTIFACT_DIR
            },
            "tasks": [{
                "id": "lower_snake_case_task_id",
                "goal": "one concrete coding task for the code-agent skill, including concrete file/symbol/line-range evidence from explorer_handoff when available",
                "depends_on": [],
                "skills": ["instruction-skill-id"],
                "allowed_files": ["glob-like/path/**"],
                "forbidden_files": ["target/**", ".air/**"],
                "verification": [{"command": "cargo test -p crate-name", "description": "focused verification"}],
                "success_conditions": {
                    "required_changed_files": ["path/to/file.rs"],
                    "required_diff_contains": ["important_symbol_or_module_name"]
                },
                "max_changed_files": 4,
                "max_diff_lines": 350
            }]
        },
        "instructions": [
            "Return exactly one JSON object matching output_contract; no markdown.",
            "Use explorer_handoff as the repository evidence. Do not assume the CLI already scanned the repository.",
            "Preserve useful file paths, symbol names, and line ranges from explorer_handoff directly in each task goal so the code agent can inspect targeted spans instead of rediscovering context.",
            "For skills, recommend only installed instruction skill ids that directly help the task. Leave skills empty when no skill is clearly useful. Do not include the code-agent executor skill in task skills.",
            "Split the project into 2-8 well-scoped coding tasks when the goal is larger than a single edit.",
            "Use depends_on to form a DAG; avoid cycles.",
            "Each task must be executable by the existing code-agent skill edit loop.",
            "Prefer focused allowed_files and verification commands.",
            "Make tasks small: one cohesive move/add/change per task, max_changed_files <= 4, max_diff_lines <= 350 unless the goal is impossible otherwise.",
            "Use forbidden_files to exclude target/**, .git/**, .air/**, and generated outputs.",
            "For create/add goals, tasks may create new files under plausible roots identified by explorer_handoff.",
            "For refactor/split goals, prefer behavior-preserving module extraction tasks guided by explorer_handoff evidence.",
            "Do not invent unrelated files outside package roots and candidate paths from explorer_handoff.",
            "Use success_conditions only for concrete symbols/files the task should create or modify."
        ]
    }))
}

fn parse_project_planner_response(response: Value) -> Result<ProjectManifest> {
    for candidate in project_manifest_candidates(&response) {
        if let Ok(manifest) = serde_json::from_value::<ProjectManifest>(candidate.clone()) {
            return Ok(manifest);
        }
    }
    bail!(
        "project planner response did not contain a valid AIR project manifest: {}",
        compact_response_preview(&response)
    )
}

fn project_manifest_candidates(value: &Value) -> Vec<Value> {
    let mut candidates = Vec::new();
    candidates.push(value.clone());
    match value {
        Value::Object(object) => {
            for key in ["manifest", "project_manifest", "project", "output"] {
                if let Some(value) = object.get(key) {
                    candidates.extend(project_manifest_candidates(value));
                }
            }
            for key in ["content", "answer", "text"] {
                if let Some(text) = object.get(key).and_then(Value::as_str) {
                    candidates.extend(project_manifest_text_candidates(text));
                }
            }
        }
        Value::String(text) => candidates.extend(project_manifest_text_candidates(text)),
        _ => {}
    }
    candidates
}

fn project_manifest_text_candidates(text: &str) -> Vec<Value> {
    let mut candidates = Vec::new();
    for candidate in fenced_or_raw_text_candidates(text) {
        if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
            candidates.push(value);
        } else if let Ok(value) = serde_yaml::from_str::<Value>(&candidate) {
            candidates.push(value);
        }
    }
    candidates
}

fn fenced_or_raw_text_candidates(text: &str) -> Vec<String> {
    let mut candidates = vec![text.trim().to_string()];
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        rest = &rest[start + 3..];
        if let Some(line_end) = rest.find('\n') {
            rest = &rest[line_end + 1..];
        }
        let Some(end) = rest.find("```") else {
            break;
        };
        let body = rest[..end].trim();
        if !body.is_empty() {
            candidates.push(body.to_string());
        }
        rest = &rest[end + 3..];
    }
    candidates
}

fn normalize_project_manifest(
    manifest: &mut ProjectManifest,
    goal: &str,
    defaults: ProjectDefaults,
) {
    manifest.schema = PROJECT_SCHEMA.to_string();
    manifest.defaults = defaults;
    if manifest.project.goal.trim().is_empty() {
        manifest.project.goal = goal.trim().to_string();
    }
    if manifest.project.name.trim().is_empty() {
        manifest.project.name = "air-project".to_string();
    }
    let mut old_to_new_ids = BTreeMap::new();
    for (index, task) in manifest.tasks.iter_mut().enumerate() {
        let old_id = task.id.trim().to_string();
        if task.id.trim().is_empty() {
            task.id = format!("task_{:03}", index + 1);
        }
        task.id = normalize_task_id(&task.id, index + 1);
        if !old_id.is_empty() {
            old_to_new_ids.insert(old_id, task.id.clone());
        }
    }
    let normalized_ids = manifest
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    for task in &mut manifest.tasks {
        task.skills.retain(|skill| !skill.trim().is_empty());
        task.skills.sort();
        task.skills.dedup();
        for dep in &mut task.depends_on {
            if let Some(new_id) = old_to_new_ids.get(dep) {
                *dep = new_id.clone();
                continue;
            }
            let normalized = normalize_task_id(dep, 0);
            if normalized_ids.contains(&normalized) {
                *dep = normalized;
            }
        }
        task.forbidden_files.extend(
            ["target/**", ".git/**", ".air/**"]
                .into_iter()
                .map(str::to_string),
        );
        task.forbidden_files.sort();
        task.forbidden_files.dedup();
        if task.verification.is_empty() {
            task.verification.push(ProjectVerificationCommand {
                command: "cargo test -q".to_string(),
                description: Some("Run project tests".to_string()),
            });
        }
        if task.max_changed_files.is_none_or(|limit| limit > 4) {
            task.max_changed_files = Some(4);
        }
        if task.max_diff_lines.is_none_or(|limit| limit > 500) {
            task.max_diff_lines = Some(500);
        }
    }
}

fn normalize_task_id(id: &str, fallback_index: usize) -> String {
    let normalized = id
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if normalized.is_empty() {
        format!("task_{fallback_index:03}")
    } else {
        normalized
    }
}

fn run_project_task(
    context: &ProjectContext,
    task: &ProjectTask,
    log: bool,
) -> Result<ProjectTaskState> {
    fs::create_dir_all(&context.artifact_dir)
        .with_context(|| format!("create {}", context.artifact_dir.display()))?;
    let artifact_dir = context.task_artifact_dir(&task.id);
    let worktree_dir = prepare_project_task_worktree(context, task)?;
    let profile = project_task_config_path(
        &context.manifest_dir,
        &worktree_dir,
        &context.manifest.defaults.profile,
    );
    let model_config = project_task_config_path(
        &context.manifest_dir,
        &worktree_dir,
        &context.manifest.defaults.model_config,
    );
    let tool_config = project_task_config_path(
        &context.manifest_dir,
        &worktree_dir,
        &context.manifest.defaults.tool_config,
    );
    let trace_path = artifact_dir.join("trace.jsonl");
    let prepared_skills = if task.skills.is_empty() {
        None
    } else {
        Some(prepare_skill_composition(&task.skills)?)
    };
    let mut artifact_extra = BTreeMap::from([
        (
            "project_file".to_string(),
            json!(context.manifest_path.display().to_string()),
        ),
        ("project_task_id".to_string(), json!(task.id)),
        ("project_task_skills".to_string(), json!(task.skills)),
        (
            "project_task_worktree".to_string(),
            json!(worktree_dir.display().to_string()),
        ),
        (
            "project_constraints".to_string(),
            serde_json::to_value(task)?,
        ),
    ]);
    if let Some(prepared) = &prepared_skills {
        artifact_extra.extend(prepared.artifact_extra.clone());
    }
    let task_prompt = if let Some(prepared) = &prepared_skills {
        if let Some(prefix) = &prepared.task_prefix {
            format!(
                "{prefix}\n\n{}",
                project_task_prompt(&context.manifest, task)
            )
        } else {
            project_task_prompt(&context.manifest, task)
        }
    } else {
        project_task_prompt(&context.manifest, task)
    };
    let skill_metadata = prepared_skills
        .as_ref()
        .map(|prepared| prepared.metadata.clone())
        .or_else(|| resolve_skill_run_metadata("code-agent").ok());
    let previous_dir = std::env::current_dir().context("resolve current directory")?;
    std::env::set_current_dir(&worktree_dir)
        .with_context(|| format!("enter project task worktree {}", worktree_dir.display()))?;
    let output = run_code_agent(CodeOptions {
        task: task_prompt,
        verification_command: task
            .verification
            .first()
            .map(|command| command.command.clone()),
        artifact_task: None,
        skill: skill_metadata,
        profile: Some(profile),
        model_config: Some(model_config),
        trace_out: Some(trace_path),
        trace_redact: false,
        trace_raw: true,
        log,
        tool_config: Some(tool_config),
        artifact_out: Some(artifact_dir.clone()),
        artifact_extra,
        verdict_constraints: crate::code_artifact::CodeRunVerdictConstraints::code_edit(),
        replay_artifact: None,
        replay_from: None,
    });
    std::env::set_current_dir(&previous_dir)
        .with_context(|| format!("restore cwd {}", previous_dir.display()))?;
    if let Err(error) = output {
        return Ok(ProjectTaskState {
            status: ProjectTaskStatus::Failed,
            artifact_path: artifact_dir.display().to_string(),
            changed_files: Vec::new(),
            artifact_snapshot_matches_current: false,
            worktree_path: Some(worktree_dir.display().to_string()),
            patch_path: None,
            verification: Vec::new(),
            constraints: ProjectConstraintResult {
                passed: false,
                violations: vec![error.to_string()],
            },
            failure_reason: Some(FailureReason {
                category: FailureCategory::AgentError,
                message: error.to_string(),
                details: BTreeMap::new(),
            }),
        });
    }
    let mut isolated_state = evaluate_project_task(&worktree_dir, &artifact_dir, task)?;
    attach_project_task_paths(&mut isolated_state, &worktree_dir, &artifact_dir);
    if matches!(isolated_state.status, ProjectTaskStatus::Failed) {
        return Ok(isolated_state);
    }
    if let Err(error) =
        apply_project_task_changes(&context.manifest_dir, &worktree_dir, &artifact_dir)
    {
        isolated_state.status = ProjectTaskStatus::Failed;
        isolated_state.artifact_snapshot_matches_current = false;
        isolated_state.constraints.passed = false;
        isolated_state
            .constraints
            .violations
            .push(error.to_string());
        isolated_state.failure_reason = Some(FailureReason {
            category: FailureCategory::ReplayMismatch,
            message: "project task patch could not be applied to main workspace".to_string(),
            details: BTreeMap::from([("error".to_string(), json!(error.to_string()))]),
        });
        return Ok(isolated_state);
    }
    let mut state = evaluate_project_task(&context.manifest_dir, &artifact_dir, task)?;
    attach_project_task_paths(&mut state, &worktree_dir, &artifact_dir);
    Ok(state)
}

fn prepare_project_task_worktree(context: &ProjectContext, task: &ProjectTask) -> Result<PathBuf> {
    let worktree_dir = context.task_worktree_dir(&task.id);
    if worktree_dir.exists() {
        fs::remove_dir_all(&worktree_dir)
            .with_context(|| format!("remove old task worktree {}", worktree_dir.display()))?;
    }
    copy_project_workspace(&context.manifest_dir, &worktree_dir)?;
    init_project_task_git_baseline(&worktree_dir)?;
    Ok(worktree_dir)
}

fn project_task_config_path(manifest_dir: &Path, worktree_dir: &Path, path: &Path) -> PathBuf {
    let resolved = resolve_manifest_path(manifest_dir, path);
    if let Ok(relative) = resolved.strip_prefix(manifest_dir) {
        let candidate = worktree_dir.join(relative);
        if candidate.exists() {
            return candidate;
        }
    }
    resolved
}

fn attach_project_task_paths(
    state: &mut ProjectTaskState,
    worktree_dir: &Path,
    artifact_dir: &Path,
) {
    state.worktree_path = Some(worktree_dir.display().to_string());
    state.patch_path = Some(artifact_dir.join("diff.patch").display().to_string());
}

fn apply_project_task_changes(
    workspace: &Path,
    worktree: &Path,
    artifact_dir: &Path,
) -> Result<()> {
    let artifact = read_code_run_artifact(artifact_dir)
        .with_context(|| format!("read task artifact {}", artifact_dir.display()))?;
    let current = WorkspaceSnapshot::capture(workspace)
        .with_context(|| format!("snapshot workspace {}", workspace.display()))?;
    if !artifact.snapshot.before.matches_snapshot(&current) {
        bail!("main workspace no longer matches task base snapshot");
    }
    for path in &artifact.delta.changed_files {
        let source = worktree.join(path);
        let destination = workspace.join(path);
        if source.exists() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            fs::copy(&source, &destination).with_context(|| {
                format!("copy {} to {}", source.display(), destination.display())
            })?;
        } else if destination.exists() {
            fs::remove_file(&destination)
                .with_context(|| format!("remove {}", destination.display()))?;
        }
    }
    Ok(())
}

fn copy_project_workspace(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    copy_project_workspace_dir(source, source, destination)
}

fn copy_project_workspace_dir(root: &Path, source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let source_path = entry.path();
        let relative = source_path
            .strip_prefix(root)
            .unwrap_or(&source_path)
            .to_string_lossy()
            .replace('\\', "/");
        if project_workspace_copy_ignored(&relative) {
            continue;
        }
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir_all(&destination_path)
                .with_context(|| format!("create {}", destination_path.display()))?;
            copy_project_workspace_dir(root, &source_path, &destination_path)?;
        } else if file_type.is_file() {
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

fn project_workspace_copy_ignored(path: &str) -> bool {
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

fn init_project_task_git_baseline(worktree: &Path) -> Result<()> {
    run_project_task_command(worktree, "git init -q")?;
    run_project_task_command(
        worktree,
        "git config user.email air-project@example.invalid",
    )?;
    run_project_task_command(worktree, "git config user.name 'AIR Project'")?;
    run_project_task_command(worktree, "git add -A")?;
    run_project_task_command(
        worktree,
        "git -c commit.gpgsign=false commit --allow-empty --no-verify -m baseline >/dev/null",
    )
}

fn run_project_task_command(workdir: &Path, command: &str) -> Result<()> {
    let output = Command::new("sh")
        .args(["-lc", command])
        .current_dir(workdir)
        .output()
        .with_context(|| format!("run `{command}` in {}", workdir.display()))?;
    if !output.status.success() {
        bail!(
            "command `{}` failed: {}{}",
            command,
            preview_bytes(&output.stdout, 2000),
            preview_bytes(&output.stderr, 2000)
        );
    }
    Ok(())
}

fn evaluate_project_task(
    workspace: &Path,
    artifact_dir: &Path,
    task: &ProjectTask,
) -> Result<ProjectTaskState> {
    let artifact_path = artifact_dir.join("artifact.json");
    if !artifact_path.exists() {
        return Ok(ProjectTaskState {
            status: ProjectTaskStatus::Failed,
            artifact_path: artifact_dir.display().to_string(),
            changed_files: Vec::new(),
            artifact_snapshot_matches_current: false,
            worktree_path: None,
            patch_path: None,
            verification: Vec::new(),
            constraints: ProjectConstraintResult {
                passed: false,
                violations: vec![format!(
                    "missing task artifact: {}",
                    artifact_path.display()
                )],
            },
            failure_reason: Some(FailureReason {
                category: FailureCategory::ReplayMismatch,
                message: "project task has no code-run artifact; run it before verify".to_string(),
                details: BTreeMap::from([(
                    "artifact_path".to_string(),
                    json!(artifact_path.display().to_string()),
                )]),
            }),
        });
    }
    let artifact = read_code_run_artifact(artifact_dir)
        .with_context(|| format!("read task artifact {}", artifact_dir.display()))?;
    let current_workspace = WorkspaceSnapshot::capture(workspace)
        .with_context(|| format!("snapshot workspace {}", workspace.display()))?;
    let artifact_snapshot_matches_current =
        artifact.snapshot.after.matches_snapshot(&current_workspace);
    let verification = task
        .verification
        .iter()
        .map(|command| run_verification_command(workspace, command))
        .collect::<Result<Vec<_>>>()?;
    let constraints =
        check_task_constraints(task, &artifact.delta.changed_files, &artifact.delta.diff);
    let verification_passed = verification.iter().all(|result| result.success);
    let failure_reason = project_failure_reason(
        &verification,
        &constraints,
        artifact_snapshot_matches_current,
    );
    Ok(ProjectTaskState {
        status: if verification_passed && constraints.passed && artifact_snapshot_matches_current {
            ProjectTaskStatus::Passed
        } else {
            ProjectTaskStatus::Failed
        },
        artifact_path: artifact_dir.display().to_string(),
        changed_files: artifact.delta.changed_files,
        artifact_snapshot_matches_current,
        worktree_path: None,
        patch_path: Some(artifact_dir.join("diff.patch").display().to_string()),
        verification,
        constraints,
        failure_reason,
    })
}

fn run_verification_command(
    workspace: &Path,
    command: &ProjectVerificationCommand,
) -> Result<ProjectVerificationResult> {
    let output = Command::new("sh")
        .args(["-lc", &command.command])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("run verification `{}`", command.command))?;
    Ok(ProjectVerificationResult {
        command: command.command.clone(),
        description: command.description.clone(),
        success: output.status.success(),
        status: output.status.code(),
        stdout_preview: preview_bytes(&output.stdout, 4000),
        stderr_preview: preview_bytes(&output.stderr, 4000),
    })
}

fn check_task_constraints(
    task: &ProjectTask,
    changed_files: &[String],
    diff: &str,
) -> ProjectConstraintResult {
    let mut result = ProjectConstraintResult {
        passed: true,
        violations: Vec::new(),
    };
    if let Some(max_changed_files) = task.max_changed_files {
        if changed_files.len() > max_changed_files {
            result.violations.push(format!(
                "changed {} files, exceeding max_changed_files={max_changed_files}",
                changed_files.len()
            ));
        }
    }
    if let Some(max_diff_lines) = task.max_diff_lines {
        let diff_lines = diff.lines().count();
        if diff_lines > max_diff_lines {
            result.violations.push(format!(
                "diff has {diff_lines} lines, exceeding max_diff_lines={max_diff_lines}"
            ));
        }
    }
    if !task.allowed_files.is_empty() {
        let allowed = match project_glob_set(&task.allowed_files) {
            Ok(allowed) => allowed,
            Err(error) => {
                result.violations.push(error);
                result.passed = false;
                return result;
            }
        };
        for path in changed_files {
            if !allowed.is_match(normalized_project_path(path)) {
                result
                    .violations
                    .push(format!("changed file outside allowed_files: {path}"));
            }
        }
    }
    let forbidden = match project_glob_set(&task.forbidden_files) {
        Ok(forbidden) => forbidden,
        Err(error) => {
            result.violations.push(error);
            result.passed = false;
            return result;
        }
    };
    for path in changed_files {
        if forbidden.is_match(normalized_project_path(path)) {
            result
                .violations
                .push(format!("changed forbidden file: {path}"));
        }
    }
    let changed = changed_files.iter().cloned().collect::<BTreeSet<_>>();
    for path in &task.success_conditions.required_changed_files {
        if !changed.contains(path) {
            result
                .violations
                .push(format!("required changed file missing: {path}"));
        }
    }
    for needle in &task.success_conditions.required_diff_contains {
        if !diff.contains(needle) {
            result
                .violations
                .push(format!("diff does not contain required text: {needle}"));
        }
    }
    result.passed = result.violations.is_empty();
    result
}

fn project_glob_set(patterns: &[String]) -> std::result::Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = normalized_project_path(pattern);
        let glob = Glob::new(&pattern)
            .map_err(|error| format!("invalid project file glob `{}`: {error}", pattern))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| format!("invalid project file glob set: {error}"))
}

fn normalized_project_path(path: &str) -> String {
    path.trim().replace('\\', "/")
}

fn project_failure_reason(
    verification: &[ProjectVerificationResult],
    constraints: &ProjectConstraintResult,
    artifact_snapshot_matches_current: bool,
) -> Option<FailureReason> {
    if let Some(failed) = verification.iter().find(|result| !result.success) {
        return Some(FailureReason {
            category: FailureCategory::VerificationFailed,
            message: format!("verification command failed: {}", failed.command),
            details: BTreeMap::from([
                ("status".to_string(), json!(failed.status)),
                ("stderr_preview".to_string(), json!(failed.stderr_preview)),
            ]),
        });
    }
    if !constraints.passed {
        return Some(FailureReason {
            category: FailureCategory::DiffConstraintFailed,
            message: "project task diff constraints failed".to_string(),
            details: BTreeMap::from([("violations".to_string(), json!(constraints.violations))]),
        });
    }
    if !artifact_snapshot_matches_current {
        return Some(FailureReason {
            category: FailureCategory::ReplayMismatch,
            message: "artifact after snapshot does not match current workspace".to_string(),
            details: BTreeMap::from([(
                "artifact_snapshot_matches_current".to_string(),
                json!(false),
            )]),
        });
    }
    None
}

fn project_task_prompt(manifest: &ProjectManifest, task: &ProjectTask) -> String {
    let mut prompt = format!(
        "Project goal: {}\n\nTask {}: {}\n",
        manifest.project.goal, task.id, task.goal
    );
    if !task.allowed_files.is_empty() || !task.forbidden_files.is_empty() {
        prompt.push_str("\nFile contract:\n");
        prompt.push_str("- Only modify files allowed by this task.\n");
        prompt.push_str(
            "- If the task requires a file outside allowed_files, stop and report why.\n",
        );
        prompt.push_str("- Do not edit forbidden files.\n");
    }
    if !task.allowed_files.is_empty() {
        prompt.push_str("\nAllowed files:\n");
        for path in &task.allowed_files {
            prompt.push_str(&format!("- {path}\n"));
        }
    }
    if !task.forbidden_files.is_empty() {
        prompt.push_str("\nForbidden files:\n");
        for path in &task.forbidden_files {
            prompt.push_str(&format!("- {path}\n"));
        }
    }
    if !task.skills.is_empty() {
        prompt.push_str("\nPreloaded instruction skills:\n");
        for skill in &task.skills {
            prompt.push_str(&format!("- {skill}\n"));
        }
    }
    if !task.verification.is_empty() {
        prompt.push_str("\nRun verification before finishing:\n");
        for command in &task.verification {
            prompt.push_str(&format!("- {}\n", command.command));
        }
    }
    if !task.success_conditions.required_changed_files.is_empty()
        || !task.success_conditions.required_diff_contains.is_empty()
    {
        prompt.push_str("\nSuccess conditions:\n");
        for path in &task.success_conditions.required_changed_files {
            prompt.push_str(&format!("- changed file required: {path}\n"));
        }
        for text in &task.success_conditions.required_diff_contains {
            prompt.push_str(&format!("- diff must contain: {text}\n"));
        }
    }
    prompt
}

fn selected_task_ids(manifest: &ProjectManifest, task: Option<&str>) -> Result<Vec<String>> {
    let ordered = topological_task_ids(manifest)?;
    if let Some(id) = task {
        if !ordered.iter().any(|task_id| task_id == id) {
            bail!("unknown project task {id}");
        }
        Ok(vec![id.to_string()])
    } else {
        Ok(ordered)
    }
}

fn topological_task_ids(manifest: &ProjectManifest) -> Result<Vec<String>> {
    let tasks = manifest
        .tasks
        .iter()
        .map(|task| (&task.id, task))
        .collect::<BTreeMap<_, _>>();
    let mut ordered = Vec::new();
    let mut remaining = tasks
        .keys()
        .map(|id| (*id).clone())
        .collect::<BTreeSet<_>>();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .find(|id| {
                tasks
                    .get(*id)
                    .is_some_and(|task| task.depends_on.iter().all(|dep| ordered.contains(dep)))
            })
            .cloned();
        let Some(id) = ready else {
            bail!("project task dependency cycle or missing dependency");
        };
        remaining.remove(&id);
        ordered.push(id);
    }
    Ok(ordered)
}

fn ensure_dependencies_passed(state: &ProjectState, task: &ProjectTask) -> Result<()> {
    for dep in &task.depends_on {
        match state.tasks.get(dep).map(|state| &state.status) {
            Some(ProjectTaskStatus::Passed) => {}
            Some(ProjectTaskStatus::Failed) => {
                bail!("task {} depends on failed task {dep}", task.id);
            }
            None => bail!("task {} depends on pending task {dep}", task.id),
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &ProjectManifest) -> Result<()> {
    if manifest.schema != PROJECT_SCHEMA {
        bail!("unsupported project schema {}", manifest.schema);
    }
    if manifest.tasks.is_empty() {
        bail!("project manifest has no tasks");
    }
    let mut ids = BTreeSet::new();
    for task in &manifest.tasks {
        if task.id.trim().is_empty() {
            bail!("project task id must not be empty");
        }
        if !ids.insert(task.id.clone()) {
            bail!("duplicate project task id {}", task.id);
        }
    }
    for task in &manifest.tasks {
        if task.skills.iter().any(|skill| skill == "code-agent") {
            bail!(
                "task {} skills must list instruction skills, not the code-agent executor",
                task.id
            );
        }
        for dep in &task.depends_on {
            if !ids.contains(dep) {
                bail!("task {} depends on unknown task {dep}", task.id);
            }
        }
    }
    topological_task_ids(manifest)?;
    Ok(())
}

fn compact_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect::<String>()
}

fn compact_response_preview(value: &Value) -> String {
    compact_text(&value.to_string(), 2000)
}

fn resolve_manifest_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn path_for_manifest(manifest_dir: &Path, target: &Path) -> Result<PathBuf> {
    fs::create_dir_all(manifest_dir)
        .with_context(|| format!("create {}", manifest_dir.display()))?;
    let manifest_dir = manifest_dir
        .canonicalize()
        .with_context(|| format!("canonicalize {}", manifest_dir.display()))?;
    let target = target
        .canonicalize()
        .with_context(|| format!("canonicalize {}", target.display()))?;
    Ok(relative_path(&manifest_dir, &target).unwrap_or(target))
}

fn relative_path(from_dir: &Path, target: &Path) -> Option<PathBuf> {
    let from_components = from_dir.components().collect::<Vec<_>>();
    let target_components = target.components().collect::<Vec<_>>();
    let common = from_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return None;
    }
    let mut relative = PathBuf::new();
    for component in from_components.iter().skip(common) {
        if matches!(component, std::path::Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in target_components.iter().skip(common) {
        relative.push(component.as_os_str());
    }
    if relative.as_os_str().is_empty() {
        Some(PathBuf::from("."))
    } else {
        Some(relative)
    }
}

fn absolutize(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn timestamp_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("{millis}")
}

fn with_current_dir<T>(dir: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let previous_dir = std::env::current_dir().context("resolve current directory")?;
    std::env::set_current_dir(dir).with_context(|| format!("enter {}", dir.display()))?;
    let result = operation();
    let restore_result = std::env::set_current_dir(&previous_dir)
        .with_context(|| format!("restore cwd {}", previous_dir.display()));
    match (result, restore_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(restore_error)) => {
            Err(error).with_context(|| format!("also failed to restore cwd: {restore_error}"))
        }
    }
}

fn preview_bytes(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.chars().count() <= max_chars {
        text.into_owned()
    } else {
        let head = text.chars().take(max_chars / 2).collect::<String>();
        let tail = text
            .chars()
            .rev()
            .take(max_chars / 2)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        format!("{head}\n[AIR_TRUNCATED]\n{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_topological_task_ids_orders_dependencies() {
        let manifest = ProjectManifest {
            schema: PROJECT_SCHEMA.to_string(),
            project: ProjectMetadata {
                name: "demo".to_string(),
                goal: "demo".to_string(),
            },
            defaults: ProjectDefaults::default(),
            tasks: vec![
                ProjectTask {
                    id: "second".to_string(),
                    goal: "second".to_string(),
                    depends_on: vec!["first".to_string()],
                    skills: Vec::new(),
                    allowed_files: Vec::new(),
                    forbidden_files: Vec::new(),
                    verification: Vec::new(),
                    success_conditions: ProjectSuccessConditions::default(),
                    max_changed_files: None,
                    max_diff_lines: None,
                },
                ProjectTask {
                    id: "first".to_string(),
                    goal: "first".to_string(),
                    depends_on: Vec::new(),
                    skills: Vec::new(),
                    allowed_files: Vec::new(),
                    forbidden_files: Vec::new(),
                    verification: Vec::new(),
                    success_conditions: ProjectSuccessConditions::default(),
                    max_changed_files: None,
                    max_diff_lines: None,
                },
            ],
        };

        assert_eq!(
            topological_task_ids(&manifest).unwrap(),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn project_constraints_enforce_allowed_files_and_diff_text() {
        let task = ProjectTask {
            id: "task".to_string(),
            goal: "goal".to_string(),
            depends_on: Vec::new(),
            skills: Vec::new(),
            allowed_files: vec!["src/**".to_string()],
            forbidden_files: vec!["src/generated/**".to_string()],
            verification: Vec::new(),
            success_conditions: ProjectSuccessConditions {
                required_changed_files: vec!["src/lib.rs".to_string()],
                required_diff_contains: vec!["helper_name".to_string()],
            },
            max_changed_files: Some(1),
            max_diff_lines: Some(20),
        };

        let result =
            check_task_constraints(&task, &["src/lib.rs".to_string()], "+fn helper_name() {}\n");
        assert!(result.passed, "{:?}", result.violations);

        let result = check_task_constraints(
            &task,
            &["tests/lib.rs".to_string()],
            "+fn helper_name() {}\n",
        );
        assert!(!result.passed);
        assert!(result
            .violations
            .iter()
            .any(|violation| violation.contains("outside allowed_files")));
    }

    #[test]
    fn project_constraints_use_standard_glob_patterns() {
        let task = ProjectTask {
            id: "task".to_string(),
            goal: "goal".to_string(),
            depends_on: Vec::new(),
            skills: Vec::new(),
            allowed_files: vec!["crates/**/src/*.rs".to_string()],
            forbidden_files: Vec::new(),
            verification: Vec::new(),
            success_conditions: ProjectSuccessConditions::default(),
            max_changed_files: None,
            max_diff_lines: None,
        };

        let result = check_task_constraints(
            &task,
            &["crates/air-tools/src/lib.rs".to_string()],
            "+pub fn helper() {}\n",
        );
        assert!(result.passed, "{:?}", result.violations);

        let result = check_task_constraints(
            &task,
            &["crates/air-tools/tests/lib.rs".to_string()],
            "+pub fn helper() {}\n",
        );
        assert!(!result.passed);
        assert!(result
            .violations
            .iter()
            .any(|violation| violation.contains("outside allowed_files")));
    }

    #[test]
    fn project_task_config_path_rebases_repo_local_configs_to_worktree() {
        let dir = std::env::temp_dir().join(format!(
            "air-project-config-path-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = dir.join("repo");
        let worktree = dir.join("worktree");
        fs::create_dir_all(worktree.join("skills/code-agent")).unwrap();
        fs::write(worktree.join("skills/code-agent/tools.json"), "{}").unwrap();
        let config = PathBuf::from("skills/code-agent/tools.json");

        assert_eq!(
            project_task_config_path(&root, &worktree, &config),
            worktree.join("skills/code-agent/tools.json")
        );
    }

    #[test]
    fn apply_project_task_changes_syncs_isolated_worktree_delta() {
        let dir = std::env::temp_dir().join(format!(
            "air-project-apply-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let workspace = dir.join("workspace");
        let worktree = dir.join("worktree");
        let artifact_dir = dir.join("artifact");
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(
            workspace.join("src/lib.rs"),
            "pub fn value() -> i32 { 1 }\n",
        )
        .unwrap();
        fs::write(worktree.join("src/lib.rs"), "pub fn value() -> i32 { 1 }\n").unwrap();
        let before = WorkspaceSnapshot::capture(&workspace).unwrap();
        fs::write(worktree.join("src/lib.rs"), "pub fn value() -> i32 { 2 }\n").unwrap();
        let after = WorkspaceSnapshot::capture(&worktree).unwrap();
        let delta = before.delta(&after).unwrap();
        let artifact = build_code_run_artifact(
            CodeRunDescriptor {
                task: "change value".to_string(),
                skill: None,
                profile: "test".to_string(),
                model_config: None,
                tool_config: None,
                extra: BTreeMap::new(),
            },
            &before,
            &after,
            delta,
            Vec::new(),
            None,
            None,
        )
        .unwrap();
        write_code_run_artifact(&artifact_dir, &artifact, &json!({}), None).unwrap();

        apply_project_task_changes(&workspace, &worktree, &artifact_dir).unwrap();

        assert_eq!(
            fs::read_to_string(workspace.join("src/lib.rs")).unwrap(),
            "pub fn value() -> i32 { 2 }\n"
        );
    }

    #[test]
    fn project_task_prompt_includes_verification_and_constraints() {
        let manifest = ProjectManifest {
            schema: PROJECT_SCHEMA.to_string(),
            project: ProjectMetadata {
                name: "demo".to_string(),
                goal: "ship feature".to_string(),
            },
            defaults: ProjectDefaults::default(),
            tasks: vec![],
        };
        let task = ProjectTask {
            id: "task_001".to_string(),
            goal: "change parser".to_string(),
            depends_on: Vec::new(),
            skills: vec!["tdd-workflow".to_string()],
            allowed_files: vec!["crates/air-parser/**".to_string()],
            forbidden_files: vec!["target/**".to_string()],
            verification: vec![ProjectVerificationCommand {
                command: "cargo test -p air-parser".to_string(),
                description: None,
            }],
            success_conditions: ProjectSuccessConditions {
                required_changed_files: vec!["crates/air-parser/src/lib.rs".to_string()],
                required_diff_contains: vec!["parse_helper".to_string()],
            },
            max_changed_files: None,
            max_diff_lines: None,
        };

        let prompt = project_task_prompt(&manifest, &task);
        assert!(prompt.contains("Project goal: ship feature"));
        assert!(prompt.contains("tdd-workflow"));
        assert!(prompt.contains("Only modify files allowed by this task"));
        assert!(prompt.contains("crates/air-parser/**"));
        assert!(prompt.contains("cargo test -p air-parser"));
        assert!(prompt.contains("parse_helper"));
    }

    #[test]
    fn parses_project_planner_response_from_content_wrapper() {
        let response = json!({
            "content": "```json\n{\"schema\":\"air.project.v1\",\"project\":{\"name\":\"tools-refactor\",\"goal\":\"refactor tools\"},\"defaults\":{},\"tasks\":[{\"id\":\"split_file_tools\",\"goal\":\"split file tools\",\"allowed_files\":[\"crates/air-tools/src/**\"],\"verification\":[{\"command\":\"cargo test -p air-tools\"}],\"success_conditions\":{\"required_diff_contains\":[\"mod file\"]}}]}\n```"
        });

        let mut manifest = parse_project_planner_response(response).unwrap();
        normalize_project_manifest(&mut manifest, "refactor tools", ProjectDefaults::default());
        validate_manifest(&manifest).unwrap();

        assert_eq!(manifest.tasks.len(), 1);
        assert_eq!(manifest.tasks[0].id, "split_file_tools");
        assert!(manifest.tasks[0]
            .forbidden_files
            .contains(&"target/**".to_string()));
    }

    #[test]
    fn normalize_project_manifest_rewrites_dependency_ids() {
        let mut manifest = ProjectManifest {
            schema: PROJECT_SCHEMA.to_string(),
            project: ProjectMetadata {
                name: "demo".to_string(),
                goal: "demo".to_string(),
            },
            defaults: ProjectDefaults::default(),
            tasks: vec![
                ProjectTask {
                    id: "split-file-tools".to_string(),
                    goal: "split file tools".to_string(),
                    depends_on: Vec::new(),
                    skills: Vec::new(),
                    allowed_files: Vec::new(),
                    forbidden_files: Vec::new(),
                    verification: Vec::new(),
                    success_conditions: ProjectSuccessConditions::default(),
                    max_changed_files: None,
                    max_diff_lines: None,
                },
                ProjectTask {
                    id: "update-callers".to_string(),
                    goal: "update callers".to_string(),
                    depends_on: vec!["split-file-tools".to_string()],
                    skills: Vec::new(),
                    allowed_files: Vec::new(),
                    forbidden_files: Vec::new(),
                    verification: Vec::new(),
                    success_conditions: ProjectSuccessConditions::default(),
                    max_changed_files: None,
                    max_diff_lines: None,
                },
            ],
        };

        normalize_project_manifest(&mut manifest, "demo", ProjectDefaults::default());
        validate_manifest(&manifest).unwrap();

        assert_eq!(manifest.tasks[0].id, "split_file_tools");
        assert_eq!(manifest.tasks[1].id, "update_callers");
        assert_eq!(manifest.tasks[1].depends_on, vec!["split_file_tools"]);
    }

    #[test]
    fn normalize_project_manifest_preserves_supplied_defaults() {
        let defaults = ProjectDefaults {
            profile: PathBuf::from("profiles/custom.air-profile.yaml"),
            model_config: PathBuf::from("models/custom.json"),
            tool_config: PathBuf::from("tools/custom.json"),
            artifact_dir: PathBuf::from(".air/custom-project"),
        };
        let mut manifest = ProjectManifest {
            schema: PROJECT_SCHEMA.to_string(),
            project: ProjectMetadata {
                name: "demo".to_string(),
                goal: String::new(),
            },
            defaults: ProjectDefaults::default(),
            tasks: vec![ProjectTask {
                id: "task".to_string(),
                goal: "task".to_string(),
                depends_on: Vec::new(),
                skills: Vec::new(),
                allowed_files: Vec::new(),
                forbidden_files: Vec::new(),
                verification: Vec::new(),
                success_conditions: ProjectSuccessConditions::default(),
                max_changed_files: None,
                max_diff_lines: None,
            }],
        };

        normalize_project_manifest(&mut manifest, "demo", defaults.clone());

        assert_eq!(manifest.defaults.profile, defaults.profile);
        assert_eq!(manifest.defaults.model_config, defaults.model_config);
        assert_eq!(manifest.defaults.tool_config, defaults.tool_config);
        assert_eq!(manifest.defaults.artifact_dir, defaults.artifact_dir);
    }

    #[test]
    fn project_state_must_match_manifest_project() {
        let dir = std::env::temp_dir().join(format!(
            "air-project-state-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let artifact_dir = dir.join(".air/project");
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::write(
            artifact_dir.join("state.json"),
            serde_json::to_vec_pretty(&ProjectState {
                schema: PROJECT_STATE_SCHEMA.to_string(),
                project: "other".to_string(),
                tasks: BTreeMap::new(),
            })
            .unwrap(),
        )
        .unwrap();
        let context = ProjectContext {
            manifest_path: dir.join("air-project.yaml"),
            manifest_dir: dir.clone(),
            manifest: ProjectManifest {
                schema: PROJECT_SCHEMA.to_string(),
                project: ProjectMetadata {
                    name: "demo".to_string(),
                    goal: "demo".to_string(),
                },
                defaults: ProjectDefaults::default(),
                tasks: Vec::new(),
            },
            artifact_dir: artifact_dir.clone(),
            state_path: artifact_dir.join("state.json"),
        };

        let error = context.read_state().unwrap_err().to_string();
        assert!(error.contains("project state belongs to other"));
    }

    #[test]
    fn project_planner_request_uses_explorer_handoff_without_scanning_repo() {
        let dir = std::env::temp_dir().join(format!(
            "air-project-request-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();

        let exploration = json!({
            "trace_path": ".air/project/planning/explore.trace.jsonl",
            "answer": "Relevant file: src/lib.rs. Proposed task: split src/lib.rs."
        });
        let request = project_planner_request("split src", &dir, &exploration).unwrap();
        let rendered = serde_json::to_string(&request).unwrap();

        assert!(!rendered.contains("pub fn demo"), "{rendered}");
        assert!(!rendered.contains("line_count"), "{rendered}");
        assert!(rendered.contains("explorer_handoff"), "{rendered}");
        assert!(rendered.contains("Relevant file"), "{rendered}");
        assert!(rendered.contains("output_contract"), "{rendered}");
        assert!(rendered.contains("line-range evidence"), "{rendered}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn project_failure_reason_allows_verify_only_passed_tasks_without_patch() {
        let verification = vec![ProjectVerificationResult {
            command: "cargo test -p air-tools".to_string(),
            description: None,
            success: true,
            status: Some(0),
            stdout_preview: String::new(),
            stderr_preview: String::new(),
        }];
        let constraints = ProjectConstraintResult {
            passed: true,
            violations: Vec::new(),
        };

        assert!(project_failure_reason(&verification, &constraints, true).is_none());
    }

    #[test]
    fn project_failure_reason_marks_stale_artifact_as_replay_mismatch() {
        let verification = vec![ProjectVerificationResult {
            command: "cargo test".to_string(),
            description: None,
            success: true,
            status: Some(0),
            stdout_preview: String::new(),
            stderr_preview: String::new(),
        }];
        let constraints = ProjectConstraintResult {
            passed: true,
            violations: Vec::new(),
        };

        let reason = project_failure_reason(&verification, &constraints, false).unwrap();
        assert_eq!(reason.category, FailureCategory::ReplayMismatch);
        assert!(reason.message.contains("artifact after snapshot"));
    }

    #[test]
    fn project_explorer_handoff_extracts_compact_answer() {
        let output = json!({
            "result": {
                "answer": "Relevant files: crates/air-tools/src/lib.rs. Proposed task: split helpers.",
                "decision": {
                    "complete": true,
                    "tool_calls": []
                }
            }
        });
        let handoff = project_explorer_handoff_value(
            Path::new(".air/project/planning/explore.trace.jsonl"),
            &output,
        );

        assert_eq!(
            handoff["answer"].as_str(),
            Some("Relevant files: crates/air-tools/src/lib.rs. Proposed task: split helpers.")
        );
        assert_eq!(
            handoff["trace_path"].as_str(),
            Some(".air/project/planning/explore.trace.jsonl")
        );
        assert!(handoff.get("decision").is_none());
    }
}
