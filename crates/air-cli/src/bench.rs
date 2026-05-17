use crate::code_agent::{run_code_agent, CodeOptions};
use crate::code_artifact::{
    path_content_identity, read_code_run_artifact, replay_code_run_artifact, CodeRunDescriptor,
    CodeRunMode, CodeRunSkill, FailureCategory, FailureReason, WorkspaceSnapshot,
};
use crate::skill::{prepare_skill_run_context, resolve_skill_run_metadata};
use air_runtime::read_trace_jsonl;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_CODE_BENCH_SUITE: &str = "skills/code-agent/benches/rust-small/suite.json";
const DEFAULT_CODE_PROFILE: &str = "skills/code-agent/edit.air-profile.yaml";
const DEFAULT_MODEL_CONFIG: &str = "examples/bigmodel-openai-compatible.json";

pub(crate) struct BenchCodeAgentOptions {
    pub(crate) suite: Option<PathBuf>,
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) task: Option<String>,
    pub(crate) limit: Option<usize>,
    pub(crate) log: bool,
    pub(crate) keep_workdirs: bool,
    pub(crate) refresh: bool,
}

pub(crate) struct BenchSkillOptions {
    pub(crate) skill: String,
    pub(crate) suite: Option<PathBuf>,
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) task: Option<String>,
    pub(crate) limit: Option<usize>,
    pub(crate) compare_no_skill: bool,
    pub(crate) log: bool,
    pub(crate) keep_workdirs: bool,
    pub(crate) refresh: bool,
}

#[derive(Debug, Deserialize)]
struct BenchSuite {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    subagents: bool,
    tasks: Vec<BenchTask>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BenchTask {
    id: String,
    fixture: PathBuf,
    prompt: String,
    #[serde(default)]
    verification: Vec<VerificationCommand>,
    #[serde(default)]
    diff: DiffConstraints,
}

#[derive(Debug, Deserialize, Serialize)]
struct VerificationCommand {
    command: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DiffConstraints {
    #[serde(default)]
    allowed_changed_files: Vec<String>,
    #[serde(default)]
    required_changed_files: Vec<String>,
    #[serde(default)]
    required_diff_contains: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BenchRun {
    suite: String,
    description: Option<String>,
    run_dir: String,
    profile: String,
    model_config: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runs: Option<Vec<BenchRun>>,
    summary: BenchSummary,
    tasks: Vec<TaskRun>,
}

#[derive(Debug, Default, Serialize)]
struct BenchSummary {
    total: usize,
    passed: usize,
    failed: usize,
    executed: usize,
    replayed: usize,
}

#[derive(Debug, Serialize)]
struct TaskRun {
    id: String,
    mode: CodeRunMode,
    pass: bool,
    fingerprint: String,
    artifact_path: String,
    workdir: String,
    trace_path: String,
    output_path: String,
    changed_files: Vec<String>,
    metrics: TraceMetrics,
    verification: Vec<VerificationResult>,
    diff_constraints: ConstraintResult,
    model_calls_spent: usize,
    failure_reason: Option<FailureReason>,
    error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct TraceMetrics {
    model_calls: usize,
    model_errors: usize,
    tool_calls: usize,
    tool_errors: usize,
    tool_counts: BTreeMap<String, usize>,
    first_edit_tool_index: Option<usize>,
    repeated_reads: usize,
    verification_tool_calls: usize,
    verification_failures: usize,
    repair_iterations: usize,
    subagent_calls: usize,
    subagent_errors: usize,
    subagent_model_calls: usize,
    subagent_tool_calls: usize,
    subagent_tool_errors: usize,
}

#[derive(Debug, Serialize)]
struct VerificationResult {
    command: String,
    description: Option<String>,
    success: bool,
    status: Option<i32>,
    stdout_preview: String,
    stderr_preview: String,
}

#[derive(Debug, Default, Serialize)]
struct ConstraintResult {
    passed: bool,
    violations: Vec<String>,
}

struct BenchTaskContext<'a> {
    suite_dir: &'a Path,
    run_dir: &'a Path,
    profile: &'a Path,
    model_config: &'a Path,
    artifact_cache_root: &'a Path,
    repo_root: &'a Path,
    subagents: bool,
    skill: Option<CodeRunSkill>,
    task_prefix: Option<&'a str>,
    artifact_extra: BTreeMap<String, Value>,
    log: bool,
    keep_workdirs: bool,
    refresh: bool,
}

pub(crate) fn bench_code_agent(options: BenchCodeAgentOptions) -> Result<()> {
    let repo_root = std::env::current_dir().context("resolve current directory")?;
    let suite_path = options
        .suite
        .unwrap_or_else(|| repo_root.join(DEFAULT_CODE_BENCH_SUITE));
    let suite_path = absolutize(&repo_root, suite_path);
    let suite_dir = suite_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("suite path has no parent: {}", suite_path.display()))?
        .to_path_buf();
    let suite: BenchSuite = serde_json::from_str(
        &fs::read_to_string(&suite_path)
            .with_context(|| format!("read bench suite {}", suite_path.display()))?,
    )
    .with_context(|| format!("parse bench suite {}", suite_path.display()))?;
    if suite.tasks.is_empty() {
        bail!("bench suite {} has no tasks", suite_path.display());
    }

    let run_dir = options.out_dir.unwrap_or_else(|| {
        repo_root
            .join("target/generated/code-agent-bench")
            .join(timestamp_id())
    });
    let run_dir = absolutize(&repo_root, run_dir);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    let artifact_cache_root = repo_root.join("target/generated/code-run-artifacts");
    fs::create_dir_all(&artifact_cache_root)
        .with_context(|| format!("create {}", artifact_cache_root.display()))?;

    let profile = absolutize(
        &repo_root,
        options
            .profile
            .unwrap_or_else(|| repo_root.join(DEFAULT_CODE_PROFILE)),
    );
    let model_config = absolutize(
        &repo_root,
        options
            .model_config
            .unwrap_or_else(|| repo_root.join(DEFAULT_MODEL_CONFIG)),
    );

    let selected = suite
        .tasks
        .iter()
        .filter(|task| options.task.as_ref().is_none_or(|id| id == &task.id))
        .take(options.limit.unwrap_or(usize::MAX))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        bail!("no benchmark tasks selected");
    }

    let mut task_runs = Vec::new();
    let context = BenchTaskContext {
        suite_dir: &suite_dir,
        run_dir: &run_dir,
        profile: &profile,
        model_config: &model_config,
        artifact_cache_root: &artifact_cache_root,
        repo_root: &repo_root,
        subagents: suite.subagents,
        skill: resolve_skill_run_metadata("code-agent").ok(),
        task_prefix: None,
        artifact_extra: BTreeMap::new(),
        log: options.log,
        keep_workdirs: options.keep_workdirs,
        refresh: options.refresh,
    };
    for task in selected {
        eprintln!("[air bench] running {}", task.id);
        let run = run_bench_task(&context, task)?;
        task_runs.push(run);
    }

    let passed = task_runs.iter().filter(|task| task.pass).count();
    let executed = task_runs
        .iter()
        .filter(|task| matches!(task.mode, CodeRunMode::Executed))
        .count();
    let replayed = task_runs
        .iter()
        .filter(|task| matches!(task.mode, CodeRunMode::Replayed))
        .count();
    let summary = BenchSummary {
        total: task_runs.len(),
        passed,
        failed: task_runs.len().saturating_sub(passed),
        executed,
        replayed,
    };
    let run = BenchRun {
        suite: suite.name,
        description: suite.description,
        run_dir: run_dir.display().to_string(),
        profile: profile.display().to_string(),
        model_config: model_config.display().to_string(),
        skill: None,
        runs: None,
        summary,
        tasks: task_runs,
    };
    let run_json = run_dir.join("run.json");
    fs::write(&run_json, serde_json::to_vec_pretty(&run)?)
        .with_context(|| format!("write {}", run_json.display()))?;
    println!("{}", serde_json::to_string_pretty(&run.summary)?);
    println!("run_json: {}", run_json.display());
    Ok(())
}

pub(crate) fn bench_skill(options: BenchSkillOptions) -> Result<()> {
    let repo_root = std::env::current_dir().context("resolve current directory")?;
    let suite_path = options
        .suite
        .unwrap_or_else(|| repo_root.join(DEFAULT_CODE_BENCH_SUITE));
    let suite_path = absolutize(&repo_root, suite_path);
    let suite_dir = suite_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("suite path has no parent: {}", suite_path.display()))?
        .to_path_buf();
    let suite: BenchSuite = serde_json::from_str(
        &fs::read_to_string(&suite_path)
            .with_context(|| format!("read bench suite {}", suite_path.display()))?,
    )
    .with_context(|| format!("parse bench suite {}", suite_path.display()))?;
    if suite.tasks.is_empty() {
        bail!("bench suite {} has no tasks", suite_path.display());
    }
    let selected = suite
        .tasks
        .iter()
        .filter(|task| options.task.as_ref().is_none_or(|id| id == &task.id))
        .take(options.limit.unwrap_or(usize::MAX))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        bail!("no benchmark tasks selected");
    }

    let run_dir = options.out_dir.unwrap_or_else(|| {
        repo_root
            .join("target/generated/skill-bench")
            .join(timestamp_id())
    });
    let run_dir = absolutize(&repo_root, run_dir);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    let artifact_cache_root = repo_root.join("target/generated/code-run-artifacts");
    fs::create_dir_all(&artifact_cache_root)
        .with_context(|| format!("create {}", artifact_cache_root.display()))?;
    let model_config = absolutize(
        &repo_root,
        options
            .model_config
            .unwrap_or_else(|| repo_root.join(DEFAULT_MODEL_CONFIG)),
    );
    let prepared = prepare_skill_run_context(&options.skill)?;

    let mut groups = Vec::new();
    if options.compare_no_skill {
        let profile = repo_root.join(DEFAULT_CODE_PROFILE);
        let group_dir = run_dir.join("no-skill");
        groups.push(run_bench_group(BenchGroupOptions {
            label: "no-skill",
            suite: &suite,
            selected: &selected,
            suite_dir: &suite_dir,
            run_dir: &group_dir,
            profile: &profile,
            model_config: &model_config,
            artifact_cache_root: &artifact_cache_root,
            repo_root: &repo_root,
            subagents: suite.subagents,
            skill: resolve_skill_run_metadata("code-agent").ok(),
            task_prefix: None,
            artifact_extra: BTreeMap::from([("bench_skill_group".to_string(), json!("no-skill"))]),
            log: options.log,
            keep_workdirs: options.keep_workdirs,
            refresh: options.refresh,
        })?);
    }
    let skill_group_dir = run_dir.join("skill");
    let mut skill_extra = prepared.artifact_extra.clone();
    skill_extra.insert("bench_skill_group".to_string(), json!("skill"));
    skill_extra.insert("bench_skill".to_string(), json!(options.skill));
    groups.push(run_bench_group(BenchGroupOptions {
        label: "skill",
        suite: &suite,
        selected: &selected,
        suite_dir: &suite_dir,
        run_dir: &skill_group_dir,
        profile: &prepared.profile,
        model_config: &model_config,
        artifact_cache_root: &artifact_cache_root,
        repo_root: &repo_root,
        subagents: suite.subagents,
        skill: Some(prepared.metadata),
        task_prefix: prepared.task_prefix.as_deref(),
        artifact_extra: skill_extra,
        log: options.log,
        keep_workdirs: options.keep_workdirs,
        refresh: options.refresh,
    })?);

    let summary = BenchSummary {
        total: groups.iter().map(|run| run.summary.total).sum(),
        passed: groups.iter().map(|run| run.summary.passed).sum(),
        failed: groups.iter().map(|run| run.summary.failed).sum(),
        executed: groups.iter().map(|run| run.summary.executed).sum(),
        replayed: groups.iter().map(|run| run.summary.replayed).sum(),
    };
    let run = BenchRun {
        suite: suite.name,
        description: suite.description,
        run_dir: run_dir.display().to_string(),
        profile: prepared.profile.display().to_string(),
        model_config: model_config.display().to_string(),
        skill: Some(options.skill),
        runs: Some(groups),
        summary,
        tasks: Vec::new(),
    };
    let run_json = run_dir.join("run.json");
    fs::write(&run_json, serde_json::to_vec_pretty(&run)?)
        .with_context(|| format!("write {}", run_json.display()))?;
    println!("{}", serde_json::to_string_pretty(&run.summary)?);
    println!("run_json: {}", run_json.display());
    Ok(())
}

struct BenchGroupOptions<'a> {
    label: &'a str,
    suite: &'a BenchSuite,
    selected: &'a [&'a BenchTask],
    suite_dir: &'a Path,
    run_dir: &'a Path,
    profile: &'a Path,
    model_config: &'a Path,
    artifact_cache_root: &'a Path,
    repo_root: &'a Path,
    subagents: bool,
    skill: Option<CodeRunSkill>,
    task_prefix: Option<&'a str>,
    artifact_extra: BTreeMap<String, Value>,
    log: bool,
    keep_workdirs: bool,
    refresh: bool,
}

fn run_bench_group(options: BenchGroupOptions<'_>) -> Result<BenchRun> {
    fs::create_dir_all(options.run_dir)
        .with_context(|| format!("create {}", options.run_dir.display()))?;
    let context = BenchTaskContext {
        suite_dir: options.suite_dir,
        run_dir: options.run_dir,
        profile: options.profile,
        model_config: options.model_config,
        artifact_cache_root: options.artifact_cache_root,
        repo_root: options.repo_root,
        subagents: options.subagents,
        skill: options.skill,
        task_prefix: options.task_prefix,
        artifact_extra: options.artifact_extra,
        log: options.log,
        keep_workdirs: options.keep_workdirs,
        refresh: options.refresh,
    };
    let mut task_runs = Vec::new();
    for task in options.selected {
        eprintln!(
            "[air bench:{label}] running {}",
            task.id,
            label = options.label
        );
        task_runs.push(run_bench_task(&context, task)?);
    }
    let summary = bench_summary(&task_runs);
    Ok(BenchRun {
        suite: format!("{}:{}", options.suite.name, options.label),
        description: options.suite.description.clone(),
        run_dir: options.run_dir.display().to_string(),
        profile: options.profile.display().to_string(),
        model_config: options.model_config.display().to_string(),
        skill: context.skill.as_ref().map(|skill| skill.id.clone()),
        runs: None,
        summary,
        tasks: task_runs,
    })
}

fn bench_summary(task_runs: &[TaskRun]) -> BenchSummary {
    let passed = task_runs.iter().filter(|task| task.pass).count();
    let executed = task_runs
        .iter()
        .filter(|task| matches!(task.mode, CodeRunMode::Executed))
        .count();
    let replayed = task_runs
        .iter()
        .filter(|task| matches!(task.mode, CodeRunMode::Replayed))
        .count();
    BenchSummary {
        total: task_runs.len(),
        passed,
        failed: task_runs.len().saturating_sub(passed),
        executed,
        replayed,
    }
}

fn run_bench_task(context: &BenchTaskContext<'_>, task: &BenchTask) -> Result<TaskRun> {
    let task_dir = context.run_dir.join("tasks").join(&task.id);
    let workdir = task_dir.join("work");
    let fixture = absolutize(context.suite_dir, context.suite_dir.join(&task.fixture));
    if workdir.exists() {
        fs::remove_dir_all(&workdir)
            .with_context(|| format!("remove old workdir {}", workdir.display()))?;
    }
    copy_dir(&fixture, &workdir)?;
    init_git_baseline(&workdir)?;

    let tool_config = task_dir.join("tools.json");
    fs::create_dir_all(&task_dir)?;
    let subagent_paths = if context.subagents {
        let explore_tool_config = task_dir.join("tools.explore.json");
        fs::write(&explore_tool_config, default_bench_explore_tool_config())?;
        Some(BenchSubagentPaths {
            repo_root: context.repo_root.to_path_buf(),
            explore_tool_config,
        })
    } else {
        None
    };
    fs::write(
        &tool_config,
        default_bench_tool_config(context.repo_root, subagent_paths.as_ref()),
    )?;
    let trace_path = task_dir.join("trace.jsonl");
    let output_path = task_dir.join("output.json");
    let before = WorkspaceSnapshot::capture(&workdir)?;
    let mut artifact_extra = BTreeMap::from([
        ("suite_task_id".to_string(), json!(task.id)),
        (
            "verification".to_string(),
            serde_json::to_value(&task.verification)?,
        ),
        ("diff".to_string(), serde_json::to_value(&task.diff)?),
    ]);
    artifact_extra.extend(context.artifact_extra.clone());
    let agent_task = if let Some(prefix) = context.task_prefix {
        format!("{prefix}\n\nTask: {}", task.prompt)
    } else {
        task.prompt.clone()
    };
    let descriptor = CodeRunDescriptor {
        task: task.prompt.clone(),
        skill: context.skill.clone(),
        profile: path_content_identity(context.profile)?,
        model_config: Some(path_content_identity(context.model_config)?),
        tool_config: Some(path_content_identity(&tool_config)?),
        extra: artifact_extra.clone(),
    };
    let fingerprint = descriptor.fingerprint(&before)?;
    let artifact_dir = context.artifact_cache_root.join(cache_key(&fingerprint));
    let mut mode = CodeRunMode::Executed;

    let mut error = None;
    if !context.refresh && artifact_dir.join("artifact.json").exists() {
        mode = CodeRunMode::Replayed;
        eprintln!("[air bench] replaying {} from {}", task.id, fingerprint);
        match replay_code_run_artifact(&artifact_dir, &workdir) {
            Ok(output) => {
                copy_cached_artifact_files(&artifact_dir, &task_dir, &trace_path)?;
                fs::write(&output_path, serde_json::to_vec_pretty(&output)?)
                    .with_context(|| format!("write {}", output_path.display()))?;
            }
            Err(err) => {
                let message = err.to_string();
                error = Some(message.clone());
                fs::write(
                    &output_path,
                    serde_json::to_vec_pretty(&json!({ "error": message }))?,
                )
                .with_context(|| format!("write {}", output_path.display()))?;
            }
        }
    } else {
        eprintln!("[air bench] executing {} as {}", task.id, fingerprint);
        let previous_dir = std::env::current_dir().context("resolve current directory")?;
        std::env::set_current_dir(&workdir)
            .with_context(|| format!("enter workdir {}", workdir.display()))?;
        let agent_result = run_code_agent(CodeOptions {
            task: agent_task,
            artifact_task: Some(task.prompt.clone()),
            skill: context.skill.clone(),
            profile: Some(context.profile.to_path_buf()),
            model_config: Some(context.model_config.to_path_buf()),
            trace_out: Some(trace_path.clone()),
            trace_redact: false,
            trace_raw: true,
            log: context.log,
            tool_config: Some(tool_config.clone()),
            artifact_out: Some(artifact_dir.clone()),
            artifact_extra,
            replay_artifact: None,
            replay_from: None,
        });
        let restore_result = std::env::set_current_dir(&previous_dir)
            .with_context(|| format!("restore cwd {}", previous_dir.display()));
        restore_result?;

        match agent_result {
            Ok(output) => {
                fs::write(&output_path, serde_json::to_vec_pretty(&output)?)
                    .with_context(|| format!("write {}", output_path.display()))?;
            }
            Err(err) => {
                let message = err.to_string();
                error = Some(message.clone());
                fs::write(
                    &output_path,
                    serde_json::to_vec_pretty(&json!({ "error": message }))?,
                )
                .with_context(|| format!("write {}", output_path.display()))?;
            }
        }
    }
    if !artifact_dir.join("artifact.json").exists() && trace_path.exists() && output_path.exists() {
        let _ = fs::create_dir_all(&artifact_dir);
        let _ = fs::copy(&trace_path, artifact_dir.join("trace.jsonl"));
        let _ = fs::copy(&output_path, artifact_dir.join("output.json"));
    }

    if output_path.exists() && !task_dir.join("artifact").exists() {
        let _ = fs::create_dir_all(task_dir.join("artifact"));
        if artifact_dir.exists() {
            let _ = copy_dir(&artifact_dir, &task_dir.join("artifact"));
        }
    }

    let verification = task
        .verification
        .iter()
        .map(|command| run_verification_command(&workdir, command))
        .collect::<Result<Vec<_>>>()?;
    let changed_files = git_changed_files(&workdir)?;
    let diff = git_diff(&workdir)?;
    let diff_constraints = check_diff_constraints(&task.diff, &changed_files, &diff);
    let metrics = trace_metrics(&trace_path).unwrap_or_default();
    let verification_passed = verification.iter().all(|result| result.success);
    let pass = error.is_none() && verification_passed && diff_constraints.passed;
    let failure_reason = bench_failure_reason(
        error.as_deref(),
        &verification,
        &diff_constraints,
        &changed_files,
    );
    let model_calls_spent = if matches!(mode, CodeRunMode::Replayed) {
        0
    } else {
        metrics.model_calls
    };

    if !context.keep_workdirs && pass {
        let _ = fs::remove_dir_all(&workdir);
    }

    Ok(TaskRun {
        id: task.id.clone(),
        mode,
        pass,
        fingerprint: fingerprint.clone(),
        artifact_path: artifact_dir.display().to_string(),
        workdir: workdir.display().to_string(),
        trace_path: trace_path.display().to_string(),
        output_path: output_path.display().to_string(),
        changed_files,
        metrics,
        verification,
        diff_constraints,
        model_calls_spent,
        failure_reason,
        error,
    })
}

fn run_verification_command(
    workdir: &Path,
    command: &VerificationCommand,
) -> Result<VerificationResult> {
    let output = Command::new("sh")
        .args(["-lc", &command.command])
        .current_dir(workdir)
        .output()
        .with_context(|| format!("run verification `{}`", command.command))?;
    Ok(VerificationResult {
        command: command.command.clone(),
        description: command.description.clone(),
        success: output.status.success(),
        status: output.status.code(),
        stdout_preview: preview_bytes(&output.stdout, 4000),
        stderr_preview: preview_bytes(&output.stderr, 4000),
    })
}

fn check_diff_constraints(
    constraints: &DiffConstraints,
    changed_files: &[String],
    diff: &str,
) -> ConstraintResult {
    let mut result = ConstraintResult {
        passed: true,
        violations: Vec::new(),
    };
    let changed = changed_files.iter().cloned().collect::<BTreeSet<_>>();
    if !constraints.allowed_changed_files.is_empty() {
        let allowed = constraints
            .allowed_changed_files
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for path in &changed {
            if !allowed.contains(path) {
                result
                    .violations
                    .push(format!("unexpected changed file: {path}"));
            }
        }
    }
    for path in &constraints.required_changed_files {
        if !changed.contains(path) {
            result
                .violations
                .push(format!("required changed file missing: {path}"));
        }
    }
    for needle in &constraints.required_diff_contains {
        if !diff.contains(needle) {
            result
                .violations
                .push(format!("diff does not contain required text: {needle}"));
        }
    }
    result.passed = result.violations.is_empty();
    result
}

fn bench_failure_reason(
    agent_error: Option<&str>,
    verification: &[VerificationResult],
    diff_constraints: &ConstraintResult,
    changed_files: &[String],
) -> Option<FailureReason> {
    if let Some(error) = agent_error {
        return Some(FailureReason {
            category: classify_agent_error(error),
            message: error.to_string(),
            details: BTreeMap::new(),
        });
    }
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
    if !diff_constraints.passed {
        return Some(FailureReason {
            category: FailureCategory::DiffConstraintFailed,
            message: "workspace diff did not satisfy benchmark constraints".to_string(),
            details: BTreeMap::from([(
                "violations".to_string(),
                json!(diff_constraints.violations),
            )]),
        });
    }
    if changed_files.is_empty() {
        return Some(FailureReason {
            category: FailureCategory::NoPatchApplied,
            message: "no workspace files changed".to_string(),
            details: BTreeMap::new(),
        });
    }
    None
}

fn classify_agent_error(error: &str) -> FailureCategory {
    let lower = error.to_ascii_lowercase();
    if lower.contains("429") || lower.contains("rate limit") {
        FailureCategory::ProviderRateLimited
    } else if lower.contains("timeout") || lower.contains("timed out") {
        FailureCategory::Timeout
    } else if lower.contains("policy") || lower.contains("approval") {
        FailureCategory::PolicyViolation
    } else if lower.contains("replay") || lower.contains("snapshot") {
        FailureCategory::ReplayMismatch
    } else {
        FailureCategory::AgentError
    }
}

fn copy_cached_artifact_files(
    artifact_dir: &Path,
    task_dir: &Path,
    trace_path: &Path,
) -> Result<()> {
    let artifact = read_code_run_artifact(artifact_dir)?;
    let cached_trace = artifact_dir.join(&artifact.files.trace);
    if cached_trace.exists() {
        fs::copy(&cached_trace, trace_path).with_context(|| {
            format!(
                "copy {} to {}",
                cached_trace.display(),
                trace_path.display()
            )
        })?;
    }
    let task_artifact_dir = task_dir.join("artifact");
    if task_artifact_dir.exists() {
        fs::remove_dir_all(&task_artifact_dir)
            .with_context(|| format!("remove {}", task_artifact_dir.display()))?;
    }
    copy_dir(artifact_dir, &task_artifact_dir)
}

fn cache_key(fingerprint: &str) -> String {
    fingerprint
        .strip_prefix("sha256:")
        .unwrap_or(fingerprint)
        .to_string()
}

fn trace_metrics(path: &Path) -> Result<TraceMetrics> {
    trace_metrics_inner(path, &mut BTreeSet::new())
}

fn trace_metrics_inner(path: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<TraceMetrics> {
    if !path.exists() {
        return Ok(TraceMetrics::default());
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical) {
        return Ok(TraceMetrics::default());
    }
    let events = read_trace_jsonl(path)
        .map_err(|error| anyhow::anyhow!("read trace {}: {error}", path.display()))?;
    let mut metrics = TraceMetrics::default();
    let mut read_keys = BTreeSet::new();
    for event in events {
        match event.action.as_str() {
            "model_call" => {
                if event.status == air_runtime::TraceStatus::Ok {
                    metrics.model_calls += 1;
                } else {
                    metrics.model_errors += 1;
                }
            }
            "tool_batch_dispatch_item" => {
                metrics.tool_calls += 1;
                if event.status != air_runtime::TraceStatus::Ok {
                    metrics.tool_errors += 1;
                }
                let tool = event
                    .meta
                    .as_ref()
                    .and_then(Value::as_object)
                    .and_then(|meta| meta.get("tool"))
                    .or_else(|| {
                        event
                            .output
                            .as_ref()
                            .and_then(|output| output.pointer("/output/tool"))
                    })
                    .or_else(|| {
                        event
                            .input
                            .as_ref()
                            .and_then(|input| input.get("tool").or_else(|| input.get("name")))
                    })
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                *metrics.tool_counts.entry(tool.clone()).or_insert(0) += 1;
                if tool == "task" {
                    metrics.subagent_calls += 1;
                    if event.status != air_runtime::TraceStatus::Ok {
                        metrics.subagent_errors += 1;
                    }
                    if let Some(child_trace) = event
                        .output
                        .as_ref()
                        .and_then(|output| output.get("child_trace_path"))
                        .and_then(Value::as_str)
                    {
                        let child = trace_metrics_inner(Path::new(child_trace), visited)?;
                        metrics.subagent_model_calls += child.model_calls;
                        metrics.subagent_tool_calls += child.tool_calls;
                        metrics.subagent_tool_errors += child.tool_errors;
                    }
                }
                if matches!(tool.as_str(), "edit" | "write" | "apply_patch")
                    && metrics.first_edit_tool_index.is_none()
                {
                    metrics.first_edit_tool_index = Some(metrics.tool_calls);
                }
                if matches!(tool.as_str(), "read" | "read_range" | "read_contains") {
                    let key = event
                        .input
                        .as_ref()
                        .map(read_key)
                        .unwrap_or_else(|| "<missing-input>".to_string());
                    if !read_keys.insert(key) {
                        metrics.repeated_reads += 1;
                    }
                }
                if event
                    .output
                    .as_ref()
                    .is_some_and(|output| is_verification_event(output, &tool))
                {
                    metrics.verification_tool_calls += 1;
                    if !event.output.as_ref().is_some_and(tool_output_success) {
                        metrics.verification_failures += 1;
                    }
                }
            }
            _ => {}
        }
    }
    metrics.repair_iterations = metrics.verification_failures;
    Ok(metrics)
}

fn read_key(input: &Value) -> String {
    let path = input
        .get("filePath")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let offset = input
        .get("offset")
        .or_else(|| input.get("start_line"))
        .or_else(|| input.get("startLine"))
        .map(Value::to_string)
        .unwrap_or_default();
    let limit = input
        .get("limit")
        .or_else(|| input.get("max_bytes"))
        .or_else(|| input.get("maxBytes"))
        .map(Value::to_string)
        .unwrap_or_default();
    format!("{path}:{offset}:{limit}")
}

fn is_verification_event(output: &Value, tool: &str) -> bool {
    if tool == "test" {
        return true;
    }
    output
        .get("verification")
        .or_else(|| output.pointer("/output/verification"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn tool_output_success(output: &Value) -> bool {
    output
        .get("success")
        .or_else(|| output.pointer("/output/success"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_ignored_changed_file(path: &str) -> bool {
    path == ".air" || path.starts_with(".air/") || path == "target" || path.starts_with("target/")
}

fn ensure_bench_gitignore(workdir: &Path) -> Result<()> {
    let gitignore = workdir.join(".gitignore");
    let mut lines = if gitignore.exists() {
        fs::read_to_string(&gitignore)
            .with_context(|| format!("read {}", gitignore.display()))?
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for entry in [".air/", "target/"] {
        if !lines.iter().any(|line| line.trim() == entry) {
            lines.push(entry.to_string());
        }
    }
    let mut content = lines.join("\n");
    content.push('\n');
    fs::write(&gitignore, content).with_context(|| format!("write {}", gitignore.display()))
}

struct BenchSubagentPaths {
    repo_root: PathBuf,
    explore_tool_config: PathBuf,
}

fn default_bench_tool_config(
    repo_root: &Path,
    subagent_paths: Option<&BenchSubagentPaths>,
) -> String {
    let task_tool = if let Some(paths) = subagent_paths {
        json!({
            "kind": "subagent",
            "capability": "code.read",
            "subagents": {
                "explore": {
                    "cwd": ".",
                    "command": [
                        "{air_exe}",
                        "run-plan",
                        "--profile",
                        paths.repo_root.join("skills/code-agent/explore.air-profile.yaml"),
                        "--input",
                        "{input_file}",
                        "--model-config",
                        "{env:AIR_CODE_MODEL_CONFIG}",
                        "--tool-config",
                        paths.explore_tool_config,
                        "--trace-out",
                        "{trace_file}"
                    ],
                    "timeout_seconds": 600,
                    "max_bytes": 51200,
                    "truncation_direction": "tail"
                }
            }
        })
    } else {
        json!({"kind": "local_reflection", "capability": "code.read"})
    };
    serde_json::to_string_pretty(&json!({
        "workspace_dir": ".",
        "tools": {
            "question": {"kind": "local_reflection", "capability": "code.read"},
            "read_range": {
                "kind": "file_read",
                "capability": "file.read",
                "base_dir": ".",
                "max_bytes": 51200
            },
            "read_contains": {
                "kind": "file_read",
                "capability": "file.read",
                "base_dir": ".",
                "max_bytes": 51200
            },
            "glob": {
                "kind": "repo_files",
                "capability": "code.read",
                "repo_dir": ".",
                "max_files": 100
            },
            "grep": {
                "kind": "file_search",
                "capability": "file.read",
                "base_dir": ".",
                "max_matches": 100,
                "max_context_lines": 2,
                "max_line_chars": 2000,
                "max_bytes": 51200
            },
            "edit": {
                "kind": "file_edit",
                "capability": "file.write",
                "base_dir": ".",
                "max_bytes": 262144
            },
            "write": {
                "kind": "file_write",
                "capability": "file.write",
                "base_dir": ".",
                "max_bytes": 262144,
                "create_dirs": true,
                "allow_overwrite": true
            },
            "apply_patch": {
                "kind": "apply_patch",
                "capability": "file.write",
                "base_dir": ".",
                "max_bytes": 262144
            },
            "bash": {
                "kind": "bash",
                "capability": "code.test",
                "cwd": ".",
                "timeout_seconds": 120,
                "max_bytes": 51200,
                "truncation_direction": "tail"
            },
            "task": task_tool,
            "webfetch": {
                "kind": "web_fetch",
                "capability": "code.read",
                "timeout_seconds": 120,
                "max_bytes": 51200
            },
            "todowrite": {"kind": "todo_write", "capability": "code.read"},
            "todoread": {"kind": "local_reflection", "capability": "code.read"},
            "skill": {
                "kind": "skill",
                "capability": "code.read",
                "root_dir": repo_root,
                "max_bytes": 65536
            }
        },
        "approvals": {
            "file.write": {
                "approved": true,
                "approver": "code-agent-bench",
                "reason": "benchmark workspace is isolated"
            }
        }
    }))
    .expect("bench tool config is serializable")
}

fn default_bench_explore_tool_config() -> String {
    serde_json::to_string_pretty(&json!({
        "workspace_dir": ".",
        "tools": {
            "question": {"kind": "local_reflection", "capability": "code.read"},
            "read_range": {
                "kind": "file_read",
                "capability": "file.read",
                "base_dir": ".",
                "max_bytes": 51200
            },
            "read_contains": {
                "kind": "file_read",
                "capability": "file.read",
                "base_dir": ".",
                "max_bytes": 51200
            },
            "glob": {
                "kind": "repo_files",
                "capability": "code.read",
                "repo_dir": ".",
                "max_files": 100
            },
            "grep": {
                "kind": "file_search",
                "capability": "file.read",
                "base_dir": ".",
                "max_matches": 100,
                "max_context_lines": 2,
                "max_line_chars": 2000,
                "max_bytes": 51200
            },
            "webfetch": {
                "kind": "web_fetch",
                "capability": "code.read",
                "timeout_seconds": 120,
                "max_bytes": 51200
            },
            "skill": {"kind": "local_reflection", "capability": "code.read"}
        }
    }))
    .expect("bench explore tool config is serializable")
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".git" | "target" | ".air") {
            continue;
        }
        let target = destination.join(name.as_ref());
        if file_type.is_dir() {
            copy_dir(&path, &target)?;
        } else if file_type.is_file() {
            fs::copy(&path, &target)
                .with_context(|| format!("copy {} to {}", path.display(), target.display()))?;
        }
    }
    Ok(())
}

fn init_git_baseline(workdir: &Path) -> Result<()> {
    ensure_bench_gitignore(workdir)?;
    run_quiet(workdir, "git init -q")?;
    run_quiet(workdir, "git config user.email air-bench@example.invalid")?;
    run_quiet(workdir, "git config user.name 'AIR Bench'")?;
    run_quiet(workdir, "git add -A")?;
    run_quiet(
        workdir,
        "git -c commit.gpgsign=false commit --allow-empty --no-verify -m baseline >/dev/null",
    )
}

fn run_quiet(workdir: &Path, command: &str) -> Result<()> {
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

fn git_changed_files(workdir: &Path) -> Result<Vec<String>> {
    let output = Command::new("sh")
        .args([
            "-lc",
            "{ git diff --name-only --; git ls-files --others --exclude-standard; } | sort -u",
        ])
        .current_dir(workdir)
        .output()
        .context("list changed files")?;
    if !output.status.success() {
        bail!("git changed files failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !is_ignored_changed_file(line))
        .map(str::to_string)
        .collect())
}

fn git_diff(workdir: &Path) -> Result<String> {
    let output = Command::new("sh")
        .args([
            "-lc",
            "git diff -- && git ls-files --others --exclude-standard | while IFS= read -r file; do git diff --no-index -- /dev/null \"$file\" || true; done",
        ])
        .current_dir(workdir)
        .output()
        .context("render git diff")?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
        .unwrap_or(0);
    millis.to_string()
}

fn preview_bytes(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut preview = text.chars().take(max_chars).collect::<String>();
    preview.push_str("\n[AIR_BENCH_TRUNCATED]");
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_constraints_reject_unexpected_files() {
        let constraints = DiffConstraints {
            allowed_changed_files: vec!["src/lib.rs".to_string()],
            required_changed_files: vec!["src/lib.rs".to_string()],
            required_diff_contains: vec!["helper".to_string()],
        };
        let result = check_diff_constraints(
            &constraints,
            &["src/lib.rs".to_string(), "README.md".to_string()],
            "fn helper() {}",
        );

        assert!(!result.passed);
        assert!(result
            .violations
            .iter()
            .any(|violation| violation.contains("README.md")));
    }

    #[test]
    fn read_key_uses_path_and_range() {
        assert_eq!(
            read_key(&json!({"filePath": "src/lib.rs", "offset": 10, "limit": 20})),
            "src/lib.rs:10:20"
        );
    }

    #[test]
    fn subagent_smoke_suite_enables_subagents() {
        let suite: BenchSuite = serde_json::from_str(include_str!(
            "../../../skills/code-agent/benches/rust-subagent-smoke/suite.json"
        ))
        .unwrap();

        assert!(suite.subagents);
        assert_eq!(suite.tasks.len(), 2);
    }

    #[test]
    fn subagent_bench_tool_config_records_child_trace() {
        let paths = BenchSubagentPaths {
            repo_root: PathBuf::from("/repo"),
            explore_tool_config: PathBuf::from("/run/tools.explore.json"),
        };
        let config: Value =
            serde_json::from_str(&default_bench_tool_config(Path::new("/repo"), Some(&paths)))
                .unwrap();
        let command = config
            .pointer("/tools/task/subagents/explore/command")
            .and_then(Value::as_array)
            .unwrap();

        assert!(command.iter().any(|value| value == "--trace-out"));
        assert!(command.iter().any(|value| value == "{trace_file}"));
    }
}
