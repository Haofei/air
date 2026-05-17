use crate::code_agent::{run_code_agent, CodeOptions};
use crate::code_artifact::{read_code_run_artifact, FailureCategory, FailureReason};
use crate::models::ModelProviderChoice;
use crate::run_plan::{run_plan_capture, RunPlanOptions};
use air_runtime::ModelProvider;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PROJECT_SCHEMA: &str = "air.project.v1";
const PROJECT_STATE_SCHEMA: &str = "air.project_state.v1";
const DEFAULT_PROJECT_FILE: &str = "air-project.yaml";
const DEFAULT_CODE_PROFILE: &str = "examples/code-agent/edit.air-profile.yaml";
const DEFAULT_PROJECT_SCOUT_PROFILE: &str = "examples/code-agent/project-scout.air-profile.yaml";
const DEFAULT_MODEL_CONFIG: &str = "examples/bigmodel-openai-compatible.json";
const DEFAULT_TOOL_CONFIG: &str = "examples/code-agent/tools.json";
const DEFAULT_ARTIFACT_DIR: &str = ".air/project";

pub(crate) fn default_project_file() -> PathBuf {
    PathBuf::from(DEFAULT_PROJECT_FILE)
}

#[derive(Debug)]
pub(crate) struct ProjectPlanOptions {
    pub(crate) goal: String,
    pub(crate) output: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) planner_model: String,
    pub(crate) template: bool,
}

#[derive(Debug)]
pub(crate) struct ProjectRunOptions {
    pub(crate) file: PathBuf,
    pub(crate) task: Option<String>,
    pub(crate) log: bool,
}

#[derive(Debug)]
pub(crate) struct ProjectStatusOptions {
    pub(crate) file: PathBuf,
}

#[derive(Debug)]
pub(crate) struct ProjectVerifyOptions {
    pub(crate) file: PathBuf,
    pub(crate) task: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectManifest {
    schema: String,
    project: ProjectMetadata,
    #[serde(default)]
    defaults: ProjectDefaults,
    tasks: Vec<ProjectTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectMetadata {
    name: String,
    goal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectDefaults {
    #[serde(default = "default_profile")]
    profile: PathBuf,
    #[serde(default = "default_model_config")]
    model_config: PathBuf,
    #[serde(default = "default_tool_config")]
    tool_config: PathBuf,
    #[serde(default = "default_artifact_dir")]
    artifact_dir: PathBuf,
}

impl Default for ProjectDefaults {
    fn default() -> Self {
        Self {
            profile: default_profile(),
            model_config: default_model_config(),
            tool_config: default_tool_config(),
            artifact_dir: default_artifact_dir(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectTask {
    id: String,
    goal: String,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    allowed_files: Vec<String>,
    #[serde(default)]
    forbidden_files: Vec<String>,
    #[serde(default)]
    verification: Vec<ProjectVerificationCommand>,
    #[serde(default)]
    success_conditions: ProjectSuccessConditions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_changed_files: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_diff_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectVerificationCommand {
    command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectSuccessConditions {
    #[serde(default)]
    required_changed_files: Vec<String>,
    #[serde(default)]
    required_diff_contains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectState {
    schema: String,
    project: String,
    tasks: BTreeMap<String, ProjectTaskState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProjectTaskStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectTaskState {
    status: ProjectTaskStatus,
    artifact_path: String,
    changed_files: Vec<String>,
    verification: Vec<ProjectVerificationResult>,
    constraints: ProjectConstraintResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_reason: Option<FailureReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectVerificationResult {
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<i32>,
    stdout_preview: String,
    stderr_preview: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectConstraintResult {
    passed: bool,
    violations: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProjectRunOutput {
    project: String,
    state_path: String,
    tasks: Vec<ProjectTaskRunOutput>,
}

#[derive(Debug, Serialize)]
struct ProjectTaskRunOutput {
    id: String,
    status: ProjectTaskStatus,
    artifact_path: String,
    changed_files: Vec<String>,
    verification_passed: bool,
    constraints_passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_reason: Option<FailureReason>,
}

pub(crate) fn project_plan(options: ProjectPlanOptions) -> Result<()> {
    if options.goal.trim().is_empty() {
        bail!("air project plan goal must not be empty");
    }
    let manifest = if options.template {
        project_template_manifest(options.goal)
    } else {
        let cwd = std::env::current_dir().context("resolve current directory")?;
        let model_config = options
            .model_config
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
        normalize_project_manifest(&mut manifest, &options.goal);
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
        "You are the read-only project scout for AIR project planning.\n\nProject goal: {goal}\n\nYour job is project-level scouting, not implementation. Explore freely enough to split this project into real code-agent tasks, but stop before reading implementation details that the later task agents can read themselves.\n\nLarge projects can contain hundreds of thousands of files, so do not scan the whole repository. Use targeted glob, grep, LSP, and narrow reads to find evidence for this specific goal. Prefer path and symbol search before reading file content. Use narrow reads only to identify architectural boundaries, public API surfaces, existing module layout, test/verification entry points, or dependency order.\n\nGood scout evidence is enough to answer:\n- what package/root is relevant\n- what files or symbol groups define natural task boundaries\n- what task should run before another task\n- what focused command can verify each task\n- what files each task should be allowed to touch\n- what risks or unknowns the task agent must handle\n\nDo not try to understand every function body. Do not collect a complete function list. Do not inspect tests or helper internals unless they change task boundaries or verification. Once you can propose credible task boundaries, finish with a concise handoff for the project planner.\n\nFinal handoff format:\n- relevant files or directories and why\n- proposed task boundaries small enough for one code edit loop each\n- dependency order\n- focused verification commands\n- allowed file scopes for each task\n- risks or unknowns\n\nThe next model will turn your handoff into an AIR project DAG. The later code agents will do detailed file reading during each task."
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

fn project_template_manifest(goal: String) -> ProjectManifest {
    ProjectManifest {
        schema: PROJECT_SCHEMA.to_string(),
        project: ProjectMetadata {
            name: "air-project".to_string(),
            goal: goal.clone(),
        },
        defaults: ProjectDefaults::default(),
        tasks: vec![ProjectTask {
            id: "task_001".to_string(),
            goal,
            depends_on: Vec::new(),
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
            let status = state
                .tasks
                .get(&task.id)
                .map(|state| match state.status {
                    ProjectTaskStatus::Passed => "passed",
                    ProjectTaskStatus::Failed => "failed",
                })
                .unwrap_or("pending");
            json!({
                "id": task.id,
                "status": status,
                "depends_on": task.depends_on,
                "artifact_path": state.tasks.get(&task.id).map(|state| state.artifact_path.clone()),
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

    fn read_state(&self) -> Result<ProjectState> {
        if !self.state_path.exists() {
            return Ok(ProjectState {
                schema: PROJECT_STATE_SCHEMA.to_string(),
                project: self.manifest.project.name.clone(),
                tasks: BTreeMap::new(),
            });
        }
        serde_json::from_slice(
            &fs::read(&self.state_path)
                .with_context(|| format!("read {}", self.state_path.display()))?,
        )
        .with_context(|| format!("parse {}", self.state_path.display()))
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
            verification_passed: !missing_artifact
                && state.verification.iter().all(|result| result.success),
            constraints_passed: state.constraints.passed,
            failure_reason: state.failure_reason.clone(),
        }
    }
}

fn project_planner_request(goal: &str, repo_root: &Path, exploration: &Value) -> Result<Value> {
    Ok(json!({
        "goal": goal.trim(),
        "explorer_handoff": exploration,
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
                "goal": "one concrete coding task for air code",
                "depends_on": [],
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
            "Split the project into 2-8 well-scoped coding tasks when the goal is larger than a single edit.",
            "Use depends_on to form a DAG; avoid cycles.",
            "Each task must be executable by the existing air code edit loop.",
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

fn normalize_project_manifest(manifest: &mut ProjectManifest, goal: &str) {
    manifest.schema = PROJECT_SCHEMA.to_string();
    manifest.defaults = ProjectDefaults::default();
    if manifest.project.goal.trim().is_empty() {
        manifest.project.goal = goal.trim().to_string();
    }
    if manifest.project.name.trim().is_empty() {
        manifest.project.name = "air-project".to_string();
    }
    for (index, task) in manifest.tasks.iter_mut().enumerate() {
        if task.id.trim().is_empty() {
            task.id = format!("task_{:03}", index + 1);
        }
        task.id = normalize_task_id(&task.id, index + 1);
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
    let profile = resolve_manifest_path(&context.manifest_dir, &context.manifest.defaults.profile);
    let model_config = resolve_manifest_path(
        &context.manifest_dir,
        &context.manifest.defaults.model_config,
    );
    let tool_config = resolve_manifest_path(
        &context.manifest_dir,
        &context.manifest.defaults.tool_config,
    );
    let artifact_dir = context.task_artifact_dir(&task.id);
    let trace_path = artifact_dir.join("trace.jsonl");
    let artifact_extra = BTreeMap::from([
        (
            "project_file".to_string(),
            json!(context.manifest_path.display().to_string()),
        ),
        ("project_task_id".to_string(), json!(task.id)),
        (
            "project_constraints".to_string(),
            serde_json::to_value(task)?,
        ),
    ]);
    let previous_dir = std::env::current_dir().context("resolve current directory")?;
    std::env::set_current_dir(&context.manifest_dir)
        .with_context(|| format!("enter project workspace {}", context.manifest_dir.display()))?;
    let output = run_code_agent(CodeOptions {
        task: project_task_prompt(&context.manifest, task),
        profile: Some(profile),
        model_config: Some(model_config),
        trace_out: Some(trace_path),
        trace_redact: false,
        trace_raw: true,
        log,
        explain: false,
        tool_config: Some(tool_config),
        artifact_out: Some(artifact_dir.clone()),
        artifact_extra,
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
    evaluate_project_task(&context.manifest_dir, &artifact_dir, task)
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
    let verification = task
        .verification
        .iter()
        .map(|command| run_verification_command(workspace, command))
        .collect::<Result<Vec<_>>>()?;
    let constraints =
        check_task_constraints(task, &artifact.delta.changed_files, &artifact.delta.diff);
    let verification_passed = verification.iter().all(|result| result.success);
    let failure_reason =
        project_failure_reason(&verification, &constraints, &artifact.delta.changed_files);
    Ok(ProjectTaskState {
        status: if verification_passed && constraints.passed {
            ProjectTaskStatus::Passed
        } else {
            ProjectTaskStatus::Failed
        },
        artifact_path: artifact_dir.display().to_string(),
        changed_files: artifact.delta.changed_files,
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
        for path in changed_files {
            if !task
                .allowed_files
                .iter()
                .any(|pattern| globish_match(pattern, path))
            {
                result
                    .violations
                    .push(format!("changed file outside allowed_files: {path}"));
            }
        }
    }
    for path in changed_files {
        if task
            .forbidden_files
            .iter()
            .any(|pattern| globish_match(pattern, path))
        {
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

fn project_failure_reason(
    verification: &[ProjectVerificationResult],
    constraints: &ProjectConstraintResult,
    changed_files: &[String],
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
    if changed_files.is_empty() {
        return Some(FailureReason {
            category: FailureCategory::NoPatchApplied,
            message: "project task produced no workspace diff".to_string(),
            details: BTreeMap::new(),
        });
    }
    None
}

fn project_task_prompt(manifest: &ProjectManifest, task: &ProjectTask) -> String {
    let mut prompt = format!(
        "Project goal: {}\n\nTask {}: {}\n",
        manifest.project.goal, task.id, task.goal
    );
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

fn globish_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");
    if !pattern.contains('*') {
        return pattern == path;
    }
    let mut remainder = path.as_str();
    let mut first = true;
    for part in pattern.split('*') {
        if part.is_empty() {
            continue;
        }
        if first && !pattern.starts_with('*') {
            let Some(next) = remainder.strip_prefix(part) else {
                return false;
            };
            remainder = next;
        } else if let Some(index) = remainder.find(part) {
            remainder = &remainder[index + part.len()..];
        } else {
            return false;
        }
        first = false;
    }
    pattern.ends_with('*') || remainder.is_empty()
}

fn resolve_manifest_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn absolutize(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
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

fn default_profile() -> PathBuf {
    PathBuf::from(DEFAULT_CODE_PROFILE)
}

fn default_model_config() -> PathBuf {
    PathBuf::from(DEFAULT_MODEL_CONFIG)
}

fn default_tool_config() -> PathBuf {
    PathBuf::from(DEFAULT_TOOL_CONFIG)
}

fn default_artifact_dir() -> PathBuf {
    PathBuf::from(DEFAULT_ARTIFACT_DIR)
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
        normalize_project_manifest(&mut manifest, "refactor tools");
        validate_manifest(&manifest).unwrap();

        assert_eq!(manifest.tasks.len(), 1);
        assert_eq!(manifest.tasks[0].id, "split_file_tools");
        assert!(manifest.tasks[0]
            .forbidden_files
            .contains(&"target/**".to_string()));
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
        let _ = fs::remove_dir_all(dir);
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
