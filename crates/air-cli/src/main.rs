mod bench;
mod code_agent;
mod entry;
mod mcp;
mod models;
mod native_loop;
mod ops;
mod profile;
mod project;
mod review_agent;
mod self_lab;
mod skill;
mod tools;
mod trace_io;
use crate::bench::{bench_code_agent, bench_skill, BenchCodeAgentOptions, BenchSkillOptions};
use crate::entry::{run_entry_task, EntryMode, EntryTaskOptions};
use crate::mcp::{
    audit_mcp, call_mcp, explain_mcp, list_mcp, McpAuditOptions, McpCallOptions, McpExplainOptions,
    McpListOptions,
};
use crate::native_loop::{run_native_loop, NativeLoopKind, NativeLoopOptions};
use crate::ops::run_current_air_passthrough;
use crate::profile::read_json_object;
use crate::project::{
    bench_project, default_project_file, project_plan, project_run, project_status, project_verify,
    BenchProjectOptions, ProjectPlanOptions, ProjectRunOptions, ProjectStatusOptions,
    ProjectVerifyOptions,
};
use crate::self_lab::{
    self_capture, self_compare, self_fix, self_prepare, SelfCaptureOptions, SelfCompareOptions,
    SelfFixOptions, SelfPrepareOptions,
};
use crate::skill::{
    audit_skill, audit_skill_value, explain_skill, import_skill, list_skills, route_skill,
    upgrade_skill, validate_skill, validate_skill_value, SkillRouteOptions, SkillUpgradeOptions,
};
use crate::trace_io::{replay, ReplayOptions};
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
    /// Advanced native-loop runtime tools.
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
    /// Run a native Rust AIR loop without the YAML state-machine VM.
    #[command(hide = true)]
    NativeLoop {
        /// Native loop kind: code-edit, explore, project-scout, review, or bench.
        #[arg(long)]
        kind: String,

        /// Path to a JSON object containing at least task.
        #[arg(long)]
        input: PathBuf,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Trace writing mode when --trace-out is set.
        #[arg(long, value_enum, default_value_t = TraceMode::Redact)]
        trace: TraceMode,

        /// Print human-readable execution logs to stderr.
        #[arg(short = 'v', long)]
        log: bool,
    },
    /// Replay final output from a JSONL trace.
    Replay {
        /// Path to a trace JSONL file.
        trace: PathBuf,

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
        DevCommand::NativeLoop {
            kind,
            input,
            model_config,
            tool_config,
            trace_out,
            trace,
            log,
        } => run_native_loop_command(
            kind,
            input,
            option_or_config(model_config, config.model_config()),
            tool_config,
            trace.trace_out(trace_out),
            trace.redact(),
            trace.raw(),
            log,
        ),
        DevCommand::Replay { trace, stats } => replay(ReplayOptions { trace, stats }),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_native_loop_command(
    kind: String,
    input: PathBuf,
    model_config: Option<PathBuf>,
    tool_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    trace_raw: bool,
    log: bool,
) -> Result<()> {
    let inputs = read_json_object(input, "native loop input")?;
    let task = inputs
        .get("task")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("native loop input.task must be a non-empty string"))?
        .to_string();
    let verification_command = inputs
        .get("verification_command")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let requires_edit = inputs
        .get("requires_edit")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let outputs = run_native_loop(NativeLoopOptions {
        kind: parse_native_loop_kind(&kind)?,
        task,
        verification_command,
        requires_edit,
        model_config,
        tool_config,
        model_replay: None,
        trace_out,
        trace_redact,
        trace_raw,
        log,
    })?;
    println!("{}", serde_json::to_string_pretty(&outputs)?);
    Ok(())
}

fn parse_native_loop_kind(kind: &str) -> Result<NativeLoopKind> {
    match kind {
        "code-edit" | "edit" | "code" => Ok(NativeLoopKind::CodeEdit),
        "explore" | "code-explore" => Ok(NativeLoopKind::Explore),
        "project-scout" | "scout" => Ok(NativeLoopKind::ProjectScout),
        "review" => Ok(NativeLoopKind::Review),
        "bench" => Ok(NativeLoopKind::Bench),
        _ => anyhow::bail!(
            "unknown native loop kind {kind:?}; expected code-edit, explore, project-scout, review, or bench"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::read_json_object;
    use crate::tools::{provider_error_snippet, search_docs, ConfigTools, LocalDoc};
    use air_runtime::read_trace_jsonl;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn parse_native_loop_kind_accepts_expected_aliases() {
        assert_eq!(
            parse_native_loop_kind("code-edit").unwrap(),
            NativeLoopKind::CodeEdit
        );
        assert_eq!(
            parse_native_loop_kind("code").unwrap(),
            NativeLoopKind::CodeEdit
        );
        assert_eq!(
            parse_native_loop_kind("explore").unwrap(),
            NativeLoopKind::Explore
        );
        assert_eq!(
            parse_native_loop_kind("project-scout").unwrap(),
            NativeLoopKind::ProjectScout
        );
        assert_eq!(
            parse_native_loop_kind("review").unwrap(),
            NativeLoopKind::Review
        );
        assert!(parse_native_loop_kind("legacy-plan").is_err());
    }

    #[test]
    fn dev_native_loop_accepts_input_and_configs() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "native-loop",
            "--kind",
            "explore",
            "--input",
            "input.json",
            "--model-config",
            "models.json",
            "--tool-config",
            "tools.json",
            "--trace-out",
            "trace.jsonl",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::NativeLoop {
                    kind,
                    input,
                    model_config,
                    tool_config,
                    trace_out,
                    ..
                },
        } = cli.command
        else {
            panic!("expected dev native-loop command");
        };

        assert_eq!(kind, "explore");
        assert_eq!(input, PathBuf::from("input.json"));
        assert_eq!(model_config, Some(PathBuf::from("models.json")));
        assert_eq!(tool_config, Some(PathBuf::from("tools.json")));
        assert_eq!(trace_out, Some(PathBuf::from("trace.jsonl")));
    }

    #[test]
    fn dev_help_exposes_no_legacy_ir_commands() {
        let mut output = Vec::new();
        let mut command = <Cli as clap::CommandFactory>::command();
        let dev = command.find_subcommand_mut("dev").unwrap();
        dev.write_help(&mut output).unwrap();
        let help = String::from_utf8(output).unwrap();

        assert!(help.contains("replay"));
        for removed in [
            "run-module",
            "run-system",
            "validate-module",
            "validate-system",
        ] {
            assert!(!help.contains(removed), "dev help still exposes {removed}");
        }
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
    fn bundled_subagent_configs_use_native_loop() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
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

        let native_loop_index = command
            .iter()
            .position(|value| value == "native-loop")
            .expect("subagent command includes native-loop");
        assert_eq!(
            command
                .get(native_loop_index.saturating_sub(1))
                .and_then(serde_json::Value::as_str),
            Some("dev")
        );
        assert!(command.iter().any(|value| value == "explore"));
    }

    #[test]
    fn code_agent_fixture_runs_through_workspace_tests() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture_root = temp_dir_path("air-code-agent-fixture");
        let trace_path = temp_repo_file_path(&root, "code-agent-minimal", "jsonl");
        let _ = fs::remove_dir_all(&fixture_root);
        copy_dir_recursive(
            &root.join("skills/code-agent"),
            &fixture_root.join("skills/code-agent"),
        );
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

        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&fixture_root).unwrap();
        let result = crate::code_agent::run_code_agent(crate::code_agent::CodeOptions {
            task: "fix the failing add function and retest".to_string(),
            verification_command: None,
            artifact_task: None,
            skill: None,
            profile: Some(fixture_root.join("skills/code-agent/edit.air-profile.yaml")),
            model_config: Some(fixture_root.join("skills/code-agent/fixtures/model-fixtures.json")),
            trace_out: Some(trace_path.clone()),
            trace_redact: false,
            trace_raw: true,
            log: false,
            tool_config: Some(fixture_tool_config),
            artifact_out: None,
            artifact_extra: BTreeMap::new(),
            verdict_constraints: air_code_artifact::CodeRunVerdictConstraints::code_edit(),
            replay_artifact: None,
            replay_from: None,
        });
        std::env::set_current_dir(original_cwd).unwrap();
        let result = result.unwrap();

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
                "edit"
            ])
        );

        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_dir_all(fixture_root);
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
}
