use air_runtime::{system_return_event, Vm};
mod bench;
mod code_agent;
mod diagnostics;
mod entry;
mod explain;
mod mcp;
mod models;
mod ops;
mod planner;
mod profile;
mod project;
mod review_agent;
mod run_plan;
mod self_lab;
mod skill;
mod tools;
use crate::bench::{bench_code_agent, bench_skill, BenchCodeAgentOptions, BenchSkillOptions};
use crate::diagnostics::emit_diagnostics;
use crate::entry::{run_entry_task, EntryMode, EntryTaskOptions};
use crate::explain::{build_plan_explanation, format_plan_explanation};
use crate::mcp::{
    audit_mcp, call_mcp, explain_mcp, list_mcp, McpAuditOptions, McpCallOptions, McpExplainOptions,
    McpListOptions,
};
use crate::models::ModelProviderChoice;
use crate::ops::run_current_air_passthrough;
use crate::planner::{
    module_base_dir_for_modules, module_base_dir_for_store_path, plan_task, PlanOptions,
    ValidatePlanOptions,
};
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::project::{
    bench_project, default_project_file, project_plan, project_run, project_status, project_verify,
    BenchProjectOptions, ProjectPlanOptions, ProjectRunOptions, ProjectStatusOptions,
    ProjectVerifyOptions,
};
use crate::run_plan::{
    observe_event_with_trace_file, replay, resume_plan, run_plan, write_partial_trace, write_trace,
    ReplayOptions, ResumePlanOptions, RunPlanOptions,
};
use crate::self_lab::{
    self_capture, self_compare, self_fix, self_prepare, SelfCaptureOptions, SelfCompareOptions,
    SelfFixOptions, SelfPrepareOptions,
};
use crate::skill::{
    audit_skill, audit_skill_value, explain_skill, import_skill, list_skills, route_skill,
    upgrade_skill, validate_skill, validate_skill_value, SkillRouteOptions, SkillUpgradeOptions,
};
use crate::tools::ToolProviderChoice;
use air_audit::{audit_collect, audit_run, AuditCollectOptions, AuditRunOptions};
use air_code_artifact::write_timeout_code_run_artifact;
use air_dream::{
    dismiss_dream_finding, list_dream_findings, open_dream_finding, resolve_dream_finding,
    run_dream, show_dream_state, DreamFindingUpdateOptions, DreamFindingsListOptions, DreamMode,
    DreamRunOptions, DreamStateOptions,
};
use air_eval::{
    check_eval_manifest_command, write_eval_manifest, EvalCheckOptions, EvalManifestOptions,
};
use air_improve::{run_improve, ImproveAction, ImproveOptions};
use air_memory::{
    advance_memory, build_memory_pack, build_memory_run_hint, causal_eval_memory,
    check_policy_candidate, draft_skill_from_memory, evaluate_memory_skill, list_memory,
    pack_memory_command, promote_memory, retire_memory, search_memory, show_brain,
    show_memory_graph, show_memory_scorecard, view_memory, BrainOptions, BrainSection,
    MemoryAdvanceOptions, MemoryCausalEvalOptions, MemoryGraphOptions, MemoryHintOptions,
    MemoryListOptions, MemoryPackOptions, MemoryPolicyCheckOptions, MemoryPromoteOptions,
    MemoryRetireOptions, MemoryScorecardOptions, MemorySearchOptions, MemorySkillCheckRunner,
    MemorySkillDraftOptions, MemorySkillEvaluateOptions, MemoryViewOptions, StageCheck,
};
use air_regression::{run_regression_command, RegressionRunOptions};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::Duration;
use std::time::Instant;

fn load_dotenv() {
    let _ = dotenvy::from_filename(".env");
    apply_model_profile_env();
}

fn parse_nonzero_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|error| format!("expected a positive integer: {error}"))?;
    if parsed == 0 {
        return Err("value must be greater than 0".to_string());
    }
    Ok(parsed)
}

fn parse_dream_top_findings(value: &str) -> Result<usize, String> {
    let parsed = parse_nonzero_usize(value)?;
    if parsed > 10 {
        return Err("value must be between 1 and 10".to_string());
    }
    Ok(parsed)
}

fn parse_dream_candidates(value: &str) -> Result<usize, String> {
    let parsed = parse_nonzero_usize(value)?;
    if parsed > 12 {
        return Err("value must be between 1 and 12".to_string());
    }
    Ok(parsed)
}

struct CliMemorySkillCheckRunner;

impl CliMemorySkillCheckRunner {
    fn run_json_check(
        command: String,
        log_path: &Path,
        run: impl FnOnce() -> Result<Value>,
    ) -> Result<StageCheck> {
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        match run() {
            Ok(value) => {
                fs::write(log_path, serde_json::to_vec_pretty(&value)?)
                    .with_context(|| format!("write {}", log_path.display()))?;
                Ok(StageCheck {
                    command,
                    status: "passed".to_string(),
                    code: Some(0),
                    log: log_path.display().to_string(),
                })
            }
            Err(error) => {
                fs::write(log_path, format!("{error:#}\n"))
                    .with_context(|| format!("write {}", log_path.display()))?;
                Ok(StageCheck {
                    command,
                    status: "failed".to_string(),
                    code: Some(1),
                    log: log_path.display().to_string(),
                })
            }
        }
    }

    fn run_air_capture(
        command_label: String,
        args: &[&str],
        log_path: &Path,
        timeout: Option<Duration>,
    ) -> Result<StageCheck> {
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let exe = std::env::current_exe().context("resolve current air executable")?;
        let mut command = ProcessCommand::new(exe);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("run {command_label}"))?;
        let mut stdout_reader = child.stdout.take().map(read_child_stream);
        let mut stderr_reader = child.stderr.take().map(read_child_stream);
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().context("poll memory skill child")? {
                break status;
            }
            if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
                let _ = child.kill();
                let _ = child.wait();
                let stdout = join_child_stream(stdout_reader.take(), "stdout")?;
                let stderr = join_child_stream(stderr_reader.take(), "stderr")?;
                write_stage_log(log_path, &command_label, &stdout, &stderr, "timeout")?;
                return Ok(StageCheck {
                    command: command_label,
                    status: "timeout".to_string(),
                    code: None,
                    log: log_path.display().to_string(),
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let stdout = join_child_stream(stdout_reader.take(), "stdout")?;
        let stderr = join_child_stream(stderr_reader.take(), "stderr")?;
        let status_label = if status.success() { "passed" } else { "failed" };
        write_stage_log(log_path, &command_label, &stdout, &stderr, status_label)?;
        Ok(StageCheck {
            command: command_label,
            status: status_label.to_string(),
            code: status.code(),
            log: log_path.display().to_string(),
        })
    }
}

impl MemorySkillCheckRunner for CliMemorySkillCheckRunner {
    fn validate_skill(
        &self,
        draft_dir: &Path,
        log_path: &Path,
        _timeout: Option<Duration>,
    ) -> Result<StageCheck> {
        let command = format!("air skill validate {}", draft_dir.display());
        Self::run_json_check(command, log_path, || {
            validate_skill_value(&draft_dir.display().to_string())
        })
    }

    fn audit_skill(
        &self,
        draft_dir: &Path,
        log_path: &Path,
        _timeout: Option<Duration>,
    ) -> Result<StageCheck> {
        let command = format!("air skill audit {}", draft_dir.display());
        Self::run_json_check(command, log_path, || {
            audit_skill_value(&draft_dir.display().to_string(), false)
        })
    }

    fn bench_skill(
        &self,
        draft_dir: &Path,
        log_path: &Path,
        timeout: Option<Duration>,
    ) -> Result<StageCheck> {
        let draft = draft_dir.to_string_lossy().to_string();
        let command = format!("air bench skill {draft} --compare-no-skill --limit 1");
        Self::run_air_capture(
            command,
            &[
                "bench",
                "skill",
                draft.as_str(),
                "--compare-no-skill",
                "--limit",
                "1",
            ],
            log_path,
            timeout,
        )
    }
}

fn read_child_stream<R: Read + Send + 'static>(
    mut stream: R,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_child_stream(
    reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
    stream_name: &str,
) -> Result<Vec<u8>> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("memory skill child {stream_name} reader panicked"))?
        .with_context(|| format!("read memory skill child {stream_name}"))
}

fn write_stage_log(
    log_path: &Path,
    command: &str,
    stdout: &[u8],
    stderr: &[u8],
    status: &str,
) -> Result<()> {
    let mut log = String::new();
    log.push_str("$ ");
    log.push_str(command);
    log.push_str("\n\n[status]\n");
    log.push_str(status);
    log.push_str("\n\n[stdout]\n");
    log.push_str(&String::from_utf8_lossy(stdout));
    log.push_str("\n\n[stderr]\n");
    log.push_str(&String::from_utf8_lossy(stderr));
    fs::write(log_path, log).with_context(|| format!("write {}", log_path.display()))
}

fn apply_model_profile_env() {
    let Ok(profile) = std::env::var("AIR_MODEL_PROFILE") else {
        return;
    };
    let profile = normalize_model_profile_key(&profile);
    if profile.is_empty() {
        return;
    }
    for (target, suffix) in [
        ("OPENAI_API_KEY", "API_KEY"),
        ("OPENAI_BASE_URL", "BASE_URL"),
        ("OPENAI_MODEL", "MODEL"),
    ] {
        let source = format!("AIR_MODEL_{profile}_{suffix}");
        if let Ok(value) = std::env::var(source) {
            if !value.trim().is_empty() && std::env::var_os(target).is_none() {
                std::env::set_var(target, value);
            }
        }
    }
}

fn normalize_model_profile_key(profile: &str) -> String {
    profile
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[derive(Debug, Parser)]
#[command(name = "air")]
#[command(about = "AIR audit-first runtime CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TraceMode {
    Redact,
    Raw,
    Off,
}

impl TraceMode {
    fn redact(self) -> bool {
        !matches!(self, Self::Raw)
    }

    fn raw(self) -> bool {
        matches!(self, Self::Raw)
    }

    fn trace_out(self, trace_out: Option<PathBuf>) -> Option<PathBuf> {
        if matches!(self, Self::Off) {
            None
        } else {
            trace_out
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct AirCliConfig {
    #[serde(default)]
    model_config: Option<PathBuf>,
    #[serde(default)]
    memory_dir: Option<PathBuf>,
    #[serde(default)]
    dream_dir: Option<PathBuf>,
    #[serde(default)]
    defaults: Option<AirCliConfigDefaults>,
}

#[derive(Debug, Default, Deserialize)]
struct AirCliConfigDefaults {
    #[serde(default)]
    model_config: Option<PathBuf>,
    #[serde(default)]
    memory_dir: Option<PathBuf>,
    #[serde(default)]
    dream_dir: Option<PathBuf>,
}

impl AirCliConfig {
    fn load() -> Result<Self> {
        let path = Path::new(".air/config.yaml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        serde_yaml::from_str(&raw).with_context(|| format!("parse {}", path.display()))
    }

    fn model_config(&self) -> Option<PathBuf> {
        self.model_config
            .clone()
            .or_else(|| {
                self.defaults
                    .as_ref()
                    .and_then(|defaults| defaults.model_config.clone())
            })
            .or_else(|| std::env::var_os("AIR_MODEL_CONFIG").map(PathBuf::from))
    }

    fn memory_dir(&self) -> Option<PathBuf> {
        self.memory_dir.clone().or_else(|| {
            self.defaults
                .as_ref()
                .and_then(|defaults| defaults.memory_dir.clone())
        })
    }

    fn dream_dir(&self) -> Option<PathBuf> {
        self.dream_dir.clone().or_else(|| {
            self.defaults
                .as_ref()
                .and_then(|defaults| defaults.dream_dir.clone())
        })
    }
}

fn option_or_config(option: Option<PathBuf>, fallback: Option<PathBuf>) -> Option<PathBuf> {
    option.or(fallback)
}

/// Convenience helper that builds an LLM-advisory provider for the CLI to pass
/// into a library crate's option struct (e.g. `air-audit`).
fn open_advisory_provider(model_config: &Path) -> Result<Box<dyn air_runtime::ModelProvider>> {
    Ok(Box::new(crate::models::ModelProviderChoice::open_advisory(
        model_config,
    )?))
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Deterministically audit AIR run artifacts without calling a model.
    Audit {
        #[command(subcommand)]
        command: AuditCommand,
    },
    /// One-screen summary of the current AIR workspace state.
    Status {
        /// Emit machine-readable JSON instead of the human-readable view.
        #[arg(long)]
        json: bool,
    },
    /// Show the organized agent brain: memory, skill drafts, policies, findings, and candidates.
    Brain {
        #[command(subcommand)]
        command: Option<BrainCommand>,

        /// Memory directory.
        #[arg(long, global = true)]
        memory_dir: Option<PathBuf>,

        /// Dream directory.
        #[arg(long, global = true)]
        dream_dir: Option<PathBuf>,

        /// Maximum items per section.
        #[arg(short = 'l', long, global = true)]
        limit: Option<usize>,

        /// Emit machine-readable JSON instead of the human-readable view.
        #[arg(long, global = true)]
        json: bool,
    },
    /// Run or inspect installed AIR skills.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Offline review cycle that audits recent runs and mines improvement findings.
    Dream {
        #[command(subcommand)]
        command: DreamCommand,
    },
    /// Persistent Dream findings.
    Findings {
        #[command(subcommand)]
        command: DreamFindingsCommand,
    },
    /// Inspect evidence-backed Dream memory candidates.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Inspect and audit MCP tools declared in AIR tool configs.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Run AIR benchmark suites.
    Bench {
        #[command(subcommand)]
        command: BenchCommand,
    },
    /// Mine AIR artifacts and benchmark runs for failures and regression candidates.
    Improve {
        #[command(subcommand)]
        command: Option<ImproveCommand>,

        /// Artifact, benchmark, or generated output roots to scan.
        #[arg(short = 'f', long = "from", global = true)]
        from: Vec<PathBuf>,

        /// Output directory for observations, findings, suggested regressions, and report.
        #[arg(short = 'o', long, global = true)]
        out_dir: Option<PathBuf>,

        /// Write one suggested regression JSON file per finding.
        #[arg(long, global = true)]
        write_regressions: bool,
    },
    /// Run promoted AIR regression candidates.
    Regression {
        #[command(subcommand)]
        command: RegressionCommand,
    },
    /// Protect evaluation files with a content manifest.
    Eval {
        #[command(subcommand)]
        command: EvalCommand,
    },
    /// Prepare and compare self-improvement candidates.
    #[command(name = "self")]
    Self_ {
        #[command(subcommand)]
        command: SelfCommand,
    },
    /// Advanced AIR IR/runtime tools.
    Dev {
        #[command(subcommand)]
        command: DevCommand,
    },
    /// Run bounded multi-task coding projects.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Run a user task through AIR's entry agent.
    Run {
        /// Natural-language task.
        target: String,

        /// Entry routing mode for natural-language tasks.
        #[arg(long, value_enum, default_value_t = EntryMode::Auto)]
        mode: EntryMode,

        /// Number of instruction skills to route into the host agent.
        #[arg(long, default_value_t = 3, value_parser = parse_nonzero_usize, hide = true)]
        top_k: usize,

        /// Additional instruction skills to preload. Accepts repeated flags or comma-separated ids.
        #[arg(long = "skills", value_delimiter = ',')]
        skills: Vec<String>,

        /// Project manifest path when --mode project is selected.
        #[arg(long, hide = true)]
        project_file: Option<PathBuf>,

        /// For project mode, stop after writing the project manifest.
        #[arg(long, hide = true, conflicts_with = "execute")]
        plan_only: bool,

        /// For project mode, run the generated project manifest immediately.
        #[arg(long)]
        execute: bool,

        /// Project planner model alias from --model-config.
        #[arg(long, default_value = "project_planner", hide = true)]
        planner_model: String,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long, hide = true)]
        trace_out: Option<PathBuf>,

        /// Trace writing mode when --trace-out is set.
        #[arg(long, value_enum, default_value_t = TraceMode::Redact, hide = true)]
        trace: TraceMode,

        /// Print human-readable execution logs to stderr.
        #[arg(short = 'v', long)]
        log: bool,

        /// Maximum wall-clock seconds for this run.
        #[arg(long)]
        timeout_seconds: Option<u64>,

        /// Explain selected executor, skills, and permissions without running a model.
        #[arg(long)]
        explain: bool,

        /// Optional tool provider config JSON.
        #[arg(long, hide = true)]
        tool_config: Option<PathBuf>,

        /// Optional directory for an auditable code-run artifact.
        #[arg(long, hide = true)]
        artifact_out: Option<PathBuf>,

        /// Replay a previous code-run artifact, optionally switching to live execution with --replay-from.
        #[arg(long, hide = true)]
        replay_artifact: Option<PathBuf>,

        /// 1-based trace event line where replay should switch to the live provider.
        #[arg(long, requires = "replay_artifact", hide = true)]
        replay_from: Option<usize>,

        /// Deterministic verification command for code-agent verify phase.
        #[arg(long, hide = true)]
        verification_command: Option<String>,

        /// Disable promoted Dream memory injection for this run.
        #[arg(long)]
        no_memory: bool,

        /// Memory directory for default Dream memory injection.
        #[arg(long, hide = true)]
        memory_dir: Option<PathBuf>,

        /// Maximum promoted memories to include with default Dream memory injection.
        #[arg(long, default_value_t = 5, hide = true)]
        memory_limit: usize,
    },
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    /// List local AIR skills.
    List,
    /// Validate an AIR skill manifest.
    Validate {
        /// Skill id, manifest path, or skill directory.
        skill: String,
    },
    /// Audit an AIR skill package without executing it.
    Audit {
        /// Skill id, manifest path, or skill directory.
        skill: String,

        /// Write audit.json next to the skill manifest.
        #[arg(long)]
        write: bool,
    },
    /// Import a local folder, git repository, or zip as AIR skill package(s).
    Import {
        /// Local directory/path, git URL, or zip URL/path.
        source: String,

        /// Destination skill directory for one skill, or destination root for a skill collection.
        /// Defaults to skills/vendor for collections and skills/vendor/<skill-id> for single skills.
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    /// Check what would change if an imported skill was upgraded from its source.
    Upgrade {
        /// Skill id, manifest path, or skill directory.
        skill: String,

        /// Show the upgrade diff without changing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Route a task to the most relevant local AIR skills.
    Route {
        /// Natural-language task.
        task: String,

        /// Number of selected skill cards to return.
        #[arg(short = 'k', long, default_value_t = 3)]
        top_k: usize,

        /// Include deterministic routing score details.
        #[arg(long)]
        explain: bool,

        /// Disable promoted Dream memory pack in routing output.
        #[arg(long)]
        no_memory: bool,

        /// Memory directory for Dream memory.
        #[arg(long, hide = true)]
        memory_dir: Option<PathBuf>,

        /// Maximum promoted memories to include.
        #[arg(long, default_value_t = 5, hide = true)]
        memory_limit: usize,
    },
    /// Explain what a skill is allowed to do without running it.
    Explain {
        /// Skill id, manifest path, or skill directory.
        skill: String,

        /// Skill run profile override.
        #[arg(long)]
        profile: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum AuditCommand {
    /// Audit a code-run artifact directory containing artifact.json and trace.jsonl.
    Run {
        /// Code-run artifact directory.
        artifact_dir: PathBuf,

        /// Optional Markdown audit report path.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Collect and summarize many code-run artifact audits for a time window or run set.
    Collect {
        /// Artifact, benchmark, or generated output roots to scan.
        #[arg(short = 'f', long = "from")]
        from: Vec<PathBuf>,

        /// Only audit artifacts whose artifact.json mtime is at or after this Unix timestamp.
        #[arg(long)]
        since_unix: Option<u64>,

        /// Audit at most this many newest artifacts after applying the time window.
        #[arg(short = 'l', long)]
        limit: Option<usize>,

        /// Output directory for collection.json, per-run audits, and default report.md.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,

        /// Optional Markdown collection report path.
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum EvalCommand {
    /// Write an eval integrity manifest with sha256 pins.
    Manifest {
        /// Output manifest path.
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,

        /// File or directory to include. Defaults to AIR benchmark/regression files.
        #[arg(long)]
        include: Vec<PathBuf>,
    },
    /// Check the eval integrity manifest and protected git diff paths.
    Check {
        /// Eval manifest path.
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum SelfCommand {
    /// Create candidate directories for a finding.
    Prepare {
        /// Finding id such as IMP-001.
        finding: String,

        /// Number of candidate slots to create.
        #[arg(long, default_value_t = 3)]
        candidates: usize,

        /// Candidate root directory.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,
    },
    /// Capture the current workspace diff as a candidate patch.
    Capture {
        /// Finding id such as IMP-001.
        finding: String,

        /// Candidate id such as cand-1.
        candidate: String,

        /// Candidate root directory.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,
    },
    /// Generate candidate patches with AIR's code-agent in isolated worktrees.
    Fix {
        /// Finding id such as IMP-001.
        finding: String,

        /// Number of candidate patches to generate.
        #[arg(long, default_value_t = 1)]
        candidates: usize,

        /// Limit the candidate improvement prompt to a local AIR skill package.
        #[arg(long)]
        skill: Option<String>,

        /// Override the generated fix task prompt.
        #[arg(long)]
        task: Option<String>,

        /// Artifact, benchmark, or generated output roots to use during evaluation.
        #[arg(short = 'f', long = "from")]
        from: Vec<PathBuf>,

        /// Candidate root directory.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional regression JSON file to evaluate instead of a promoted regression.
        #[arg(long = "regression-file")]
        regression_file: Option<PathBuf>,

        /// Evaluate each generated candidate in its isolated worktree.
        #[arg(long)]
        evaluate: bool,

        /// Keep temporary candidate git worktrees for manual inspection.
        #[arg(long)]
        keep_worktrees: bool,

        /// Allow bootstrapping uncommitted workspace changes into candidate baselines.
        #[arg(long)]
        allow_dirty_bootstrap: bool,
    },
    /// Compare candidate eval.json files.
    Compare {
        /// Finding id such as IMP-001.
        finding: String,

        /// Candidate directory or eval.json path. Defaults to .air/candidates/<finding>.
        #[arg(short = 'f', long = "from")]
        from: Vec<PathBuf>,

        /// Optional Markdown scorecard path.
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum DreamCommand {
    /// Audit a run window and mine improvement findings without changing source code.
    Run {
        /// Artifact, benchmark, or generated output roots to scan.
        #[arg(short = 'f', long = "from")]
        from: Vec<PathBuf>,

        /// Only include artifacts whose artifact.json mtime is at or after this Unix timestamp.
        #[arg(long)]
        since_unix: Option<u64>,

        /// Review at most this many newest artifacts after applying the time window.
        #[arg(short = 'l', long)]
        limit: Option<usize>,

        /// Output directory for dream.json, dream.md, audit/, improve/, and logs/.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,

        /// Write one suggested regression JSON file per finding under the Dream improve output.
        #[arg(long)]
        write_regressions: bool,

        /// Memory synthesis depth: micro records episodes, deep mines patterns, evolution adds cross-domain proposals.
        #[arg(long, value_enum, default_value = "deep")]
        mode: DreamMode,

        /// Run self-improvement experiments for the top findings after Dream mining.
        #[arg(long, requires = "write_regressions")]
        experiment: bool,

        /// Number of findings to experiment on when --experiment is set.
        #[arg(long, default_value_t = 1, value_parser = parse_dream_top_findings)]
        top_findings: usize,

        /// Maximum wall-clock seconds for all --experiment work.
        #[arg(long)]
        budget_seconds: Option<u64>,

        /// Candidate patches per finding when --experiment is set.
        #[arg(long, default_value_t = 1, value_parser = parse_dream_candidates)]
        candidates: usize,

        /// Disable the safe memory advancement pass after extraction.
        #[arg(long)]
        no_advance: bool,

        /// Maximum memory cards to process during the advancement pass.
        #[arg(long, default_value_t = 25)]
        advance_limit: usize,

        /// Maximum wall-clock seconds for the memory advancement pass.
        #[arg(long)]
        advance_timeout_seconds: Option<u64>,

        /// Ignore Dream state and scan the full requested roots.
        #[arg(long, conflicts_with = "since_unix")]
        full: bool,
    },
    /// Show the saved Dream incremental state.
    State,
    /// Inspect or update persistent Dream findings.
    Findings {
        #[command(subcommand)]
        command: DreamFindingsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DreamFindingsCommand {
    /// List persistent findings.
    List {
        /// Filter by lifecycle status: open, resolved, or dismissed.
        #[arg(long)]
        status: Option<String>,

        /// Maximum records to return.
        #[arg(short = 'l', long)]
        limit: Option<usize>,
    },
    /// Reopen a finding.
    Open {
        /// Finding id.
        finding: String,

        /// Human-readable reason.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Mark a finding resolved.
    Resolve {
        /// Finding id.
        finding: String,

        /// Human-readable reason.
        #[arg(long)]
        reason: Option<String>,

        /// Candidate, PR, commit, or patch that fixed it.
        #[arg(long)]
        fixed_by: Option<String>,
    },
    /// Dismiss a finding.
    Dismiss {
        /// Finding id.
        finding: String,

        /// Human-readable reason.
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum MemoryCommand {
    /// List memory cards created by Dream.
    List {
        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Filter by memory kind such as failure, procedure, or routing.
        #[arg(long)]
        kind: Option<String>,

        /// Filter by lifecycle status such as candidate or promoted.
        #[arg(long)]
        status: Option<String>,

        /// Maximum cards to return.
        #[arg(short = 'l', long)]
        limit: Option<usize>,
    },
    /// Search memory cards.
    Search {
        /// Search query.
        query: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Filter by memory kind such as failure, procedure, or routing.
        #[arg(long)]
        kind: Option<String>,

        /// Filter by lifecycle status such as candidate or promoted.
        #[arg(long)]
        status: Option<String>,

        /// Maximum cards to return.
        #[arg(short = 'l', long)]
        limit: Option<usize>,
    },
    /// View one memory card.
    View {
        /// Memory id.
        id: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Include evidence pointers in the output.
        #[arg(long)]
        evidence: bool,
    },
    /// Promote candidate memory after review.
    Promote {
        /// Memory id.
        id: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Promotion status.
        #[arg(long)]
        status: Option<String>,
    },
    /// Record causal compare-no-memory/replay evidence for promotion.
    CausalEval {
        /// Memory id.
        id: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Causal outcome: helped, hurt, or neutral.
        #[arg(long)]
        outcome: String,

        /// Evidence artifact, such as compare-no-memory output or replay/bench report.
        #[arg(long)]
        evidence: PathBuf,

        /// Optional note describing the comparison.
        #[arg(long)]
        note: Option<String>,
    },
    /// Retire stale or harmful memory.
    Retire {
        /// Memory id.
        id: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Human-readable retirement reason.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Build a compact context pack from promoted memory for a task.
    Pack {
        /// Natural-language task.
        task: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Maximum promoted memories to include.
        #[arg(short = 'l', long)]
        limit: Option<usize>,

        /// Include evidence pointers in JSON output.
        #[arg(long)]
        evidence: bool,
    },
    /// Show experience graph edges produced by Dream synthesis.
    Graph {
        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Maximum graph edges to return.
        #[arg(short = 'l', long)]
        limit: Option<usize>,
    },
    /// Show helped/hurt usage scorecard for promoted memory.
    Scorecard {
        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Maximum rows to return.
        #[arg(short = 'l', long)]
        limit: Option<usize>,
    },
    /// Advance evidence-backed memory into promoted memory, validated skill drafts, routing measurements, and guard proposals.
    Advance {
        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Output directory for advance reports and review artifacts.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,

        /// Also run a small compare-no-skill benchmark for generated skill drafts.
        #[arg(long)]
        skill_bench: bool,

        /// Maximum memory cards to process.
        #[arg(short = 'l', long)]
        limit: Option<usize>,

        /// Maximum wall-clock seconds for skill validation/audit work during this pass.
        #[arg(long)]
        timeout_seconds: Option<u64>,
    },
    /// Draft an untrusted skill package from procedure memory.
    SkillDraft {
        /// Procedure memory id.
        id: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Destination draft skill directory.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,
    },
    /// Draft and validate/audit a skill from procedure memory; benchmark is opt-in.
    SkillEvaluate {
        /// Procedure memory id.
        id: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,

        /// Destination draft skill directory.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,

        /// Also run a small benchmark comparison for this draft.
        #[arg(long)]
        bench: bool,
    },
    /// Check whether a policy memory can become a deterministic guard proposal.
    PolicyCheck {
        /// Policy memory id.
        id: String,

        /// Memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum BrainCommand {
    /// Show organized memory cards by lifecycle status.
    Memory,
    /// Show installed skills and Dream-compiled skill drafts.
    Skills,
    /// Show policy memory and reviewed runtime guard proposals.
    Policies,
    /// Show persistent Dream findings by lifecycle status.
    Findings,
    /// Show self-improvement candidates produced by Dream experiments.
    Candidates,
    /// View one memory, skill draft, guard proposal, or finding.
    View {
        /// Memory id, skill id, guard memory id, or finding id.
        id: String,
    },
    /// Write a Markdown report of the organized agent brain.
    Report {
        /// Output Markdown path.
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    /// List MCP tools declared in a tool config.
    List {
        /// AIR tool config JSON.
        #[arg(long, default_value = "skills/code-agent/tools.json")]
        tool_config: PathBuf,
    },
    /// Explain one MCP tool declaration without connecting to it.
    Explain {
        /// MCP tool name from the AIR tool config.
        tool: String,

        /// AIR tool config JSON.
        #[arg(long, default_value = "skills/code-agent/tools.json")]
        tool_config: PathBuf,
    },
    /// Audit MCP tool declarations for auth, transport, and capability risk.
    Audit {
        /// Optional MCP tool name from the AIR tool config.
        tool: Option<String>,

        /// AIR tool config JSON.
        #[arg(long, default_value = "skills/code-agent/tools.json")]
        tool_config: PathBuf,
    },
    /// Call an MCP tool through AIR's configured MCP adapter.
    Call {
        /// AIR tool config tool name.
        tool: String,

        /// AIR tool config JSON.
        #[arg(long, default_value = "skills/code-agent/tools.json")]
        tool_config: PathBuf,

        /// JSON object passed to the AIR MCP tool. Defaults to list_tools.
        #[arg(long)]
        input_json: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    /// Run the code-agent benchmark suite.
    Code {
        /// Benchmark suite JSON file.
        #[arg(long)]
        suite: Option<PathBuf>,

        /// Output directory for run.json, traces, and workdirs.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,

        /// Coding-agent run profile.
        #[arg(long)]
        profile: Option<PathBuf>,

        /// OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Run only one task id.
        #[arg(long)]
        task: Option<String>,

        /// Run at most N selected tasks.
        #[arg(short = 'l', long)]
        limit: Option<usize>,

        /// Print AIR execution logs while the benchmark runs.
        #[arg(short = 'v', long)]
        log: bool,

        /// Keep successful task workdirs. Failed task workdirs are always kept.
        #[arg(long)]
        keep_workdirs: bool,

        /// Ignore cached code-run artifacts and spend model calls again.
        #[arg(long)]
        refresh: bool,

        /// Optional Markdown benchmark report output path.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Run a code-agent suite with a skill preloaded, optionally comparing no-skill.
    Skill {
        /// Skill id, manifest path, skill directory, or comma-separated instruction skill ids.
        skill: String,

        /// Additional instruction skills to preload.
        #[arg(long, value_delimiter = ',')]
        skills: Vec<String>,

        /// Benchmark suite JSON file.
        #[arg(long)]
        suite: Option<PathBuf>,

        /// Output directory for run.json, traces, and workdirs.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,

        /// OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Run only one task id.
        #[arg(long)]
        task: Option<String>,

        /// Run at most N selected tasks.
        #[arg(short = 'l', long)]
        limit: Option<usize>,

        /// Compare against a no-skill baseline in the same benchmark run.
        #[arg(long)]
        compare_no_skill: bool,

        /// Print AIR execution logs while the benchmark runs.
        #[arg(short = 'v', long)]
        log: bool,

        /// Keep successful task workdirs. Failed task workdirs are always kept.
        #[arg(long)]
        keep_workdirs: bool,

        /// Ignore cached code-run artifacts and spend model calls again.
        #[arg(long)]
        refresh: bool,

        /// Optional Markdown benchmark report output path.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Run deterministic project-orchestrator benchmark cases.
    #[command(hide = true)]
    Project {
        /// Project benchmark suite JSON file.
        #[arg(long)]
        suite: Option<PathBuf>,

        /// Output directory for run.json.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum ImproveCommand {
    /// Pick the highest-priority finding and show the next action.
    Next,
    /// Rerun the smallest known benchmark reproduction for a finding.
    Check {
        /// Finding id such as IMP-001.
        finding: String,
    },
    /// Write a suggested regression candidate into the repo for review.
    Promote {
        /// Finding id such as IMP-001.
        finding: String,
    },
    /// Prepare the next safe fix step without changing source code automatically.
    Fix {
        /// Optional finding id. Defaults to the highest-priority finding.
        finding: Option<String>,
    },
    /// Evaluate the current candidate patch against regressions and hard gates.
    Evaluate {
        /// Finding id such as IMP-001.
        finding: String,

        /// Optional regression JSON file to evaluate instead of a promoted regression.
        #[arg(long = "regression-file")]
        regression_file: Option<PathBuf>,

        /// Optional markdown report path.
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum RegressionCommand {
    /// Run one or more promoted regressions.
    Run {
        /// Finding id such as IMP-001. Omit with --all to run every regression.
        finding: Option<String>,

        /// Run all regression JSON files.
        #[arg(long)]
        all: bool,

        /// Run a specific regression JSON file.
        #[arg(long)]
        file: Option<PathBuf>,

        /// Artifact, benchmark, or generated output roots used by regression checks.
        #[arg(short = 'f', long = "from")]
        from: Vec<PathBuf>,

        /// Output directory for regression run artifacts.
        #[arg(short = 'o', long)]
        out_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum DevCommand {
    /// Parse and statically verify an AIR module.
    ValidateModule {
        /// Path to a .air.yaml, .air.yml, or .air.json file.
        file: PathBuf,
    },
    /// Parse and statically verify an AIR system.
    ValidateSystem {
        /// Path to a .air-system.yaml file.
        file: PathBuf,
    },
    /// Parse and statically verify a dynamic AIR run plan against a module store.
    ValidatePlan {
        /// Path to a .air-plan.yaml file.
        plan: Option<PathBuf>,

        /// Optional run profile containing plan and store paths.
        #[arg(long)]
        profile: Option<PathBuf>,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: Option<PathBuf>,

        /// Print a governance and compatibility summary for the validated run plan.
        #[arg(long)]
        explain: bool,
    },
    /// Ask a planner model to generate a dynamic AIR run plan from a task.
    MakePlan {
        /// Natural-language task to plan.
        #[arg(long, conflicts_with = "task_file")]
        task: Option<String>,

        /// File containing the natural-language task to plan.
        #[arg(long, conflicts_with = "task")]
        task_file: Option<PathBuf>,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: PathBuf,

        /// OpenAI-compatible model config JSON containing the planner model alias.
        #[arg(long, required_unless_present = "explain")]
        model_config: Option<PathBuf>,

        /// Planner model alias from --model-config.
        #[arg(long, default_value = "planner")]
        planner_model: String,

        /// Include internal primitive modules in the planner catalog.
        #[arg(long)]
        allow_internal: bool,

        /// Print local component-selection reasoning without calling the planner model.
        #[arg(long)]
        explain: bool,

        /// Optional output path. Prints YAML to stdout when omitted.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
    /// Run a state-machine AIR module directly.
    RunModule {
        /// Path to a .air.yaml, .air.yml, or .air.json file.
        file: PathBuf,

        /// Path to a JSON object containing module inputs.
        #[arg(long)]
        input: PathBuf,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Trace writing mode when --trace-out is set.
        #[arg(long, value_enum, default_value_t = TraceMode::Redact)]
        trace: TraceMode,

        /// Print human-readable execution logs to stderr.
        #[arg(short = 'v', long)]
        log: bool,

        /// Use built-in example tools such as docs.search.
        #[arg(long)]
        example_tools: bool,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Run an AIR system DAG with built-in mock providers.
    RunSystem {
        /// Path to a .air-system.yaml file.
        file: PathBuf,

        /// Path to a JSON object containing system inputs.
        #[arg(long)]
        input: PathBuf,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Trace writing mode when --trace-out is set.
        #[arg(long, value_enum, default_value_t = TraceMode::Redact)]
        trace: TraceMode,

        /// Print human-readable execution logs to stderr.
        #[arg(short = 'v', long)]
        log: bool,

        /// Use built-in example tools such as docs.search.
        #[arg(long)]
        example_tools: bool,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Run a dynamic AIR run plan against a module store.
    RunPlan {
        /// Path to a .air-plan.yaml file.
        plan: Option<PathBuf>,

        /// Optional run profile containing plan, store, input, and provider config.
        #[arg(long)]
        profile: Option<PathBuf>,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: Option<PathBuf>,

        /// Path to a JSON object containing plan inputs.
        #[arg(long)]
        input: Option<PathBuf>,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Trace writing mode when --trace-out is set.
        #[arg(long, value_enum, default_value_t = TraceMode::Redact)]
        trace: TraceMode,

        /// Optional JSON state output path for AIR resume.
        #[arg(long)]
        state_out: Option<PathBuf>,

        /// Optional JSON state checkpoint path updated after each completed module.
        #[arg(long)]
        checkpoint_out: Option<PathBuf>,

        /// Optional JIT cache directory for trace-specialized hot-path RunPlans.
        #[arg(long)]
        jit_cache: Option<PathBuf>,

        /// Execute eligible schedule groups concurrently with independent provider instances.
        #[arg(long)]
        parallel: bool,

        /// Print human-readable execution logs to stderr.
        #[arg(short = 'v', long)]
        log: bool,

        /// Use built-in example tools such as docs.search.
        #[arg(long)]
        example_tools: bool,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Resume a dynamic AIR run plan from a saved AIR state.
    ResumePlan {
        /// Path to a .air-plan.yaml file.
        plan: Option<PathBuf>,

        /// Optional run profile containing plan, store, input, and provider config.
        #[arg(long)]
        profile: Option<PathBuf>,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: Option<PathBuf>,

        /// Path to a JSON object containing plan inputs.
        #[arg(long)]
        input: Option<PathBuf>,

        /// Path to a JSON state file produced by run-plan --state-out.
        #[arg(long)]
        state: PathBuf,

        /// Override an output endpoint before resuming, as endpoint=json.
        #[arg(long = "override")]
        overrides: Vec<String>,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Trace writing mode when --trace-out is set.
        #[arg(long, value_enum, default_value_t = TraceMode::Redact)]
        trace: TraceMode,

        /// Optional JSON state output path for AIR resume.
        #[arg(long)]
        state_out: Option<PathBuf>,

        /// Optional JSON state checkpoint path updated after each completed module.
        #[arg(long)]
        checkpoint_out: Option<PathBuf>,

        /// Print human-readable execution logs to stderr.
        #[arg(short = 'v', long)]
        log: bool,

        /// Use built-in example tools such as docs.search.
        #[arg(long)]
        example_tools: bool,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Replay final output from a JSONL trace.
    Replay {
        /// Path to a trace JSONL file.
        trace: PathBuf,

        /// Specialize the trace back into a validated AIR run plan.
        #[arg(long)]
        specialize_run_plan: bool,

        /// Path to a .air-store.yaml file used for trace specialization validation.
        #[arg(long)]
        store: Option<PathBuf>,

        /// Optional specialized .air-plan.yaml output path. Prints YAML when omitted.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,

        /// Optional cache identity JSON output path for specialized traces.
        #[arg(long)]
        identity_out: Option<PathBuf>,

        /// Print trace statistics and final output as JSON.
        #[arg(long)]
        stats: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Create an editable AIR project manifest draft.
    Plan {
        /// Project goal to turn into an editable task manifest.
        goal: String,

        /// Output project manifest path. Prints YAML to stdout when omitted.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,

        /// OpenAI-compatible model config JSON for project task decomposition.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Planner model alias from --model-config.
        #[arg(long, default_value = "project_planner")]
        planner_model: String,

        /// Only write a one-task manifest template without calling a model.
        #[arg(long)]
        template: bool,
    },
    /// Run pending project tasks in dependency order, or one selected task.
    Run {
        /// Project manifest path.
        #[arg(long, default_value_os_t = default_project_file())]
        file: PathBuf,

        /// Run one task id instead of all pending tasks.
        #[arg(long)]
        task: Option<String>,

        /// Print AIR execution logs while tasks run.
        #[arg(short = 'v', long)]
        log: bool,
    },
    /// Show project task state without calling a model.
    Status {
        /// Project manifest path.
        #[arg(long, default_value_os_t = default_project_file())]
        file: PathBuf,
    },
    /// Re-run deterministic verification and diff constraints without calling a model.
    Verify {
        /// Project manifest path.
        #[arg(long, default_value_os_t = default_project_file())]
        file: PathBuf,

        /// Verify one task id instead of all tasks.
        #[arg(long)]
        task: Option<String>,
    },
}

fn main() -> Result<()> {
    load_dotenv();
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    let cli = Cli::parse();
    let config = AirCliConfig::load()?;

    if let Command::Run {
        timeout_seconds: Some(timeout_seconds),
        target,
        artifact_out,
        ..
    } = &cli.command
    {
        if std::env::var_os("AIR_RUN_TIMEOUT_CHILD").is_none() {
            return run_current_air_with_timeout(
                &raw_args,
                target,
                artifact_out.as_deref(),
                *timeout_seconds,
            );
        }
    }

    match cli.command {
        Command::Skill { command } => match command {
            SkillCommand::List => list_skills(),
            SkillCommand::Validate { skill } => validate_skill(&skill),
            SkillCommand::Audit { skill, write } => audit_skill(&skill, write),
            SkillCommand::Import { source, out } => import_skill(&source, out.as_deref()),
            SkillCommand::Upgrade { skill, dry_run } => {
                upgrade_skill(SkillUpgradeOptions { skill, dry_run })
            }
            SkillCommand::Route {
                task,
                top_k,
                explain,
                no_memory,
                memory_dir,
                memory_limit,
            } => route_skill(SkillRouteOptions {
                memory_pack: if !no_memory {
                    Some(serde_json::to_value(build_memory_pack(
                        MemoryPackOptions {
                            memory_dir: option_or_config(memory_dir, config.memory_dir()),
                            task: task.clone(),
                            limit: Some(memory_limit),
                            include_evidence: false,
                            record_usage: false,
                        },
                    )?)?)
                } else {
                    None
                },
                task,
                top_k,
                explain,
                advisory_provider: config
                    .model_config()
                    .as_deref()
                    .map(open_advisory_provider)
                    .transpose()?,
            }),
            SkillCommand::Explain { skill, profile } => explain_skill(&skill, profile),
        },
        Command::Audit { command } => match command {
            AuditCommand::Run {
                artifact_dir,
                report,
            } => audit_run(AuditRunOptions {
                artifact_dir,
                report,
                diagnose_provider: config
                    .model_config()
                    .as_deref()
                    .map(open_advisory_provider)
                    .transpose()?,
            }),
            AuditCommand::Collect {
                from,
                since_unix,
                limit,
                out_dir,
                report,
            } => audit_collect(AuditCollectOptions {
                from,
                out_dir,
                report,
                since_unix,
                limit,
                emit_stdout: true,
            }),
        },
        Command::Status { json } => show_status(json, &config),
        Command::Mcp { command } => match command {
            McpCommand::List { tool_config } => list_mcp(McpListOptions { tool_config }),
            McpCommand::Explain { tool, tool_config } => {
                explain_mcp(McpExplainOptions { tool, tool_config })
            }
            McpCommand::Audit { tool, tool_config } => {
                audit_mcp(McpAuditOptions { tool, tool_config })
            }
            McpCommand::Call {
                tool,
                tool_config,
                input_json,
            } => call_mcp(McpCallOptions {
                tool,
                tool_config,
                input_json,
            }),
        },
        Command::Bench { command } => match command {
            BenchCommand::Code {
                suite,
                out_dir,
                profile,
                model_config,
                task,
                limit,
                log,
                keep_workdirs,
                refresh,
                report,
            } => bench_code_agent(BenchCodeAgentOptions {
                suite,
                out_dir,
                profile,
                model_config: option_or_config(model_config, config.model_config()),
                task,
                limit,
                log,
                keep_workdirs,
                refresh,
                report,
            }),
            BenchCommand::Skill {
                skill,
                skills,
                suite,
                out_dir,
                model_config,
                task,
                limit,
                compare_no_skill,
                log,
                keep_workdirs,
                refresh,
                report,
            } => bench_skill(BenchSkillOptions {
                skill,
                skills,
                suite,
                out_dir,
                model_config: option_or_config(model_config, config.model_config()),
                task,
                limit,
                compare_no_skill,
                log,
                keep_workdirs,
                refresh,
                report,
            }),
            BenchCommand::Project { suite, out_dir } => {
                bench_project(BenchProjectOptions { suite, out_dir })
            }
        },
        Command::Improve {
            command,
            from,
            out_dir,
            write_regressions,
        } => run_improve(ImproveOptions {
            action: improve_action(command),
            from,
            out_dir,
            write_regressions,
            emit_stdout: true,
            advisory_provider: config
                .model_config()
                .as_deref()
                .map(open_advisory_provider)
                .transpose()?,
        }),
        Command::Regression { command } => match command {
            RegressionCommand::Run {
                finding,
                all,
                file,
                from,
                out_dir,
            } => run_regression_command(RegressionRunOptions {
                finding,
                file,
                all,
                from,
                out_dir,
            }),
        },
        Command::Dream { command } => match command {
            DreamCommand::Run {
                from,
                since_unix,
                limit,
                out_dir,
                write_regressions,
                mode,
                experiment,
                top_findings,
                budget_seconds,
                candidates,
                no_advance,
                advance_limit,
                advance_timeout_seconds,
                full,
            } => run_dream(DreamRunOptions {
                from,
                since_unix,
                limit,
                out_dir,
                write_regressions,
                full,
                mode,
                improve_advisory_provider: if mode != DreamMode::Micro
                    && !air_advisory::is_disabled()
                {
                    config
                        .model_config()
                        .as_ref()
                        .map(|path| open_advisory_provider(path))
                        .transpose()?
                } else {
                    None
                },
                dream_synthesis_provider: if mode != DreamMode::Micro
                    && !air_advisory::is_disabled()
                {
                    config
                        .model_config()
                        .as_ref()
                        .map(|path| open_advisory_provider(path))
                        .transpose()?
                } else {
                    None
                },
                experiment,
                top_findings,
                budget_seconds,
                candidates,
                advance: !no_advance,
                advance_limit: Some(advance_limit),
                advance_timeout_seconds,
                memory_skill_runner: Some(Box::new(CliMemorySkillCheckRunner)),
            }),
            DreamCommand::State => show_dream_state(DreamStateOptions),
            DreamCommand::Findings { command } => run_dream_findings_command(command),
        },
        Command::Findings { command } => run_dream_findings_command(command),
        Command::Memory { command } => match command {
            MemoryCommand::List {
                memory_dir,
                kind,
                status,
                limit,
            } => list_memory(MemoryListOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                kind,
                status,
                limit,
            }),
            MemoryCommand::Search {
                query,
                memory_dir,
                kind,
                status,
                limit,
            } => search_memory(MemorySearchOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                query,
                kind,
                status,
                limit,
            }),
            MemoryCommand::View {
                id,
                memory_dir,
                evidence,
            } => view_memory(MemoryViewOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                id,
                evidence,
            }),
            MemoryCommand::Promote {
                id,
                memory_dir,
                status,
            } => promote_memory(MemoryPromoteOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                id,
                status,
            }),
            MemoryCommand::CausalEval {
                id,
                memory_dir,
                outcome,
                evidence,
                note,
            } => causal_eval_memory(MemoryCausalEvalOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                id,
                outcome,
                evidence,
                note,
            }),
            MemoryCommand::Retire {
                id,
                memory_dir,
                reason,
            } => retire_memory(MemoryRetireOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                id,
                reason,
            }),
            MemoryCommand::Pack {
                task,
                memory_dir,
                limit,
                evidence,
            } => pack_memory_command(MemoryPackOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                task,
                limit,
                include_evidence: evidence,
                record_usage: false,
            }),
            MemoryCommand::Graph { memory_dir, limit } => show_memory_graph(MemoryGraphOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                limit,
            }),
            MemoryCommand::Scorecard { memory_dir, limit } => {
                show_memory_scorecard(MemoryScorecardOptions {
                    memory_dir: option_or_config(memory_dir, config.memory_dir()),
                    limit,
                })
            }
            MemoryCommand::Advance {
                memory_dir,
                out_dir,
                skill_bench,
                limit,
                timeout_seconds,
            } => {
                let output = advance_memory(MemoryAdvanceOptions {
                    memory_dir: option_or_config(memory_dir, config.memory_dir()),
                    out_dir,
                    skill_bench,
                    limit,
                    timeout_seconds,
                    skill_runner: Some(Box::new(CliMemorySkillCheckRunner)),
                })?;
                println!("{}", serde_json::to_string_pretty(&output)?);
                Ok(())
            }
            MemoryCommand::SkillDraft {
                id,
                memory_dir,
                out_dir,
            } => draft_skill_from_memory(MemorySkillDraftOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                id,
                out_dir,
            }),
            MemoryCommand::SkillEvaluate {
                id,
                memory_dir,
                out_dir,
                bench,
            } => evaluate_memory_skill(MemorySkillEvaluateOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                id,
                out_dir,
                bench,
                skill_runner: Some(Box::new(CliMemorySkillCheckRunner)),
            }),
            MemoryCommand::PolicyCheck { id, memory_dir } => {
                check_policy_candidate(MemoryPolicyCheckOptions {
                    memory_dir: option_or_config(memory_dir, config.memory_dir()),
                    id,
                })
            }
        },
        Command::Brain {
            command,
            memory_dir,
            dream_dir,
            limit,
            json,
        } => {
            let (section, out) = match command {
                None => (BrainSection::Summary, None),
                Some(BrainCommand::Memory) => (BrainSection::Memory, None),
                Some(BrainCommand::Skills) => (BrainSection::Skills, None),
                Some(BrainCommand::Policies) => (BrainSection::Policies, None),
                Some(BrainCommand::Findings) => (BrainSection::Findings, None),
                Some(BrainCommand::Candidates) => (BrainSection::Candidates, None),
                Some(BrainCommand::View { id }) => (BrainSection::View(id), None),
                Some(BrainCommand::Report { out }) => (BrainSection::Report, out),
            };
            show_brain(BrainOptions {
                memory_dir: option_or_config(memory_dir, config.memory_dir()),
                dream_dir: option_or_config(dream_dir, config.dream_dir()),
                section,
                limit,
                out,
                json,
            })
        }
        Command::Eval { command } => match command {
            EvalCommand::Manifest { out, include } => {
                write_eval_manifest(EvalManifestOptions { out, include })
            }
            EvalCommand::Check { manifest } => {
                check_eval_manifest_command(EvalCheckOptions { manifest })
            }
        },
        Command::Self_ { command } => match command {
            SelfCommand::Prepare {
                finding,
                candidates,
                out_dir,
            } => self_prepare(SelfPrepareOptions {
                finding,
                candidates,
                out_dir,
            }),
            SelfCommand::Capture {
                finding,
                candidate,
                out_dir,
            } => self_capture(SelfCaptureOptions {
                finding,
                candidate,
                out_dir,
            }),
            SelfCommand::Fix {
                finding,
                candidates,
                skill,
                task,
                from,
                out_dir,
                model_config,
                regression_file,
                evaluate,
                keep_worktrees,
                allow_dirty_bootstrap,
            } => self_fix(SelfFixOptions {
                finding,
                candidates,
                skill,
                task,
                from,
                out_dir,
                model_config: option_or_config(model_config, config.model_config()),
                regression_file,
                evaluate,
                keep_worktrees,
                allow_dirty_bootstrap,
            }),
            SelfCommand::Compare { finding, from, out } => self_compare(SelfCompareOptions {
                finding,
                from,
                out,
                model_config: config.model_config(),
            }),
        },
        Command::Dev { command } => run_dev_command(command, &config),
        Command::Project { command } => match command {
            ProjectCommand::Plan {
                goal,
                output,
                model_config,
                planner_model,
                template,
            } => project_plan(ProjectPlanOptions {
                goal,
                output,
                model_config: option_or_config(model_config, config.model_config()),
                planner_model,
                template,
            }),
            ProjectCommand::Run { file, task, log } => {
                project_run(ProjectRunOptions { file, task, log })
            }
            ProjectCommand::Status { file } => project_status(ProjectStatusOptions { file }),
            ProjectCommand::Verify { file, task } => {
                project_verify(ProjectVerifyOptions { file, task })
            }
        },
        Command::Run {
            target,
            mode,
            top_k,
            skills,
            project_file,
            plan_only,
            execute,
            planner_model,
            model_config,
            trace_out,
            trace,
            log,
            timeout_seconds,
            explain,
            tool_config,
            artifact_out,
            replay_artifact,
            replay_from,
            verification_command,
            no_memory,
            memory_dir,
            memory_limit,
        } => {
            let memory_enabled = !no_memory;
            let memory_dir = option_or_config(memory_dir, config.memory_dir());
            let memory_hint = build_memory_run_hint(MemoryHintOptions {
                memory_dir: memory_dir.clone(),
                task: target.clone(),
                limit: Some(memory_limit),
            })?;
            let memory_pack = if memory_enabled {
                Some(build_memory_pack(MemoryPackOptions {
                    memory_dir,
                    task: target.clone(),
                    limit: Some(memory_limit),
                    include_evidence: false,
                    record_usage: true,
                })?)
            } else {
                None
            };
            let memory_context = memory_pack
                .as_ref()
                .map(|pack| pack.context.trim().to_string())
                .filter(|context| !context.is_empty());
            let output = run_entry_task(EntryTaskOptions {
                task: target,
                mode,
                top_k,
                skills,
                model_config: option_or_config(model_config, config.model_config()),
                trace_out: trace.trace_out(trace_out),
                trace_redact: trace.redact(),
                trace_raw: trace.raw(),
                log,
                explain,
                tool_config,
                artifact_out,
                replay_artifact,
                replay_from,
                verification_command,
                project_file,
                plan_only,
                execute,
                planner_model,
                memory_context,
                memory_pack,
                timeout_seconds,
            })?;
            let mut output = output;
            attach_memory_hint(&mut output, memory_hint)?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            Ok(())
        }
    }
}

fn run_dream_findings_command(command: DreamFindingsCommand) -> Result<()> {
    match command {
        DreamFindingsCommand::List { status, limit } => {
            list_dream_findings(DreamFindingsListOptions { status, limit })
        }
        DreamFindingsCommand::Open { finding, reason } => {
            open_dream_finding(DreamFindingUpdateOptions {
                finding,
                reason,
                fixed_by: None,
            })
        }
        DreamFindingsCommand::Resolve {
            finding,
            reason,
            fixed_by,
        } => resolve_dream_finding(DreamFindingUpdateOptions {
            finding,
            reason,
            fixed_by,
        }),
        DreamFindingsCommand::Dismiss { finding, reason } => {
            dismiss_dream_finding(DreamFindingUpdateOptions {
                finding,
                reason,
                fixed_by: None,
            })
        }
    }
}

fn run_current_air_with_timeout(
    raw_args: &[OsString],
    task: &str,
    artifact_out: Option<&Path>,
    timeout_seconds: u64,
) -> Result<()> {
    let args = strip_timeout_seconds(raw_args);
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let result = run_current_air_passthrough(&cwd, &args, Duration::from_secs(timeout_seconds))?;
    if result.success {
        return Ok(());
    }
    if result.status == "timeout" || result.status == "timeout_before_start" {
        let artifact_dir = artifact_out
            .map(|path| {
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(path)
                }
            })
            .unwrap_or_else(default_timeout_artifact_dir);
        write_timeout_code_run_artifact(&artifact_dir, task, timeout_seconds)?;
        anyhow::bail!("air run exceeded wall-clock timeout of {timeout_seconds} seconds");
    }
    match result.code {
        Some(code) => std::process::exit(code),
        None => anyhow::bail!("timed air run failed: {}", result.status),
    }
}

fn default_timeout_artifact_dir() -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    PathBuf::from("target")
        .join("generated")
        .join("code-runs")
        .join(format!("timeout-run-{pid}-{nanos}"))
}

fn strip_timeout_seconds(raw_args: &[OsString]) -> Vec<OsString> {
    if raw_args.get(1).is_none_or(|command| command != "run") {
        return raw_args.iter().skip(1).cloned().collect();
    }

    let mut out = Vec::new();
    let mut index = 1usize;
    let mut stripped_timeout = false;
    while index < raw_args.len() {
        let arg = &raw_args[index];

        if !stripped_timeout && arg == "--timeout-seconds" {
            stripped_timeout = true;
            index += if index + 1 < raw_args.len() { 2 } else { 1 };
            continue;
        }
        if !stripped_timeout
            && arg
                .to_str()
                .is_some_and(|value| value.starts_with("--timeout-seconds="))
        {
            stripped_timeout = true;
            index += 1;
            continue;
        }
        out.push(arg.clone());
        index += 1;
    }
    out
}

fn show_status(json: bool, config: &AirCliConfig) -> Result<()> {
    let status = build_status_value(config)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    println!("AIR status");
    if let Some(last_completed_at) = status
        .pointer("/dream/last_completed_at_unix")
        .and_then(Value::as_u64)
    {
        println!("- Dream: last completed at unix {last_completed_at}");
    } else {
        println!("- Dream: no completed run recorded");
    }
    println!(
        "- Findings: {} open",
        status
            .pointer("/findings/open_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
    if let Some(items) = status
        .pointer("/findings/top_open")
        .and_then(Value::as_array)
    {
        for item in items.iter().take(3) {
            let id = item.get("id").and_then(Value::as_str).unwrap_or("unknown");
            let category = item
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let impact = item
                .get("impact_score")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            println!("  - {id}: {category} (impact {impact:.1})");
        }
    }
    println!(
        "- Memory: {} total cards",
        status
            .pointer("/memory/total")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
    if let Some(by_status) = status
        .pointer("/memory/by_status")
        .and_then(Value::as_object)
    {
        let mut parts: Vec<String> = by_status
            .iter()
            .map(|(status, count)| format!("{status}={}", count.as_u64().unwrap_or(0)))
            .collect();
        parts.sort();
        if !parts.is_empty() {
            println!("  - {}", parts.join(", "));
        }
    }
    println!(
        "- Candidates: {} dream experiment candidates",
        status
            .pointer("/candidates/count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
    Ok(())
}

fn build_status_value(config: &AirCliConfig) -> Result<Value> {
    let dream_dir = config
        .dream_dir()
        .unwrap_or_else(|| PathBuf::from(".air/dream"));
    let memory_dir = config
        .memory_dir()
        .unwrap_or_else(|| PathBuf::from(".air/memory"));
    let dream_state_path = dream_dir.join("state.json");
    let dream_state = read_json_if_exists(&dream_state_path)?;
    let last_completed_at_unix = dream_state
        .as_ref()
        .and_then(|value| value.get("last_completed_at_unix"))
        .and_then(Value::as_u64);

    let findings = read_latest_status_findings(&dream_dir.join("findings.jsonl"))?;
    let open_count = findings
        .iter()
        .filter(|finding| {
            finding
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("open")
                == "open"
        })
        .count();
    let mut top_open: Vec<Value> = findings
        .iter()
        .filter(|finding| {
            finding
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("open")
                == "open"
        })
        .cloned()
        .collect();
    top_open.sort_by(|left, right| {
        status_number(right, "impact_score")
            .partial_cmp(&status_number(left, "impact_score"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    top_open.truncate(3);

    let memory_summary = summarize_memory_cards(&memory_dir.join("cards"))?;
    let candidate_count = count_candidate_eval_files(&dream_dir.join("latest").join("candidates"))?;

    Ok(serde_json::json!({
        "schema": "air.status.v1",
        "dream": {
            "state_path": dream_state_path,
            "last_completed_at_unix": last_completed_at_unix,
        },
        "findings": {
            "path": dream_dir.join("findings.jsonl"),
            "total": findings.len(),
            "open_count": open_count,
            "top_open": top_open,
        },
        "memory": memory_summary,
        "candidates": {
            "path": dream_dir.join("latest").join("candidates"),
            "count": candidate_count,
        },
    }))
}

fn read_json_if_exists(path: &Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

fn read_latest_status_findings(path: &Path) -> Result<Vec<Value>> {
    use std::collections::BTreeMap;

    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)?;
    let mut latest = BTreeMap::<String, Value>::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)?;
        let key = value
            .get("stable_key")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        latest.insert(key, value);
    }
    Ok(latest.into_values().collect())
}

fn summarize_memory_cards(cards_dir: &Path) -> Result<Value> {
    use std::collections::BTreeMap;

    let mut by_status = BTreeMap::<String, usize>::new();
    let mut by_kind = BTreeMap::<String, usize>::new();
    let mut total = 0usize;
    for path in json_files_under(cards_dir)? {
        let Some(value) = read_json_if_exists(&path)? else {
            continue;
        };
        total += 1;
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        *by_status.entry(status).or_default() += 1;
        *by_kind.entry(kind).or_default() += 1;
    }
    Ok(serde_json::json!({
        "path": cards_dir,
        "total": total,
        "by_status": by_status,
        "by_kind": by_kind,
    }))
}

fn count_candidate_eval_files(path: &Path) -> Result<usize> {
    Ok(json_files_under(path)?
        .into_iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("eval.json"))
        .count())
}

fn json_files_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_json_files(root, &mut out)?;
    Ok(out)
}

fn collect_json_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if root.extension().and_then(|ext| ext.to_str()) == Some("json") {
            out.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        collect_json_files(&entry.path(), out)?;
    }
    Ok(())
}

fn status_number(value: &Value, field: &str) -> f64 {
    value.get(field).and_then(Value::as_f64).unwrap_or_default()
}

fn attach_memory_hint(output: &mut Value, hint: air_memory::MemoryRunHint) -> Result<()> {
    if let Some(memory) = output.get_mut("memory").and_then(Value::as_object_mut) {
        memory.insert("hint".to_string(), serde_json::to_value(hint)?);
    }
    Ok(())
}

fn improve_action(command: Option<ImproveCommand>) -> ImproveAction {
    match command {
        None => ImproveAction::Report,
        Some(ImproveCommand::Next) => ImproveAction::Next,
        Some(ImproveCommand::Check { finding }) => ImproveAction::Check { finding },
        Some(ImproveCommand::Promote { finding }) => ImproveAction::Promote { finding },
        Some(ImproveCommand::Fix { finding }) => ImproveAction::Fix { finding },
        Some(ImproveCommand::Evaluate {
            finding,
            regression_file,
            report,
        }) => ImproveAction::Evaluate {
            finding,
            report,
            regression_file,
        },
    }
}

fn run_dev_command(command: DevCommand, config: &AirCliConfig) -> Result<()> {
    match command {
        DevCommand::ValidateModule { file } => validate(file),
        DevCommand::ValidateSystem { file } => validate_system(file),
        DevCommand::ValidatePlan {
            plan,
            profile,
            store,
            explain,
        } => validate_plan(ValidatePlanOptions {
            plan,
            profile,
            store,
            explain,
        }),
        DevCommand::MakePlan {
            task,
            task_file,
            store,
            model_config,
            planner_model,
            allow_internal,
            explain,
            output,
        } => plan_task(PlanOptions {
            task,
            task_file,
            store,
            model_config: option_or_config(model_config, config.model_config()),
            planner_model,
            allow_internal,
            explain,
            output,
        }),
        DevCommand::RunModule {
            file,
            input,
            model_config,
            trace_out,
            trace,
            log,
            example_tools,
            tool_config,
        } => run(
            file,
            input,
            option_or_config(model_config, config.model_config()),
            trace.trace_out(trace_out),
            trace.redact(),
            log,
            example_tools,
            tool_config,
        ),
        DevCommand::RunSystem {
            file,
            input,
            model_config,
            trace_out,
            trace,
            log,
            example_tools,
            tool_config,
        } => run_system(
            file,
            input,
            option_or_config(model_config, config.model_config()),
            trace.trace_out(trace_out),
            trace.redact(),
            log,
            example_tools,
            tool_config,
        ),
        DevCommand::RunPlan {
            plan,
            profile,
            store,
            input,
            model_config,
            trace_out,
            trace,
            state_out,
            checkpoint_out,
            jit_cache,
            parallel,
            log,
            example_tools,
            tool_config,
        } => run_plan(RunPlanOptions {
            plan,
            profile,
            store,
            input,
            input_values: None,
            model_config: option_or_config(model_config, config.model_config()),
            trace_out: trace.trace_out(trace_out),
            trace_redact: trace.redact(),
            trace_raw: trace.raw(),
            state_out,
            checkpoint_out,
            jit_cache,
            parallel,
            log,
            example_tools,
            tool_config,
            model_replay: None,
        }),
        DevCommand::ResumePlan {
            plan,
            profile,
            store,
            input,
            state,
            overrides,
            model_config,
            trace_out,
            trace,
            state_out,
            checkpoint_out,
            log,
            example_tools,
            tool_config,
        } => resume_plan(ResumePlanOptions {
            plan,
            profile,
            store,
            input,
            state,
            overrides,
            model_config: option_or_config(model_config, config.model_config()),
            trace_out: trace.trace_out(trace_out),
            trace_redact: trace.redact(),
            trace_raw: trace.raw(),
            state_out,
            checkpoint_out,
            log,
            example_tools,
            tool_config,
        }),
        DevCommand::Replay {
            trace,
            specialize_run_plan,
            store,
            output,
            identity_out,
            stats,
        } => replay(ReplayOptions {
            trace,
            specialize_run_plan,
            store,
            output,
            identity_out,
            stats,
        }),
    }
}

fn validate_system(file: PathBuf) -> Result<()> {
    let system = air_linker::parse_system_file(&file)?;
    let base_dir = module_base_dir_for_modules(&system.modules)?;
    let report = air_linker::validate_system(&system, base_dir);

    if report.diagnostics.is_empty() {
        println!("ok: {} {}", system.system.name, system.system.version);
        return Ok(());
    }

    emit_diagnostics(&report.diagnostics);

    if report.is_success() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn validate_plan(options: ValidatePlanOptions) -> Result<()> {
    let ValidatePlanOptions {
        plan,
        profile,
        store,
        explain,
    } = options;

    let profile = match profile {
        Some(path) => Some((path.clone(), read_run_plan_profile(&path)?)),
        None => None,
    };
    let plan = plan
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.plan))
        })
        .ok_or_else(|| anyhow::anyhow!("validate-plan requires a plan path or --profile"))?;
    let store = store
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.store))
        })
        .ok_or_else(|| anyhow::anyhow!("validate-plan requires --store or --profile"))?;

    let plan = air_linker::parse_run_plan_file(plan)?;
    let store_path = store;
    let store = air_linker::parse_module_store_file(&store_path)?;
    let base_dir = module_base_dir_for_store_path(&store, &store_path);
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);

    emit_diagnostics(&report.diagnostics);

    if report.is_success() {
        if explain {
            let explanation = build_plan_explanation(&plan, &store, &base_dir)?;
            print!("{}", format_plan_explanation(&explanation));
        } else if report.diagnostics.is_empty() {
            println!("ok: {} {}", plan.plan.name, plan.plan.version);
        }
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn validate(file: PathBuf) -> Result<()> {
    let module = air_parser::parse_air_file(&file)?;
    let report = air_verify::verify(&module);

    if report.diagnostics.is_empty() {
        println!("ok: {} {}", module.agent.name, module.agent.version);
        return Ok(());
    }

    emit_diagnostics(&report.diagnostics);

    if report.is_success() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    file: PathBuf,
    input: PathBuf,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    log: bool,
    example_tools: bool,
    tool_config: Option<PathBuf>,
) -> Result<()> {
    let module = air_parser::parse_air_file(&file)?;
    let report = air_verify::verify(&module);
    if !report.is_success() {
        emit_diagnostics(&report.diagnostics);
        std::process::exit(1);
    }

    let input = fs::read_to_string(input)?;
    let Value::Object(inputs) = serde_json::from_str::<Value>(&input)? else {
        anyhow::bail!("--input must be a JSON object");
    };

    let observe = log || trace_out.is_some();
    let mut observed_trace = Vec::new();
    let tools = ToolProviderChoice::from_config(tool_config, example_tools)?;
    let result = if let Some(model_config) = model_config {
        let models = ModelProviderChoice::from_config_file_with_provider_io(model_config, log)?;
        let mut vm = Vm { tools, models };
        if observe {
            let result = vm.run_with_observer(&module, inputs, |event| {
                observe_event_with_trace_file(
                    event,
                    log,
                    &mut observed_trace,
                    trace_out.as_ref(),
                    trace_redact,
                )
            });
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            vm.run(&module, inputs)?
        }
    } else {
        let mut vm = Vm {
            tools,
            models: ModelProviderChoice::echo(),
        };
        if observe {
            let result = vm.run_with_observer(&module, inputs, |event| {
                observe_event_with_trace_file(
                    event,
                    log,
                    &mut observed_trace,
                    trace_out.as_ref(),
                    trace_redact,
                )
            });
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            vm.run(&module, inputs)?
        }
    };
    if let Some(trace_out) = trace_out {
        let mut trace = result.trace.clone();
        trace.push(system_return_event(result.outputs.clone()));
        write_trace(trace_out, &trace, trace_redact)?;
    }
    println!("{}", serde_json::to_string_pretty(&result.outputs)?);

    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn run_system(
    file: PathBuf,
    input: PathBuf,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    log: bool,
    example_tools: bool,
    tool_config: Option<PathBuf>,
) -> Result<()> {
    let system = air_linker::parse_system_file(&file)?;
    let base_dir = module_base_dir_for_modules(&system.modules)?;
    let report = air_linker::validate_system(&system, &base_dir);
    if !report.is_success() {
        emit_diagnostics(&report.diagnostics);
        std::process::exit(1);
    }

    let input = fs::read_to_string(input)?;
    let Value::Object(inputs) = serde_json::from_str::<Value>(&input)? else {
        anyhow::bail!("--input must be a JSON object");
    };

    let observe = log || trace_out.is_some();
    let mut observed_trace = Vec::new();
    let tools = ToolProviderChoice::from_config(tool_config, example_tools)?;
    let result = if let Some(model_config) = model_config {
        let models = ModelProviderChoice::from_config_file_with_provider_io(model_config, log)?;
        if observe {
            let result = air_linker::run_system_with_observer(
                &system,
                base_dir,
                inputs,
                tools,
                models,
                |event| {
                    observe_event_with_trace_file(
                        event,
                        log,
                        &mut observed_trace,
                        trace_out.as_ref(),
                        trace_redact,
                    )
                },
            );
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            air_linker::run_system(&system, base_dir, inputs, tools, models)?
        }
    } else if observe {
        let result = air_linker::run_system_with_observer(
            &system,
            base_dir,
            inputs,
            tools,
            ModelProviderChoice::echo(),
            |event| {
                observe_event_with_trace_file(
                    event,
                    log,
                    &mut observed_trace,
                    trace_out.as_ref(),
                    trace_redact,
                )
            },
        );
        if result.is_err() {
            write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
        }
        result?
    } else {
        air_linker::run_system(
            &system,
            base_dir,
            inputs,
            tools,
            ModelProviderChoice::echo(),
        )?
    };
    if let Some(trace_out) = trace_out {
        let mut trace = result.trace.clone();
        trace.push(system_return_event(result.outputs.clone()));
        write_trace(trace_out, &trace, trace_redact)?;
    }
    println!("{}", serde_json::to_string_pretty(&result.outputs)?);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{module_catalog, parse_planner_response, planner_request, recipe_catalog};
    use crate::profile::read_json_object;
    use crate::run_plan::{
        run_plan_with_inputs, run_plan_with_inputs_capture, write_checkpoint_state, PlanStateFile,
        RunPlanExecutionOptions,
    };
    use crate::tools::{provider_error_snippet, search_docs, ConfigTools, LocalDoc};
    use air_runtime::{read_trace_jsonl, TraceStatus};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn materializes_recipe_selection_from_planner_response() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();

        let plan = parse_planner_response(
            json!({
                "recipe_id": "deep_research.four_topic_report@0.1.0",
                "rationale": "matches the four-topic deep research request"
            }),
            &store,
            &root,
            true,
        )
        .unwrap();

        assert_eq!(plan.nodes.len(), 6);
        assert_eq!(plan.outputs["result"], "final.final_report");
        assert_eq!(
            plan.decisions[0].selected,
            vec!["deep_research.four_topic_report@0.1.0"]
        );
    }

    #[test]
    fn rejects_internal_recipe_selection_without_allow_internal() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();
        store.recipes[0].visibility = air_linker::ModuleVisibility::Internal;

        let error = parse_planner_response(
            json!({"recipe_id": "deep_research.four_topic_report@0.1.0"}),
            &store,
            &root,
            false,
        )
        .unwrap_err();

        assert!(error.to_string().contains("without --allow-internal"));
    }

    #[test]
    fn parses_recipe_selection_from_content_wrapper() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();

        let plan = parse_planner_response(
            json!({
                "content": "```json\n{\"recipe_id\":\"deep_research.four_topic_report@0.1.0\"}\n```"
            }),
            &store,
            &root,
            true,
        )
        .unwrap();

        assert_eq!(plan.entry, "plan");
    }

    #[test]
    fn planner_request_teaches_dynamic_fanout_ir() {
        let store = air_linker::ModuleStore {
            store: air_linker::StoreMetadata {
                name: "test-store".to_string(),
                version: "0.1.0".to_string(),
            },
            imports: vec![],
            modules: Default::default(),
            recipes: vec![],
        };

        let request = planner_request("fan out over runtime topics", &store, vec![], vec![], true);
        let instructions = request["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .unwrap()
            .join("\n");

        assert!(instructions.contains("Use dynamic.fanouts"));
        assert!(instructions.contains("$item"));
        assert!(instructions.contains("$each.<input>"));
        assert!(instructions.contains("Do not pre-list dynamic fan-out instances"));
        assert!(instructions.contains("$parent.<array_output>"));
        assert_eq!(
            request["example_dynamic_fanout_shape"]["dynamic"]["fanouts"][0]["source"],
            json!("plan.plan.topics")
        );
        assert_eq!(
            request["example_dynamic_fanout_shape"]["dynamic"]["fanouts"][0]["input"][1]["from"],
            json!("$item")
        );
        assert_eq!(
            request["example_nested_dynamic_fanout_shape"]["dynamic"]["fanouts"][1]["after"],
            json!("parent_wave")
        );
        assert_eq!(
            request["example_nested_dynamic_fanout_shape"]["dynamic"]["fanouts"][1]["source"],
            json!("$parent.followups")
        );
    }

    #[test]
    fn planner_request_ranks_large_components_before_small_building_blocks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Build a four topic deep research report with web research and synthesis.",
            &store,
            catalog,
            recipes,
            true,
        );

        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["id"],
            json!("deep_research.four_topic_report@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["source"],
            json!("recipe")
        );
        let recommended = request["module_store"]["component_selection"]["recommended"]
            .as_array()
            .unwrap();
        let topic_position = recommended
            .iter()
            .position(|candidate| candidate["id"] == json!("deep_research.research_topic@0.1.0"))
            .unwrap();
        let final_position = recommended
            .iter()
            .position(|candidate| candidate["id"] == json!("deep_research.final_report@0.1.0"))
            .unwrap();
        assert!(final_position < topic_position);

        let instructions = request["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(instructions.contains("component_selection.first_choice"));
        assert!(instructions.contains("fallback-to-primitives"));
    }

    #[test]
    fn planner_request_ranks_public_composite_over_internal_primitives() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("tests/plans/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Triage customer billing feedback, extract the issue, score risk, and generate a report.",
            &store,
            catalog,
            recipes,
            true,
        );

        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["id"],
            json!("customer.triage@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["tier"],
            json!("large_component")
        );
    }

    #[test]
    fn planner_request_does_not_force_code_edit_loop_for_open_ended_questions() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("skills/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Explore how command_run is implemented and identify relevant repository files.",
            &store,
            catalog,
            recipes,
            true,
        );

        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["id"],
            json!("context.compact@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["tier"],
            json!("public_building_block")
        );
    }

    #[test]
    fn planner_request_ranks_code_edit_loop_for_plan_act_observe_tasks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("skills/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Explore the repository with a dynamic plan-act-observe loop that lets the model choose declared read-only tools.",
            &store,
            catalog,
            recipes,
            true,
        );

        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["id"],
            json!("code.edit@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["source"],
            json!("module")
        );
    }

    #[test]
    fn planner_request_routes_code_agent_task_shapes_to_matching_components() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("skills/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();

        for (task, expected_id, expected_source) in [
            (
                "Review the command_run implementation for safety, provenance, and diagnostics.",
                "code.edit@0.1.0",
                "module",
            ),
            (
                "Edit a failing test using structured diagnostics, apply a bounded patch, and retest.",
                "code.edit@0.1.0",
                "module",
            ),
            (
                "Explore how command_run is implemented and identify relevant repository files.",
                "context.compact@0.1.0",
                "module",
            ),
        ] {
            let catalog = module_catalog(&store, &root, true).unwrap();
            let recipes = recipe_catalog(&store, &root, true).unwrap();
            let request = planner_request(task, &store, catalog, recipes, true);
            let first_choice = &request["module_store"]["component_selection"]["first_choice"];

            assert_eq!(
                first_choice["id"],
                json!(expected_id),
                "task should route to {expected_id}: {task}"
            );
            assert_eq!(
                first_choice["source"],
                json!(expected_source),
                "task should route through {expected_source}: {task}"
            );
        }
    }

    #[test]
    fn planner_request_hides_unmatched_code_agent_components_from_recommendations() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("skills/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Build an Apple-style landing page as a single HTML file and verify screenshots.",
            &store,
            catalog,
            recipes,
            true,
        );
        let recommended = request["module_store"]["component_selection"]["recommended"]
            .as_array()
            .unwrap();

        assert!(recommended
            .iter()
            .all(|candidate| candidate["matched_terms"]
                .as_array()
                .is_some_and(|terms| !terms.is_empty())));
        assert!(!recommended
            .iter()
            .any(|candidate| candidate["id"] == json!("context.compact@0.1.0")));
    }

    #[test]
    fn local_docs_search_dedupes_raw_content_before_limit() {
        let documents = vec![
            LocalDoc {
                id: "a".to_string(),
                title: "EV readiness A".to_string(),
                content: "Duplicate EV readiness evidence".to_string(),
            },
            LocalDoc {
                id: "b".to_string(),
                title: "EV readiness B".to_string(),
                content: "Duplicate EV readiness evidence".to_string(),
            },
            LocalDoc {
                id: "c".to_string(),
                title: "EV readiness C".to_string(),
                content: "Distinct EV readiness evidence".to_string(),
            },
        ];

        let result = search_docs(&json!({"query": "EV readiness"}), &documents, 2);

        assert_eq!(result["documents"].as_array().unwrap().len(), 2);
        assert_eq!(result["documents"][0]["id"], json!("a"));
        assert_eq!(result["documents"][1]["id"], json!("c"));
    }

    #[test]
    fn read_json_object_reports_label_and_path() {
        let path = temp_file_path("air-input-diagnostic", "json");
        fs::write(&path, "[]").unwrap();

        let error = read_json_object(path.clone(), "--input").unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("--input JSON file"));
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("must contain a JSON object"));
    }

    #[test]
    fn tool_config_reports_invalid_http_json_url_field() {
        let path = temp_file_path("air-tool-diagnostic", "json");
        fs::write(
            &path,
            r#"{
              "tools": {
                "web.search": {
                  "kind": "http_json",
                  "capability": "search",
                  "url": "localhost/search"
                }
              }
            }"#,
        )
        .unwrap();

        let error = ConfigTools::from_file(path.clone()).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("tool config"));
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("tools.web.search.url"));
        assert!(message.contains("http:// or https://"));
    }

    #[test]
    fn provider_error_snippet_redacts_http_tool_body() {
        let body = format!(
            "token=tool-secret Authorization: Bearer header-secret {}",
            "x".repeat(4096)
        );
        let message = provider_error_snippet(&body);

        assert!(!message.contains("tool-secret"));
        assert!(!message.contains("header-secret"));
        assert!(message.contains("[AIR_REDACTED]"));
        assert!(message.contains("[AIR_TRUNCATED]"));
        assert!(message.len() < body.len());
    }

    #[test]
    fn tool_config_reports_invalid_local_docs_limit_field() {
        let path = temp_file_path("air-doc-tool-diagnostic", "json");
        fs::write(
            &path,
            r#"{
              "tools": {
                "docs.search": {
                  "kind": "local_docs_search",
                  "capability": "retrieval.local",
                  "documents": [
                    {"id": "doc-1", "title": "Doc", "content": "Content"}
                  ],
                  "max_results": 0
                }
              }
            }"#,
        )
        .unwrap();

        let error = ConfigTools::from_file(path.clone()).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("tools.docs.search.max_results"));
        assert!(message.contains("greater than 0"));
    }

    fn temp_file_path(prefix: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }

    fn temp_repo_file_path(root: &std::path::Path, prefix: &str, extension: &str) -> PathBuf {
        let dir = root.join("target/generated/test-tmp");
        fs::create_dir_all(&dir).unwrap();
        dir.join(format!(
            "{prefix}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }

    fn temp_dir_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let source = entry.path();
            let target = to.join(entry.file_name());
            if source.is_dir() {
                copy_dir_recursive(&source, &target);
            } else {
                fs::copy(&source, &target).unwrap();
            }
        }
    }

    #[test]
    fn code_agent_fixture_runs_through_workspace_tests() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture_root = temp_dir_path("air-code-agent-fixture");
        let trace_path = temp_repo_file_path(&root, "code-agent-minimal", "jsonl");
        let _ = fs::remove_dir_all(&fixture_root);
        fs::create_dir_all(fixture_root.join("examples")).unwrap();
        fs::create_dir_all(fixture_root.join("modules")).unwrap();
        copy_dir_recursive(
            &root.join("skills/code-agent"),
            &fixture_root.join("skills/code-agent"),
        );
        copy_dir_recursive(&root.join("modules/std"), &fixture_root.join("modules/std"));
        let fixture_tool_config = fixture_root.join("skills/code-agent/tools.json");
        let mut tool_config_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture_tool_config).unwrap()).unwrap();
        tool_config_json["workspace_dir"] =
            serde_json::Value::String(fixture_root.display().to_string());
        fs::write(
            &fixture_tool_config,
            serde_json::to_vec_pretty(&tool_config_json).unwrap(),
        )
        .unwrap();

        let git_init = std::process::Command::new("git")
            .arg("-C")
            .arg(&fixture_root)
            .arg("init")
            .arg("-q")
            .status()
            .unwrap();
        assert!(git_init.success());
        let git_add = std::process::Command::new("git")
            .arg("-C")
            .arg(&fixture_root)
            .arg("add")
            .arg("examples")
            .arg("modules")
            .arg("skills")
            .status()
            .unwrap();
        assert!(git_add.success());
        let git_commit = std::process::Command::new("git")
            .arg("-C")
            .arg(&fixture_root)
            .args(["-c", "user.email=air@example.test"])
            .args(["-c", "user.name=AIR Test"])
            .arg("commit")
            .arg("-q")
            .arg("-m")
            .arg("baseline")
            .status()
            .unwrap();
        assert!(git_commit.success());

        let result = run_plan_with_inputs_capture(
            fixture_root.join("skills/code-agent/code-edit.air-plan.yaml"),
            fixture_root.join("skills/code-agent/module-store.air-store.yaml"),
            serde_json::Map::from_iter([(
                "task".to_string(),
                json!("fix the failing add function and retest"),
            )]),
            RunPlanExecutionOptions {
                model_config: Some(
                    fixture_root.join("skills/code-agent/fixtures/model-fixtures.json"),
                ),
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: Some(fixture_tool_config),
                model_replay: None,
            },
        )
        .unwrap();

        let edit = &result["edit"];
        assert_eq!(edit["final_success"], json!(true));
        assert_eq!(edit["patch_applied"], json!(true));
        assert!(edit["workspace_diff"]["diff"]
            .as_str()
            .unwrap()
            .contains("skills/code-agent/edit-fixture/math.js"));

        let trace = read_trace_jsonl(&trace_path).unwrap();
        let first_decider = trace
            .iter()
            .find(|event| {
                event.action == "model_call_start"
                    && event.meta.as_ref().unwrap()["model"] == json!("code_edit_decider")
            })
            .expect("code_edit_decider model call");
        assert_eq!(
            first_decider.input.as_ref().unwrap()["allowed_tools"],
            json!([
                "bash",
                "grep",
                "glob",
                "read_contains",
                "read_range",
                "edit",
                "todowrite",
                "todoread"
            ])
        );

        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_dir_all(fixture_root);
    }

    #[test]
    fn writes_resume_compatible_checkpoint_state() {
        let path = std::env::temp_dir().join(format!(
            "air-checkpoint-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let checkpoint = air_linker::SystemCheckpoint {
            completed_module: "plan".to_string(),
            module_outputs: BTreeMap::from([(
                "plan".to_string(),
                serde_json::Map::from_iter([("plan".to_string(), json!({"topic_1": "EV"}))]),
            )]),
        };

        write_checkpoint_state(Some(&path), &checkpoint).unwrap();

        let state: PlanStateFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let _ = fs::remove_file(path);

        assert_eq!(
            state.status,
            air_linker::RunStatus::InProgress {
                after: "plan".to_string()
            }
        );
        assert_eq!(state.outputs, json!({}));
        assert_eq!(state.module_outputs["plan"]["plan"]["topic_1"], json!("EV"));
    }

    #[test]
    fn reads_run_plan_profile_with_relative_paths() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let profile_path = root.join("examples/deep-research/profile.air-profile.yaml");
        let profile = read_run_plan_profile(&profile_path).unwrap();

        assert_eq!(
            profile.plan,
            PathBuf::from("deep-research-clarified.air-plan.yaml")
        );
        assert_eq!(
            resolve_profile_path(&profile_path, &profile.plan),
            root.join("examples/deep-research/deep-research-clarified.air-plan.yaml")
        );
        assert_eq!(
            resolve_profile_path(&profile_path, &profile.store),
            root.join("examples/deep-research/module-store.air-store.yaml")
        );
        assert_eq!(
            resolve_profile_path(&profile_path, profile.input_file.as_ref().unwrap()),
            root.join("examples/deep-research/input.json")
        );
    }

    #[test]
    fn default_help_exposes_only_primary_commands() {
        let mut output = Vec::new();
        <Cli as clap::CommandFactory>::command()
            .write_help(&mut output)
            .unwrap();
        let help = String::from_utf8(output).unwrap();
        let command_names = help
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter(|line| !line.starts_with("-"))
            .filter_map(|line| line.split_whitespace().next())
            .collect::<Vec<_>>();

        for command in [
            "run",
            "status",
            "audit",
            "brain",
            "skill",
            "dream",
            "findings",
            "memory",
            "mcp",
            "bench",
            "improve",
            "regression",
            "eval",
            "self",
            "dev",
            "project",
        ] {
            assert!(
                command_names.contains(&command),
                "expected {command} in help"
            );
        }

        for command in [
            "validate-plan",
            "plan",
            "run-plan",
            "resume-plan",
            "validate-system",
            "run-system",
            "validate",
            "code",
            "lower",
            "lower-plan",
            "replay",
            "deep-research",
        ] {
            assert!(
                !command_names.contains(&command),
                "did not expect {command} in default help"
            );
        }
    }

    #[test]
    fn dev_help_exposes_internal_commands() {
        let mut output = Vec::new();
        let mut command = <Cli as clap::CommandFactory>::command();
        let dev = command.find_subcommand_mut("dev").unwrap();
        dev.write_help(&mut output).unwrap();
        let help = String::from_utf8(output).unwrap();
        let command_names = help
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter(|line| !line.starts_with("-"))
            .filter_map(|line| line.split_whitespace().next())
            .collect::<Vec<_>>();

        for command in [
            "validate-module",
            "validate-system",
            "validate-plan",
            "make-plan",
            "run-module",
            "run-system",
            "run-plan",
            "resume-plan",
            "replay",
        ] {
            assert!(
                command_names.contains(&command),
                "expected {command} in dev help"
            );
        }
    }

    #[test]
    fn dev_run_plan_accepts_profile() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "run-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
            "--log",
        ])
        .unwrap();

        let Command::Dev {
            command: DevCommand::RunPlan {
                plan, profile, log, ..
            },
        } = cli.command
        else {
            panic!("expected dev run-plan command");
        };

        assert_eq!(plan, None);
        assert_eq!(
            profile,
            Some(PathBuf::from(
                "examples/deep-research/profile.air-profile.yaml"
            ))
        );
        assert!(log);
    }

    #[test]
    fn run_rejects_zero_top_k() {
        let err = Cli::try_parse_from(["air", "run", "fix bug", "--top-k", "0"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn skill_help_exposes_lifecycle_not_execution_debug_commands() {
        let mut output = Vec::new();
        let mut command = <Cli as clap::CommandFactory>::command();
        let skill = command.find_subcommand_mut("skill").unwrap();
        skill.write_help(&mut output).unwrap();
        let help = String::from_utf8(output).unwrap();
        let command_names = help
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter(|line| !line.starts_with("-"))
            .filter_map(|line| line.split_whitespace().next())
            .collect::<Vec<_>>();

        for command in [
            "list", "validate", "audit", "import", "upgrade", "route", "explain",
        ] {
            assert!(
                command_names.contains(&command),
                "expected {command} in skill help"
            );
        }

        for command in ["run", "compile"] {
            assert!(
                !command_names.contains(&command),
                "did not expect {command} in default skill help"
            );
        }
    }

    #[test]
    fn skill_route_accepts_explain_flag() {
        let cli = Cli::try_parse_from([
            "air",
            "skill",
            "route",
            "fix a security vulnerability",
            "--explain",
        ])
        .unwrap();

        let Command::Skill {
            command:
                SkillCommand::Route {
                    task,
                    top_k,
                    explain,
                    no_memory,
                    ..
                },
        } = cli.command
        else {
            panic!("expected skill route command");
        };

        assert_eq!(task, "fix a security vulnerability");
        assert_eq!(top_k, 3);
        assert!(explain);
        assert!(!no_memory);
    }

    #[test]
    fn mcp_audit_accepts_tool_config() {
        let cli = Cli::try_parse_from([
            "air",
            "mcp",
            "audit",
            "github",
            "--tool-config",
            "tools.json",
        ])
        .unwrap();

        let Command::Mcp {
            command: McpCommand::Audit { tool, tool_config },
        } = cli.command
        else {
            panic!("expected mcp audit command");
        };

        assert_eq!(tool, Some("github".to_string()));
        assert_eq!(tool_config, PathBuf::from("tools.json"));
    }

    #[test]
    fn audit_run_accepts_artifact_dir_and_report() {
        let cli = Cli::try_parse_from([
            "air",
            "audit",
            "run",
            "target/generated/code-run-artifacts/demo",
            "--report",
            "target/audit.md",
        ])
        .unwrap();

        let Command::Audit {
            command:
                AuditCommand::Run {
                    artifact_dir,
                    report,
                },
        } = cli.command
        else {
            panic!("expected audit run command");
        };

        assert_eq!(
            artifact_dir,
            PathBuf::from("target/generated/code-run-artifacts/demo")
        );
        assert_eq!(report, Some(PathBuf::from("target/audit.md")));
    }

    #[test]
    fn audit_collect_accepts_roots_and_outputs() {
        let cli = Cli::try_parse_from([
            "air",
            "audit",
            "collect",
            "--from",
            "target/generated/code-run-artifacts",
            "--from",
            ".air/runs",
            "--since-unix",
            "1770000000",
            "--limit",
            "25",
            "--out-dir",
            ".air/audit/window",
            "--report",
            ".air/audit/window.md",
        ])
        .unwrap();

        let Command::Audit {
            command:
                AuditCommand::Collect {
                    from,
                    since_unix,
                    limit,
                    out_dir,
                    report,
                },
        } = cli.command
        else {
            panic!("expected audit collect command");
        };

        assert_eq!(
            from,
            vec![
                PathBuf::from("target/generated/code-run-artifacts"),
                PathBuf::from(".air/runs")
            ]
        );
        assert_eq!(since_unix, Some(1_770_000_000));
        assert_eq!(limit, Some(25));
        assert_eq!(out_dir, Some(PathBuf::from(".air/audit/window")));
        assert_eq!(report, Some(PathBuf::from(".air/audit/window.md")));
    }

    #[test]
    fn dream_run_accepts_window_and_outputs() {
        let cli = Cli::try_parse_from([
            "air",
            "dream",
            "run",
            "--from",
            "target/generated",
            "--since-unix",
            "1770000000",
            "--limit",
            "25",
            "--out-dir",
            ".air/dream/nightly",
            "--write-regressions",
        ])
        .unwrap();

        let Command::Dream {
            command:
                DreamCommand::Run {
                    from,
                    since_unix,
                    limit,
                    out_dir,
                    write_regressions,
                    mode,
                    experiment,
                    top_findings,
                    budget_seconds,
                    candidates,
                    no_advance,
                    advance_limit,
                    advance_timeout_seconds,
                    full,
                },
        } = cli.command
        else {
            panic!("expected dream run command");
        };

        assert_eq!(from, vec![PathBuf::from("target/generated")]);
        assert_eq!(since_unix, Some(1_770_000_000));
        assert_eq!(limit, Some(25));
        assert_eq!(out_dir, Some(PathBuf::from(".air/dream/nightly")));
        assert_eq!(mode, DreamMode::Deep);
        assert!(write_regressions);
        assert!(!experiment);
        assert_eq!(top_findings, 1);
        assert_eq!(budget_seconds, None);
        assert_eq!(candidates, 1);
        assert!(!no_advance);
        assert_eq!(advance_limit, 25);
        assert_eq!(advance_timeout_seconds, None);
        assert!(!full);
    }

    #[test]
    fn dream_experiment_requires_written_regressions() {
        let err = Cli::try_parse_from([
            "air",
            "dream",
            "run",
            "--from",
            "target/generated",
            "--experiment",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn dream_experiment_rejects_excessive_candidate_counts() {
        let err = Cli::try_parse_from([
            "air",
            "dream",
            "run",
            "--write-regressions",
            "--experiment",
            "--top-findings",
            "1000",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);

        let err = Cli::try_parse_from([
            "air",
            "dream",
            "run",
            "--write-regressions",
            "--experiment",
            "--candidates",
            "1000",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn dream_full_conflicts_with_explicit_since() {
        let err = Cli::try_parse_from([
            "air",
            "dream",
            "run",
            "--from",
            "target/generated",
            "--since-unix",
            "1770000000",
            "--full",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn dream_state_parse() {
        let cli = Cli::try_parse_from(["air", "dream", "state"]).unwrap();

        let Command::Dream {
            command: DreamCommand::State,
        } = cli.command
        else {
            panic!("expected dream state command");
        };
    }

    #[test]
    fn dream_findings_commands_parse() {
        let list = Cli::try_parse_from([
            "air", "dream", "findings", "list", "--status", "open", "--limit", "5",
        ])
        .unwrap();
        let Command::Dream {
            command:
                DreamCommand::Findings {
                    command: DreamFindingsCommand::List { status, limit },
                },
        } = list.command
        else {
            panic!("expected dream findings list command");
        };
        assert_eq!(status, Some("open".to_string()));
        assert_eq!(limit, Some(5));

        let resolve = Cli::try_parse_from([
            "air",
            "dream",
            "findings",
            "resolve",
            "IMP-001",
            "--fixed-by",
            "cand-1",
        ])
        .unwrap();
        let Command::Dream {
            command:
                DreamCommand::Findings {
                    command:
                        DreamFindingsCommand::Resolve {
                            finding, fixed_by, ..
                        },
                },
        } = resolve.command
        else {
            panic!("expected dream findings resolve command");
        };
        assert_eq!(finding, "IMP-001");
        assert_eq!(fixed_by, Some("cand-1".to_string()));
    }

    #[test]
    fn memory_commands_parse() {
        let list = Cli::try_parse_from([
            "air",
            "memory",
            "list",
            "--kind",
            "failure",
            "--status",
            "candidate",
            "--limit",
            "5",
        ])
        .unwrap();
        let Command::Memory {
            command:
                MemoryCommand::List {
                    kind,
                    status,
                    limit,
                    ..
                },
        } = list.command
        else {
            panic!("expected memory list command");
        };
        assert_eq!(kind, Some("failure".to_string()));
        assert_eq!(status, Some("candidate".to_string()));
        assert_eq!(limit, Some(5));

        let search = Cli::try_parse_from(["air", "memory", "search", "verification"]).unwrap();
        let Command::Memory {
            command: MemoryCommand::Search { query, .. },
        } = search.command
        else {
            panic!("expected memory search command");
        };
        assert_eq!(query, "verification");

        let view = Cli::try_parse_from(["air", "memory", "view", "mem_failure_abc", "--evidence"])
            .unwrap();
        let Command::Memory {
            command: MemoryCommand::View { id, evidence, .. },
        } = view.command
        else {
            panic!("expected memory view command");
        };
        assert_eq!(id, "mem_failure_abc");
        assert!(evidence);

        let promote = Cli::try_parse_from(["air", "memory", "promote", "mem_failure_abc"]).unwrap();
        let Command::Memory {
            command: MemoryCommand::Promote { id, .. },
        } = promote.command
        else {
            panic!("expected memory promote command");
        };
        assert_eq!(id, "mem_failure_abc");

        let causal = Cli::try_parse_from([
            "air",
            "memory",
            "causal-eval",
            "mem_failure_abc",
            "--outcome",
            "helped",
            "--evidence",
            "target/generated/compare-no-memory.json",
            "--note",
            "same task passed only with memory",
        ])
        .unwrap();
        let Command::Memory {
            command:
                MemoryCommand::CausalEval {
                    id,
                    outcome,
                    evidence,
                    note,
                    ..
                },
        } = causal.command
        else {
            panic!("expected memory causal-eval command");
        };
        assert_eq!(id, "mem_failure_abc");
        assert_eq!(outcome, "helped");
        assert_eq!(
            evidence,
            PathBuf::from("target/generated/compare-no-memory.json")
        );
        assert_eq!(note, Some("same task passed only with memory".to_string()));

        let pack =
            Cli::try_parse_from(["air", "memory", "pack", "fix verification failure"]).unwrap();
        let Command::Memory {
            command: MemoryCommand::Pack { task, .. },
        } = pack.command
        else {
            panic!("expected memory pack command");
        };
        assert_eq!(task, "fix verification failure");

        let graph = Cli::try_parse_from(["air", "memory", "graph", "--limit", "10"]).unwrap();
        let Command::Memory {
            command: MemoryCommand::Graph { limit, .. },
        } = graph.command
        else {
            panic!("expected memory graph command");
        };
        assert_eq!(limit, Some(10));

        let scorecard =
            Cli::try_parse_from(["air", "memory", "scorecard", "--limit", "10"]).unwrap();
        let Command::Memory {
            command: MemoryCommand::Scorecard { limit, .. },
        } = scorecard.command
        else {
            panic!("expected memory scorecard command");
        };
        assert_eq!(limit, Some(10));

        let advance =
            Cli::try_parse_from(["air", "memory", "advance", "--limit", "3", "--skill-bench"])
                .unwrap();
        let Command::Memory {
            command: MemoryCommand::Advance {
                limit, skill_bench, ..
            },
        } = advance.command
        else {
            panic!("expected memory advance command");
        };
        assert_eq!(limit, Some(3));
        assert!(skill_bench);

        let draft = Cli::try_parse_from([
            "air",
            "memory",
            "skill-draft",
            "mem_procedure_abc",
            "--out-dir",
            ".air/memory/drafts/skills/test",
        ])
        .unwrap();
        let Command::Memory {
            command: MemoryCommand::SkillDraft { id, out_dir, .. },
        } = draft.command
        else {
            panic!("expected memory skill-draft command");
        };
        assert_eq!(id, "mem_procedure_abc");
        assert_eq!(
            out_dir,
            Some(PathBuf::from(".air/memory/drafts/skills/test"))
        );

        let evaluate = Cli::try_parse_from([
            "air",
            "memory",
            "skill-evaluate",
            "mem_procedure_abc",
            "--bench",
        ])
        .unwrap();
        let Command::Memory {
            command: MemoryCommand::SkillEvaluate { id, bench, .. },
        } = evaluate.command
        else {
            panic!("expected memory skill-evaluate command");
        };
        assert_eq!(id, "mem_procedure_abc");
        assert!(bench);

        let policy =
            Cli::try_parse_from(["air", "memory", "policy-check", "mem_policy_abc"]).unwrap();
        let Command::Memory {
            command: MemoryCommand::PolicyCheck { id, .. },
        } = policy.command
        else {
            panic!("expected memory policy-check command");
        };
        assert_eq!(id, "mem_policy_abc");

        let retire = Cli::try_parse_from([
            "air",
            "memory",
            "retire",
            "mem_failure_abc",
            "--reason",
            "stale",
        ])
        .unwrap();
        let Command::Memory {
            command: MemoryCommand::Retire { id, reason, .. },
        } = retire.command
        else {
            panic!("expected memory retire command");
        };
        assert_eq!(id, "mem_failure_abc");
        assert_eq!(reason, Some("stale".to_string()));
    }

    #[test]
    fn brain_commands_parse() {
        let summary = Cli::try_parse_from(["air", "brain", "--limit", "5"]).unwrap();
        let Command::Brain { command, limit, .. } = summary.command else {
            panic!("expected brain command");
        };
        assert!(command.is_none());
        assert_eq!(limit, Some(5));

        let skills = Cli::try_parse_from(["air", "brain", "skills"]).unwrap();
        let Command::Brain {
            command: Some(BrainCommand::Skills),
            ..
        } = skills.command
        else {
            panic!("expected brain skills command");
        };

        let view = Cli::try_parse_from(["air", "brain", "view", "mem_procedure_abc"]).unwrap();
        let Command::Brain {
            command: Some(BrainCommand::View { id }),
            ..
        } = view.command
        else {
            panic!("expected brain view command");
        };
        assert_eq!(id, "mem_procedure_abc");

        let report =
            Cli::try_parse_from(["air", "brain", "report", "--out", ".air/brain/report.md"])
                .unwrap();
        let Command::Brain {
            command: Some(BrainCommand::Report { out }),
            ..
        } = report.command
        else {
            panic!("expected brain report command");
        };
        assert_eq!(out, Some(PathBuf::from(".air/brain/report.md")));
    }

    #[test]
    fn improve_evaluate_accepts_regression_file() {
        let cli = Cli::try_parse_from([
            "air",
            "improve",
            "evaluate",
            "IMP-001",
            "--regression-file",
            "target/generated/improve/suggested-regressions/imp-001.json",
            "--report",
            "target/generated/improve/eval.md",
        ])
        .unwrap();

        let Command::Improve {
            command,
            from: _,
            out_dir: _,
            write_regressions: _,
        } = cli.command
        else {
            panic!("expected improve command");
        };
        let ImproveAction::Evaluate {
            finding,
            report,
            regression_file,
        } = improve_action(command)
        else {
            panic!("expected improve evaluate action");
        };

        assert_eq!(finding, "IMP-001");
        assert_eq!(
            regression_file,
            Some(PathBuf::from(
                "target/generated/improve/suggested-regressions/imp-001.json"
            ))
        );
        assert_eq!(
            report,
            Some(PathBuf::from("target/generated/improve/eval.md"))
        );
    }

    #[test]
    fn eval_manifest_and_check_parse() {
        let manifest = Cli::try_parse_from([
            "air",
            "eval",
            "manifest",
            "--out",
            ".air/evals/manifest.json",
            "--include",
            "skills/code-agent/benches",
        ])
        .unwrap();
        let Command::Eval {
            command: EvalCommand::Manifest { out, include },
        } = manifest.command
        else {
            panic!("expected eval manifest command");
        };
        assert_eq!(out, Some(PathBuf::from(".air/evals/manifest.json")));
        assert_eq!(include, vec![PathBuf::from("skills/code-agent/benches")]);

        let check = Cli::try_parse_from([
            "air",
            "eval",
            "check",
            "--manifest",
            ".air/evals/manifest.json",
        ])
        .unwrap();
        let Command::Eval {
            command: EvalCommand::Check { manifest },
        } = check.command
        else {
            panic!("expected eval check command");
        };
        assert_eq!(manifest, Some(PathBuf::from(".air/evals/manifest.json")));
    }

    #[test]
    fn self_prepare_and_compare_parse() {
        let prepare = Cli::try_parse_from([
            "air",
            "self",
            "prepare",
            "IMP-001",
            "--candidates",
            "4",
            "--out-dir",
            ".air/candidates",
        ])
        .unwrap();
        let Command::Self_ {
            command:
                SelfCommand::Prepare {
                    finding,
                    candidates,
                    out_dir,
                },
        } = prepare.command
        else {
            panic!("expected self prepare command");
        };
        assert_eq!(finding, "IMP-001");
        assert_eq!(candidates, 4);
        assert_eq!(out_dir, Some(PathBuf::from(".air/candidates")));

        let capture = Cli::try_parse_from([
            "air",
            "self",
            "capture",
            "IMP-001",
            "cand-2",
            "--out-dir",
            ".air/candidates",
        ])
        .unwrap();
        let Command::Self_ {
            command:
                SelfCommand::Capture {
                    finding,
                    candidate,
                    out_dir,
                },
        } = capture.command
        else {
            panic!("expected self capture command");
        };
        assert_eq!(finding, "IMP-001");
        assert_eq!(candidate, "cand-2");
        assert_eq!(out_dir, Some(PathBuf::from(".air/candidates")));

        let fix = Cli::try_parse_from([
            "air",
            "self",
            "fix",
            "IMP-001",
            "--candidates",
            "2",
            "--skill",
            "code-agent",
            "--task",
            "fix the repeated read loop",
            "--from",
            "target/generated/improve",
            "--out-dir",
            ".air/candidates",
            "--model-config",
            "models.json",
            "--regression-file",
            "target/generated/improve/suggested-regressions/imp-001.json",
            "--evaluate",
            "--keep-worktrees",
            "--allow-dirty-bootstrap",
        ])
        .unwrap();
        let Command::Self_ {
            command:
                SelfCommand::Fix {
                    finding,
                    candidates,
                    skill,
                    task,
                    from,
                    out_dir,
                    model_config,
                    regression_file,
                    evaluate,
                    keep_worktrees,
                    allow_dirty_bootstrap,
                },
        } = fix.command
        else {
            panic!("expected self fix command");
        };
        assert_eq!(finding, "IMP-001");
        assert_eq!(candidates, 2);
        assert_eq!(skill, Some("code-agent".to_string()));
        assert_eq!(task, Some("fix the repeated read loop".to_string()));
        assert_eq!(from, vec![PathBuf::from("target/generated/improve")]);
        assert_eq!(out_dir, Some(PathBuf::from(".air/candidates")));
        assert_eq!(model_config, Some(PathBuf::from("models.json")));
        assert_eq!(
            regression_file,
            Some(PathBuf::from(
                "target/generated/improve/suggested-regressions/imp-001.json"
            ))
        );
        assert!(evaluate);
        assert!(keep_worktrees);
        assert!(allow_dirty_bootstrap);

        let compare = Cli::try_parse_from([
            "air",
            "self",
            "compare",
            "IMP-001",
            "--from",
            ".air/candidates/IMP-001",
            "--out",
            ".air/candidates/IMP-001.md",
        ])
        .unwrap();
        let Command::Self_ {
            command: SelfCommand::Compare { finding, from, out },
        } = compare.command
        else {
            panic!("expected self compare command");
        };
        assert_eq!(finding, "IMP-001");
        assert_eq!(from, vec![PathBuf::from(".air/candidates/IMP-001")]);
        assert_eq!(out, Some(PathBuf::from(".air/candidates/IMP-001.md")));
    }

    #[test]
    fn run_accepts_natural_language_task() {
        let cli = Cli::try_parse_from([
            "air",
            "run",
            "use TDD to fix the failing add function",
            "--skills",
            "tdd-workflow",
            "--mode",
            "code",
            "--explain",
        ])
        .unwrap();

        let Command::Run {
            target,
            mode,
            skills,
            execute,
            explain,
            ..
        } = cli.command
        else {
            panic!("expected run command");
        };

        assert_eq!(target, "use TDD to fix the failing add function");
        assert_eq!(mode, EntryMode::Code);
        assert_eq!(skills, vec!["tdd-workflow"]);
        assert!(!execute);
        assert!(explain);
    }

    #[test]
    fn run_project_execute_is_explicit() {
        let cli = Cli::try_parse_from([
            "air",
            "run",
            "refactor the whole crate",
            "--mode",
            "project",
            "--execute",
        ])
        .unwrap();

        let Command::Run {
            mode,
            plan_only,
            execute,
            ..
        } = cli.command
        else {
            panic!("expected run command");
        };

        assert_eq!(mode, EntryMode::Project);
        assert!(!plan_only);
        assert!(execute);
    }

    #[test]
    fn bundled_subagent_configs_use_dev_run_plan() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(|path| path.parent())
            .expect("air-cli crate lives under crates/air-cli");
        let config_path = repo_root.join("skills/code-agent/tools.json");
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        let command = config
            .pointer("/tools/task/subagents/explore/command")
            .and_then(serde_json::Value::as_array)
            .expect("explore subagent command");

        let run_plan_index = command
            .iter()
            .position(|value| value == "run-plan")
            .expect("subagent command includes run-plan");
        assert_eq!(
            command
                .get(run_plan_index.saturating_sub(1))
                .and_then(serde_json::Value::as_str),
            Some("dev")
        );
    }

    #[test]
    fn dev_run_module_accepts_low_level_module_input() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "run-module",
            "examples/model-smoke.air.yaml",
            "--input",
            "examples/model-smoke.input.json",
        ])
        .unwrap();

        let Command::Dev {
            command: DevCommand::RunModule { file, input, .. },
        } = cli.command
        else {
            panic!("expected dev run-module command");
        };

        assert_eq!(file, PathBuf::from("examples/model-smoke.air.yaml"));
        assert_eq!(input, PathBuf::from("examples/model-smoke.input.json"));
    }

    #[test]
    fn skill_explain_accepts_code_agent() {
        let cli = Cli::try_parse_from(["air", "skill", "explain", "code-agent"]).unwrap();

        let Command::Skill {
            command: SkillCommand::Explain { skill, profile },
        } = cli.command
        else {
            panic!("expected skill explain command");
        };

        assert_eq!(skill, "code-agent");
        assert_eq!(profile, None);
    }

    #[test]
    fn skill_validate_audit_parse() {
        for args in [
            ["air", "skill", "validate", "code-agent"].as_slice(),
            ["air", "skill", "audit", "code-agent"].as_slice(),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Command::Skill { .. } = cli.command else {
                panic!("expected skill command");
            };
        }
    }

    #[test]
    fn resume_plan_accepts_profile_without_positional_plan() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "resume-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
            "--state",
            "target/generated/deep_research_profile.state.json",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::ResumePlan {
                    plan,
                    profile,
                    store,
                    input,
                    state,
                    ..
                },
        } = cli.command
        else {
            panic!("expected dev resume-plan command");
        };

        assert_eq!(plan, None);
        assert_eq!(
            profile,
            Some(PathBuf::from(
                "examples/deep-research/profile.air-profile.yaml"
            ))
        );
        assert_eq!(store, None);
        assert_eq!(input, None);
        assert_eq!(
            state,
            PathBuf::from("target/generated/deep_research_profile.state.json")
        );
    }

    #[test]
    fn run_plan_jit_cache_reuses_specialized_dynamic_hot_path() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let unique = format!(
            "air-jit-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let cache_dir = std::env::temp_dir().join(&unique);
        let trace_path = std::env::temp_dir().join(format!("{unique}.trace.jsonl"));
        let store_path = root
            .join("target/generated/test-tmp")
            .join(format!("{unique}.store.yaml"));
        fs::create_dir_all(store_path.parent().unwrap()).unwrap();

        let plan_path = root.join("tests/plans/dynamic-smoke.air-plan.yaml");
        let store = air_linker::parse_module_store_file(
            root.join("tests/plans/dynamic-smoke.air-store.yaml"),
        )
        .unwrap();
        fs::write(&store_path, serde_yaml::to_string(&store).unwrap()).unwrap();
        let inputs = serde_json::Map::new();

        run_plan_with_inputs(
            plan_path.clone(),
            store_path.clone(),
            inputs.clone(),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: None,
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: Some(cache_dir.clone()),
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        let cached_plans = fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "yaml")
            })
            .collect::<Vec<_>>();
        assert_eq!(cached_plans.len(), 1);
        assert!(fs::read_to_string(&cached_plans[0])
            .unwrap()
            .contains("topic_1"));

        run_plan_with_inputs(
            plan_path,
            store_path.clone(),
            inputs,
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: Some(cache_dir.clone()),
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        let trace = read_trace_jsonl(&trace_path).unwrap();
        let resolved = trace
            .iter()
            .find(|event| event.agent == "$planner" && event.action == "resolve_plan")
            .and_then(|event| event.output.as_ref())
            .unwrap();
        assert!(resolved["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == json!("topic_1")));
        assert!(!trace
            .iter()
            .any(|event| { event.agent == "$planner" && event.action == "resolve_dynamic_plan" }));

        let _ = fs::remove_dir_all(cache_dir);
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn run_plan_uses_tool_config_approvals() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let unique = format!(
            "air-approval-smoke-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let store_path = root
            .join("target/generated/test-tmp")
            .join(format!("{unique}.store.yaml"));
        fs::create_dir_all(store_path.parent().unwrap()).unwrap();
        let trace_path = std::env::temp_dir().join(format!("{unique}.trace.jsonl"));
        let plan_path = root.join("tests/plans/approval-smoke.air-plan.yaml");
        let tool_config = root.join("tests/plans/approval-smoke.tools.json");

        let store = air_linker::parse_module_store_file(
            root.join("tests/plans/approval-smoke.air-store.yaml"),
        )
        .unwrap();
        fs::write(&store_path, serde_yaml::to_string(&store).unwrap()).unwrap();

        run_plan_with_inputs(
            plan_path,
            store_path.clone(),
            serde_json::Map::from_iter([("deployment_id".to_string(), json!("deploy-123"))]),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: Some(tool_config),
                model_replay: None,
            },
        )
        .unwrap();

        let trace = read_trace_jsonl(&trace_path).unwrap();
        let approval = trace
            .iter()
            .find(|event| event.action == "approval")
            .expect("approval event");
        assert_eq!(approval.status, TraceStatus::Ok);
        assert_eq!(approval.meta.as_ref().unwrap()["approved"], json!(true));
        assert_eq!(
            approval.meta.as_ref().unwrap()["approver"],
            json!("verification")
        );

        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn run_plan_accepts_parallel_flag_with_profile() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "run-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
            "--parallel",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::RunPlan {
                    plan,
                    profile,
                    parallel,
                    ..
                },
        } = cli.command
        else {
            panic!("expected dev run-plan command");
        };

        assert_eq!(plan, None);
        assert_eq!(
            profile,
            Some(PathBuf::from(
                "examples/deep-research/profile.air-profile.yaml"
            ))
        );
        assert!(parallel);
    }

    #[test]
    fn run_plan_parallel_executes_schedule_groups_from_cli_path() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let trace_path = std::env::temp_dir().join(format!(
            "air-parallel-cli-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store_path = temp_repo_file_path(&root, "air-parallel-cli-store", "yaml");
        fs::write(
            &store_path,
            r#"store:
  name: parallel-smoke-store
  version: 0.1.0

modules:
  test.model_smoke@0.1.0:
    path: tests/agents/model-smoke.air.yaml
    kind: primitive
    visibility: public
"#,
        )
        .unwrap();

        run_plan_with_inputs(
            root.join("tests/plans/parallel-smoke.air-plan.yaml"),
            store_path.clone(),
            serde_json::Map::from_iter([("prompt".to_string(), json!("parallel cli"))]),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: true,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        let trace = read_trace_jsonl(&trace_path).unwrap();
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);

        assert!(trace.iter().any(|event| {
            event.agent == "$system"
                && event.action == "schedule_batch"
                && event.input.as_ref().unwrap()["execution"] == "parallel"
        }));
        assert!(trace.iter().any(|event| {
            event.agent == "$system"
                && event.action == "return"
                && event.output.as_ref().unwrap()["left"]["input"] == json!("parallel cli")
                && event.output.as_ref().unwrap()["right"]["input"] == json!("parallel cli")
        }));
    }

    #[test]
    fn replay_specializes_trace_to_validated_run_plan() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let trace_path = temp_repo_file_path(&root, "air-specialize-cli", "jsonl");
        let output_path = trace_path.with_extension("air-plan.yaml");
        let identity_path = trace_path.with_extension("identity.json");
        let store_path = trace_path.with_extension("store.yaml");
        fs::write(
            &store_path,
            r#"store:
  name: parallel-smoke-store
  version: 0.1.0

modules:
  test.model_smoke@0.1.0:
    path: tests/agents/model-smoke.air.yaml
    kind: primitive
    visibility: public
"#,
        )
        .unwrap();

        run_plan_with_inputs(
            root.join("tests/plans/parallel-smoke.air-plan.yaml"),
            store_path.clone(),
            serde_json::Map::from_iter([("prompt".to_string(), json!("jit cli"))]),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: true,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        replay(ReplayOptions {
            trace: trace_path.clone(),
            output: Some(output_path.clone()),
            specialize_run_plan: true,
            store: Some(store_path.clone()),
            identity_out: Some(identity_path.clone()),
            stats: false,
        })
        .unwrap();

        let plan = air_linker::parse_run_plan_file(&output_path).unwrap();
        let identity: Value =
            serde_json::from_str(&fs::read_to_string(&identity_path).unwrap()).unwrap();
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(output_path);
        let _ = fs::remove_file(identity_path);
        let _ = fs::remove_file(store_path);

        assert_eq!(plan.plan.name, "parallel-smoke");
        assert_eq!(
            identity["nodes"]["left"]["store_module"],
            json!("test.model_smoke@0.1.0")
        );
    }

    #[test]
    fn specialized_dynamic_trace_can_be_validated() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let trace_path = temp_repo_file_path(&root, "air-dynamic-specialize", "jsonl");
        let store_path = trace_path.with_extension("store.yaml");
        let specialized_path = trace_path.with_extension("specialized.air-plan.yaml");
        let identity_path = trace_path.with_extension("identity.json");
        let store = air_linker::parse_module_store_file(
            root.join("tests/plans/dynamic-smoke.air-store.yaml"),
        )
        .unwrap();
        fs::write(&store_path, serde_yaml::to_string(&store).unwrap()).unwrap();

        run_plan_with_inputs(
            root.join("tests/plans/dynamic-smoke.air-plan.yaml"),
            store_path.clone(),
            serde_json::Map::new(),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        replay(ReplayOptions {
            trace: trace_path.clone(),
            output: Some(specialized_path.clone()),
            specialize_run_plan: true,
            store: Some(store_path.clone()),
            identity_out: Some(identity_path.clone()),
            stats: false,
        })
        .unwrap();

        let specialized = air_linker::parse_run_plan_file(&specialized_path).unwrap();
        let validation = air_linker::validate_run_plan(&specialized, &store, &root);
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);
        let _ = fs::remove_file(specialized_path);
        let _ = fs::remove_file(identity_path);

        assert!(validation.is_success(), "{:?}", validation.diagnostics);
        assert!(specialized.dynamic.is_none());
        assert!(specialized.nodes.iter().any(|node| node.id == "topic_1"));
    }

    #[test]
    fn validate_plan_accepts_profile_without_positional_plan() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "validate-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::ValidatePlan {
                    plan,
                    profile,
                    store,
                    explain,
                },
        } = cli.command
        else {
            panic!("expected dev validate-plan command");
        };

        assert_eq!(plan, None);
        assert_eq!(
            profile,
            Some(PathBuf::from(
                "examples/deep-research/profile.air-profile.yaml"
            ))
        );
        assert_eq!(store, None);
        assert!(!explain);
    }

    #[test]
    fn validate_plan_accepts_explain_flag() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "validate-plan",
            "tests/plans/approval-smoke.air-plan.yaml",
            "--store",
            "tests/plans/approval-smoke.air-store.yaml",
            "--explain",
        ])
        .unwrap();

        let Command::Dev {
            command: DevCommand::ValidatePlan { explain, .. },
        } = cli.command
        else {
            panic!("expected dev validate-plan command");
        };

        assert!(explain);
    }

    #[test]
    fn plan_explain_accepts_no_model_config() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "make-plan",
            "--explain",
            "--store",
            "skills/code-agent/module-store.air-store.yaml",
            "--task",
            "Explore the command_run implementation",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::MakePlan {
                    model_config,
                    explain,
                    ..
                },
        } = cli.command
        else {
            panic!("expected dev make-plan command");
        };

        assert_eq!(model_config, None);
        assert!(explain);
    }

    #[test]
    fn plan_requires_model_config_without_explain() {
        let error = Cli::try_parse_from([
            "air",
            "dev",
            "make-plan",
            "--store",
            "skills/code-agent/module-store.air-store.yaml",
            "--task",
            "Explore the command_run implementation",
        ])
        .unwrap_err();

        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn model_profile_keys_are_env_safe() {
        assert_eq!(normalize_model_profile_key("glm"), "GLM");
        assert_eq!(normalize_model_profile_key("local-api"), "LOCAL_API");
        assert_eq!(normalize_model_profile_key(" local api "), "LOCAL_API");
    }

    #[test]
    fn timeout_wrapper_strips_timeout_flag_before_child_run() {
        let args = [
            OsString::from("air"),
            OsString::from("run"),
            OsString::from("--timeout-seconds"),
            OsString::from("5"),
            OsString::from("fix bug"),
            OsString::from("--timeout-seconds=9"),
        ];
        assert_eq!(
            strip_timeout_seconds(&args),
            vec![
                OsString::from("run"),
                OsString::from("fix bug"),
                OsString::from("--timeout-seconds=9")
            ]
        );
    }

    #[test]
    fn timeout_wrapper_only_strips_run_timeout_flag() {
        let args = [
            OsString::from("air"),
            OsString::from("dream"),
            OsString::from("run"),
            OsString::from("--timeout-seconds"),
            OsString::from("300"),
        ];
        assert_eq!(
            strip_timeout_seconds(&args),
            vec![
                OsString::from("dream"),
                OsString::from("run"),
                OsString::from("--timeout-seconds"),
                OsString::from("300")
            ]
        );
    }
}
