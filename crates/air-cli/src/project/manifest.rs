use air_code_artifact::FailureReason;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(super) const PROJECT_SCHEMA: &str = air_schemas::PROJECT;
pub(super) const PROJECT_STATE_SCHEMA: &str = air_schemas::PROJECT_STATE;
pub(super) const DEFAULT_PROJECT_FILE: &str = "air-project.yaml";
pub(super) const DEFAULT_CODE_PROFILE: &str = "skills/code-agent/edit.air-profile.yaml";
pub(super) const DEFAULT_MODEL_CONFIG: &str = "examples/local-openai-compatible.json";
pub(super) const DEFAULT_TOOL_CONFIG: &str = "skills/code-agent/tools.json";
pub(super) const DEFAULT_ARTIFACT_DIR: &str = ".air/project";
pub(super) const DEFAULT_PROJECT_BENCH_SUITE: &str =
    "benches/project/orchestrator-smoke/suite.json";

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

pub(crate) struct BenchProjectOptions {
    pub(crate) suite: Option<PathBuf>,
    pub(crate) out_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProjectManifest {
    pub(super) schema: String,
    pub(super) project: ProjectMetadata,
    #[serde(default)]
    pub(super) defaults: ProjectDefaults,
    pub(super) tasks: Vec<ProjectTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProjectMetadata {
    pub(super) name: String,
    pub(super) goal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProjectDefaults {
    #[serde(default = "default_profile")]
    pub(super) profile: PathBuf,
    #[serde(default = "default_model_config")]
    pub(super) model_config: PathBuf,
    #[serde(default = "default_tool_config")]
    pub(super) tool_config: PathBuf,
    #[serde(default = "default_artifact_dir")]
    pub(super) artifact_dir: PathBuf,
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
pub(super) struct ProjectTask {
    pub(super) id: String,
    pub(super) goal: String,
    #[serde(default)]
    pub(super) depends_on: Vec<String>,
    #[serde(default)]
    pub(super) skills: Vec<String>,
    #[serde(default)]
    pub(super) allowed_files: Vec<String>,
    #[serde(default)]
    pub(super) forbidden_files: Vec<String>,
    #[serde(default)]
    pub(super) verification: Vec<ProjectVerificationCommand>,
    #[serde(default)]
    pub(super) success_conditions: ProjectSuccessConditions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) max_changed_files: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) max_diff_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProjectVerificationCommand {
    pub(super) command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ProjectSuccessConditions {
    #[serde(default)]
    pub(super) required_changed_files: Vec<String>,
    #[serde(default)]
    pub(super) required_diff_contains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProjectState {
    pub(super) schema: String,
    pub(super) project: String,
    pub(super) tasks: BTreeMap<String, ProjectTaskState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProjectTaskStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProjectTaskState {
    pub(super) status: ProjectTaskStatus,
    pub(super) artifact_path: String,
    pub(super) changed_files: Vec<String>,
    #[serde(default)]
    pub(super) artifact_snapshot_matches_current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) patch_path: Option<String>,
    pub(super) verification: Vec<ProjectVerificationResult>,
    pub(super) constraints: ProjectConstraintResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) failure_reason: Option<FailureReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProjectVerificationResult {
    pub(super) command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    pub(super) success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status: Option<i32>,
    pub(super) stdout_preview: String,
    pub(super) stderr_preview: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ProjectConstraintResult {
    pub(super) passed: bool,
    pub(super) violations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ProjectRunOutput {
    pub(super) project: String,
    pub(super) state_path: String,
    pub(super) tasks: Vec<ProjectTaskRunOutput>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProjectBenchSuite {
    pub(super) name: String,
    #[serde(default)]
    pub(super) description: Option<String>,
    pub(super) cases: Vec<ProjectBenchCase>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ProjectBenchCase {
    DagOrder {
        id: String,
        manifest: ProjectManifest,
        expected_order: Vec<String>,
    },
    DependencyFailure {
        id: String,
        manifest: ProjectManifest,
        failed_task: String,
        target_task: String,
    },
    ConstraintCheck {
        id: String,
        task: ProjectTask,
        changed_files: Vec<String>,
        diff: String,
        expected_pass: bool,
        #[serde(default)]
        expected_violation_contains: Option<String>,
    },
    MissingArtifact {
        id: String,
        task: ProjectTask,
    },
    StaleArtifact {
        id: String,
        task: ProjectTask,
    },
}

#[derive(Debug, Serialize)]
pub(super) struct ProjectBenchRun {
    pub(super) suite: String,
    pub(super) description: Option<String>,
    pub(super) run_dir: String,
    pub(super) summary: ProjectBenchSummary,
    pub(super) cases: Vec<ProjectBenchCaseRun>,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct ProjectBenchSummary {
    pub(super) total: usize,
    pub(super) passed: usize,
    pub(super) failed: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct ProjectBenchCaseRun {
    pub(super) id: String,
    pub(super) kind: String,
    pub(super) pass: bool,
    pub(super) details: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ProjectTaskRunOutput {
    pub(super) id: String,
    pub(super) status: ProjectTaskStatus,
    pub(super) artifact_path: String,
    pub(super) changed_files: Vec<String>,
    pub(super) artifact_snapshot_matches_current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) patch_path: Option<String>,
    pub(super) verification_passed: bool,
    pub(super) constraints_passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) failure_reason: Option<FailureReason>,
}

pub(super) fn default_profile() -> PathBuf {
    PathBuf::from(DEFAULT_CODE_PROFILE)
}

pub(super) fn default_model_config() -> PathBuf {
    PathBuf::from(DEFAULT_MODEL_CONFIG)
}

pub(super) fn default_tool_config() -> PathBuf {
    PathBuf::from(DEFAULT_TOOL_CONFIG)
}

pub(super) fn default_artifact_dir() -> PathBuf {
    PathBuf::from(DEFAULT_ARTIFACT_DIR)
}
