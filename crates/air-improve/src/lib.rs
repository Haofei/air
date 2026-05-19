use air_advisory::{
    cached_llm_advisory, is_disabled as llm_advisory_disabled, CachedLlmAdvisoryOptions,
};
use air_eval::check_default_eval_manifest_if_present;
use air_regression::{run_regression_suite, RegressionRunConfig, RegressionRunReport};
use air_runtime::{read_trace_jsonl, ModelProvider};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const IMPROVE_OBSERVATIONS_SCHEMA: &str = air_schemas::IMPROVE_OBSERVATIONS;
const IMPROVE_FINDINGS_SCHEMA: &str = air_schemas::IMPROVE_FINDINGS;
const IMPROVE_REGRESSIONS_SCHEMA: &str = air_schemas::IMPROVE_SUGGESTED_REGRESSIONS;
const DEFAULT_CHECK_SUITE: &str = "skills/code-agent/benches/rust-small/suite.json";
const FAILURE_CLASSIFIER_MODEL: &str = "failure_classifier";

pub struct ImproveOptions {
    pub action: ImproveAction,
    pub from: Vec<PathBuf>,
    pub out_dir: Option<PathBuf>,
    pub write_regressions: bool,
    pub emit_stdout: bool,
    /// Optional pre-built provider for LLM-advisory failure classification.
    /// `None` means deterministic-only — observations stay in their
    /// regex-inferred categories.
    pub advisory_provider: Option<Box<dyn ModelProvider>>,
}

pub enum ImproveAction {
    Report,
    Next,
    Check {
        finding: String,
    },
    Promote {
        finding: String,
    },
    Fix {
        finding: Option<String>,
    },
    Evaluate {
        finding: String,
        report: Option<PathBuf>,
        regression_file: Option<PathBuf>,
    },
}

#[derive(Debug, Serialize)]
struct ImproveOutput {
    schema: &'static str,
    out_dir: String,
    observations: usize,
    findings: usize,
    report: String,
    observations_file: String,
    findings_file: String,
    suggested_regressions_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    regression_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_action: Option<ImproveNextAction>,
    failure_classifier_status: String,
}

#[derive(Debug, Clone, Serialize)]
struct ImproveNextAction {
    finding: String,
    category: String,
    status: String,
    command: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct ImproveCheckOutput {
    schema: &'static str,
    finding: String,
    status: String,
    command: Vec<String>,
    check_run_dir: String,
    report: String,
    failed_tasks: usize,
    message: String,
}

#[derive(Debug, Serialize)]
struct ImprovePromoteOutput {
    schema: &'static str,
    finding: String,
    status: String,
    regression_file: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct ImproveFixOutput {
    schema: &'static str,
    status: String,
    finding: Option<String>,
    message: String,
    next_command: Option<String>,
}

#[derive(Debug, Serialize)]
struct ImproveEvaluateOutput {
    schema: &'static str,
    finding: String,
    decision: String,
    regression_gate: RegressionGateSummary,
    regression: RegressionRunReport,
    tests: ImproveTestGate,
    guard: ImproveGuardGate,
    score_delta: i32,
    eval_json: String,
    eval_report: String,
}

#[derive(Debug, Serialize)]
struct RegressionGateSummary {
    causal_pass: bool,
    unresolved_stale: usize,
    unsupported: usize,
    statuses: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ImproveTestGate {
    command: Vec<String>,
    passed: bool,
    status: Option<i32>,
}

#[derive(Debug, Serialize)]
struct ImproveGuardGate {
    passed: bool,
    warnings: Vec<String>,
    blocked: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImproveFindingStatus {
    schema: String,
    finding: String,
    status: String,
    message: String,
    updated_at_unix: u64,
}

#[derive(Debug, Serialize)]
struct ImproveObservationsFile {
    schema: &'static str,
    generated_at_unix: u64,
    roots: Vec<String>,
    observations: Vec<ImproveObservation>,
}

#[derive(Debug, Clone, Serialize)]
struct ImproveObservation {
    id: String,
    source_kind: String,
    source_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suite: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suite_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<String>,
    category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effective_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    advisory_category: Option<ImproveAdvisoryCategory>,
    message: String,
    final_success: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImproveAdvisoryCategory {
    model: String,
    category: String,
    confidence: f64,
    rationale: String,
    evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ImproveFindingsFile {
    schema: &'static str,
    generated_at_unix: u64,
    findings: Vec<ImproveFinding>,
}

#[derive(Debug, Clone, Serialize)]
struct ImproveFinding {
    id: String,
    category: String,
    summary: String,
    count: usize,
    impact_score: i32,
    priority_reason: String,
    recommended_change_type: String,
    expected_regression: String,
    evidence: Vec<String>,
    observation_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SuggestedRegressionsFile {
    schema: &'static str,
    generated_at_unix: u64,
    regressions: Vec<SuggestedRegression>,
}

#[derive(Debug, Clone, Serialize)]
struct SuggestedRegression {
    finding_id: String,
    kind: String,
    category: String,
    title: String,
    source_observations: Vec<String>,
    fixture_hint: Value,
}

#[derive(Debug, Serialize)]
struct FailureClassifierRequest {
    observation_id: String,
    source_kind: String,
    source_path: String,
    task_id: Option<String>,
    task: Option<String>,
    deterministic_category: String,
    message: String,
    evidence: Vec<String>,
    changed_files: Vec<String>,
    allowed_categories: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct FailureClassifierSummary {
    observation_id: String,
    deterministic_category: String,
    advisory_category: String,
    confidence: f64,
    rationale: String,
    evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct FailureClassifierError {
    observation_id: String,
    deterministic_category: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct FailureClassifierRun {
    status: String,
    summaries: Vec<FailureClassifierSummary>,
    errors: Vec<FailureClassifierError>,
}

#[derive(Debug, Deserialize)]
struct FailureClassifierOutput {
    category: String,
    rationale: String,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    confidence: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BenchRunLite {
    suite: Option<String>,
    suite_path: Option<String>,
    #[serde(default)]
    tasks: Vec<BenchTaskRunLite>,
    #[serde(default)]
    runs: Option<Vec<BenchRunLite>>,
}

#[derive(Debug, Deserialize)]
struct BenchTaskRunLite {
    id: Option<String>,
    pass: Option<bool>,
    artifact_path: Option<String>,
    changed_files: Option<Vec<String>>,
    failure_reason: Option<Value>,
    verdict: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BenchSuiteProbe {
    name: String,
    tasks: Vec<BenchSuiteTaskProbe>,
}

#[derive(Debug, Deserialize)]
struct BenchSuiteTaskProbe {
    id: String,
}

pub fn run_improve(mut options: ImproveOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let roots = if options.from.is_empty() {
        vec![cwd.join("target/generated")]
    } else {
        options
            .from
            .into_iter()
            .map(|path| absolutize(&cwd, path))
            .collect()
    };
    let out_dir = absolutize(
        &cwd,
        options
            .out_dir
            .unwrap_or_else(|| cwd.join(".air/improve/latest")),
    );
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let mut files = Vec::new();
    for root in &roots {
        collect_improve_inputs(root, &mut files)?;
    }
    files.sort();
    files.dedup();

    let mut observations = Vec::new();
    let mut seen = BTreeSet::new();
    for file in files {
        let file_name = file.file_name().and_then(|name| name.to_str());
        match file_name {
            Some("run.json") => collect_bench_run(&file, &mut observations, &mut seen)?,
            Some("artifact.json") => collect_code_artifact(&file, &mut observations, &mut seen)?,
            Some("trace.jsonl") => collect_trace_hygiene(&file, &mut observations, &mut seen)?,
            _ => {}
        }
    }
    renumber_observations(&mut observations);
    let advisory =
        apply_failure_classifier_advisory(&mut options.advisory_provider, &mut observations);

    let findings = mine_findings(&observations);
    let regressions = suggest_regressions(&findings);
    let statuses = load_finding_statuses(&out_dir)?;
    if options.write_regressions {
        write_regression_files(&out_dir.join("suggested-regressions"), &regressions)?;
    }

    let generated_at_unix = unix_now();
    let observations_path = out_dir.join("observations.json");
    let findings_path = out_dir.join("findings.json");
    let regressions_path = out_dir.join("suggested_regressions.json");
    let report_path = out_dir.join("report.md");

    write_json(
        &observations_path,
        &ImproveObservationsFile {
            schema: IMPROVE_OBSERVATIONS_SCHEMA,
            generated_at_unix,
            roots: roots
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            observations: observations.clone(),
        },
    )?;
    if !advisory.summaries.is_empty()
        || !advisory.errors.is_empty()
        || advisory.status != "not_needed"
    {
        write_json(
            &out_dir.join("failure_classifier.json"),
            &json!({
                "schema": "air.improve_failure_classifier.v1",
                "model": FAILURE_CLASSIFIER_MODEL,
                "status": advisory.status,
                "categories": advisory.summaries,
                "errors": advisory.errors,
            }),
        )?;
    }
    write_json(
        &findings_path,
        &ImproveFindingsFile {
            schema: IMPROVE_FINDINGS_SCHEMA,
            generated_at_unix,
            findings: findings.clone(),
        },
    )?;
    write_json(
        &regressions_path,
        &SuggestedRegressionsFile {
            schema: IMPROVE_REGRESSIONS_SCHEMA,
            generated_at_unix,
            regressions: regressions.clone(),
        },
    )?;
    write_report(
        &report_path,
        &roots,
        &observations,
        &findings,
        &regressions,
        &statuses,
    )?;

    match &options.action {
        ImproveAction::Check { finding } => {
            return check_finding(finding, &out_dir, &roots, &findings, &observations);
        }
        ImproveAction::Promote { finding } => {
            return promote_finding(finding, &out_dir, &regressions);
        }
        ImproveAction::Fix { finding } => {
            return prepare_fix(finding.as_deref(), &findings, &statuses);
        }
        ImproveAction::Evaluate {
            finding,
            report,
            regression_file,
        } => {
            return evaluate_finding(
                finding,
                report.as_deref(),
                regression_file.as_deref(),
                &out_dir,
                &roots,
            );
        }
        ImproveAction::Next => {
            let next_action = choose_next_action(&findings, &observations, &statuses);
            if options.emit_stdout {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "schema": "air.improve_next.v1",
                        "report": report_path.display().to_string(),
                        "next_action": next_action,
                    }))?
                );
            }
            return Ok(());
        }
        ImproveAction::Report => {}
    }

    let output = ImproveOutput {
        schema: "air.improve_run.v1",
        out_dir: out_dir.display().to_string(),
        observations: observations.len(),
        findings: findings.len(),
        report: report_path.display().to_string(),
        observations_file: observations_path.display().to_string(),
        findings_file: findings_path.display().to_string(),
        suggested_regressions_file: regressions_path.display().to_string(),
        regression_dir: options
            .write_regressions
            .then(|| out_dir.join("suggested-regressions").display().to_string()),
        next_action: choose_next_action(&findings, &observations, &statuses),
        failure_classifier_status: advisory.status,
    };
    if options.emit_stdout {
        println!("{}", serde_json::to_string_pretty(&output)?);
    }
    Ok(())
}

fn collect_improve_inputs(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if is_improve_input(root) {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry under {}", root.display()))?;
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if path.is_dir() {
            if matches!(
                file_name,
                ".git" | "node_modules" | ".venv" | "target" | ".cache"
            ) && root.file_name().and_then(|name| name.to_str()) != Some("generated")
            {
                continue;
            }
            collect_improve_inputs(&path, files)?;
        } else if is_improve_input(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_improve_input(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("run.json" | "artifact.json") => true,
        Some("trace.jsonl") => trace_without_sibling_artifact(path),
        _ => false,
    }
}

fn trace_without_sibling_artifact(path: &Path) -> bool {
    path.parent()
        .map(|parent| !parent.join("artifact.json").exists())
        .unwrap_or(true)
}

fn collect_suite_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if root.file_name().and_then(|name| name.to_str()) == Some("suite.json") {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry under {}", root.display()))?;
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if path.is_dir() {
            if matches!(
                file_name,
                ".git" | "node_modules" | ".venv" | "target" | ".cache"
            ) && root.file_name().and_then(|name| name.to_str()) != Some("generated")
            {
                continue;
            }
            collect_suite_files(&path, files)?;
        } else if file_name == "suite.json" {
            files.push(path);
        }
    }
    Ok(())
}

fn collect_bench_run(
    path: &Path,
    observations: &mut Vec<ImproveObservation>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let run: BenchRunLite =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    collect_bench_run_inner(path, &run, run.suite_path.as_deref(), observations, seen)
}

fn collect_bench_run_inner(
    path: &Path,
    run: &BenchRunLite,
    inherited_suite_path: Option<&str>,
    observations: &mut Vec<ImproveObservation>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    let suite_path = run.suite_path.as_deref().or(inherited_suite_path);
    for task in &run.tasks {
        if !bench_task_failed(task) {
            continue;
        }
        let source_path = path.display().to_string();
        let task_id = task.id.clone();
        let artifact_path = task.artifact_path.clone();
        let key = format!(
            "bench:{}:{}:{}",
            source_path,
            run.suite.as_deref().unwrap_or_default(),
            task_id
                .as_deref()
                .or(artifact_path.as_deref())
                .unwrap_or("")
        );
        if !seen.insert(key) {
            continue;
        }
        let (category, message) = failure_category_and_message(
            task.failure_reason.as_ref(),
            task.verdict.as_ref(),
            task.error.as_deref(),
        );
        let mut evidence = Vec::new();
        if let Some(error) = &task.error {
            evidence.push(format!("benchmark error: {}", truncate(error, 180)));
        }
        if let Some(artifact) = &artifact_path {
            evidence.push(format!("artifact: {artifact}"));
        }
        let changed_files = compact_changed_files(
            task.changed_files.clone().unwrap_or_default(),
            &mut evidence,
        );
        observations.push(ImproveObservation {
            id: String::new(),
            source_kind: "bench_task".to_string(),
            source_path,
            artifact_path,
            suite: run.suite.clone(),
            suite_path: suite_path.map(ToOwned::to_owned),
            task_id,
            task: None,
            category,
            category_source: None,
            effective_category: None,
            advisory_category: None,
            message,
            final_success: task
                .verdict
                .as_ref()
                .and_then(|verdict| verdict.get("final_success"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            changed_files,
            skills: Vec::new(),
            evidence,
        });
    }
    if let Some(groups) = &run.runs {
        for group in groups {
            collect_bench_run_inner(
                path,
                group,
                group.suite_path.as_deref().or(suite_path),
                observations,
                seen,
            )?;
        }
    }
    Ok(())
}

fn collect_code_artifact(
    path: &Path,
    observations: &mut Vec<ImproveObservation>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let artifact: Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    if !artifact_failed(&artifact) {
        return Ok(());
    }
    let source_path = path.display().to_string();
    let key = format!("artifact:{source_path}");
    if !seen.insert(key) {
        return Ok(());
    }
    let verdict = artifact.get("verdict");
    let failure = artifact.get("failure_reason");
    let (category, message) = failure_category_and_message(failure, verdict, None);
    let changed_files = artifact
        .pointer("/delta/changed_files")
        .and_then(Value::as_array)
        .map(|values| string_array(values))
        .unwrap_or_default();
    let mut skills = Vec::new();
    if let Some(skill) = artifact
        .get("skill")
        .and_then(|skill| skill.get("id"))
        .and_then(Value::as_str)
    {
        skills.push(skill.to_string());
    }
    let mut evidence = Vec::new();
    if let Some(trace) = artifact
        .pointer("/files/trace")
        .and_then(Value::as_str)
        .filter(|trace| !trace.is_empty())
    {
        evidence.push(format!(
            "trace: {}",
            path.parent()
                .unwrap_or(Path::new("."))
                .join(trace)
                .display()
        ));
    }
    let changed_files = compact_changed_files(changed_files, &mut evidence);
    observations.push(ImproveObservation {
        id: String::new(),
        source_kind: "code_run_artifact".to_string(),
        source_path,
        artifact_path: Some(path.parent().unwrap_or(path).display().to_string()),
        suite: None,
        suite_path: None,
        task_id: None,
        task: artifact
            .get("task")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        category,
        category_source: None,
        effective_category: None,
        advisory_category: None,
        message,
        final_success: verdict
            .and_then(|verdict| verdict.get("final_success"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        changed_files,
        skills,
        evidence,
    });
    Ok(())
}

fn collect_trace_hygiene(
    path: &Path,
    observations: &mut Vec<ImproveObservation>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    let source_path = path.display().to_string();
    let events = read_trace_jsonl(path)
        .map_err(|error| anyhow::anyhow!("read trace {}: {error}", path.display()))?;
    if events.is_empty() {
        push_trace_observation(
            observations,
            seen,
            &source_path,
            "aborted_trace_without_artifact",
            "trace file is empty and no artifact verdict was produced",
            vec!["empty trace".to_string()],
        );
        return Ok(());
    }

    let has_return = events.iter().any(|event| event.action == "return");
    if !has_return {
        push_trace_observation(
            observations,
            seen,
            &source_path,
            "aborted_trace_without_artifact",
            "trace ended without a return event or artifact verdict",
            vec![
                format!("trace events: {}", events.len()),
                format!(
                    "last event: {}",
                    events
                        .last()
                        .map(trace_event_label)
                        .unwrap_or_else(|| "unknown".to_string())
                ),
            ],
        );
    }

    let mut broad_glob_evidence = Vec::new();
    let mut generated_path_evidence = Vec::new();
    let mut max_input_bytes = 0usize;
    let mut first_input_bytes = None::<usize>;
    let mut model_calls = 0usize;

    for event in &events {
        if event.action == "model_call_start" {
            model_calls += 1;
            let input_bytes = event
                .meta
                .as_ref()
                .and_then(|meta| meta.get("input_bytes"))
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or_else(|| event.input.as_ref().map(value_len).unwrap_or(0));
            first_input_bytes.get_or_insert(input_bytes);
            max_input_bytes = max_input_bytes.max(input_bytes);
            if let Some(input) = &event.input {
                let text = value_to_search_text(input);
                collect_internal_path_evidence(
                    &text,
                    format!("model input at step {}", event.step),
                    &mut generated_path_evidence,
                );
            }
        }
        if event.action == "tool_batch_dispatch_item" {
            let tool = event
                .meta
                .as_ref()
                .and_then(|meta| meta.get("tool"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let input_text = event
                .input
                .as_ref()
                .map(value_to_search_text)
                .unwrap_or_default();
            let output_text = event
                .output
                .as_ref()
                .map(value_to_search_text)
                .unwrap_or_default();
            if tool == "glob" && input_text.contains("\"**/*\"") {
                collect_internal_path_evidence(
                    &output_text,
                    format!("glob **/* output at step {}", event.step),
                    &mut broad_glob_evidence,
                );
            }
            collect_internal_path_evidence(
                &output_text,
                format!("{tool} output at step {}", event.step),
                &mut generated_path_evidence,
            );
        }
    }

    if !broad_glob_evidence.is_empty() {
        push_trace_observation(
            observations,
            seen,
            &source_path,
            "trace_hygiene_warning",
            "broad glob exposed internal/generated paths to the agent trace",
            capped_evidence(broad_glob_evidence),
        );
    }
    if !generated_path_evidence.is_empty() {
        push_trace_observation(
            observations,
            seen,
            &source_path,
            "trace_hygiene_warning",
            "generated or internal paths were exposed in model/tool observations",
            capped_evidence(generated_path_evidence),
        );
    }
    if let Some(first) = first_input_bytes {
        if model_calls >= 3
            && max_input_bytes >= 50_000
            && max_input_bytes >= first.saturating_mul(8)
        {
            push_trace_observation(
                observations,
                seen,
                &source_path,
                "context_hygiene_warning",
                "model context grew rapidly before the run produced an artifact verdict",
                vec![
                    format!("model calls: {model_calls}"),
                    format!("first model input bytes: {first}"),
                    format!("max model input bytes: {max_input_bytes}"),
                ],
            );
        }
    }

    Ok(())
}

fn push_trace_observation(
    observations: &mut Vec<ImproveObservation>,
    seen: &mut BTreeSet<String>,
    source_path: &str,
    category: &str,
    message: &str,
    evidence: Vec<String>,
) {
    let key = format!("trace:{source_path}:{category}:{message}");
    if !seen.insert(key) {
        return;
    }
    observations.push(ImproveObservation {
        id: String::new(),
        source_kind: "trace_jsonl".to_string(),
        source_path: source_path.to_string(),
        artifact_path: None,
        suite: None,
        suite_path: None,
        task_id: None,
        task: None,
        category: category.to_string(),
        category_source: None,
        effective_category: None,
        advisory_category: None,
        message: message.to_string(),
        final_success: false,
        changed_files: Vec::new(),
        skills: Vec::new(),
        evidence,
    });
}

fn bench_task_failed(task: &BenchTaskRunLite) -> bool {
    if task.pass == Some(false) || task.failure_reason.is_some() || task.error.is_some() {
        return true;
    }
    task.verdict
        .as_ref()
        .and_then(|verdict| verdict.get("final_success"))
        .and_then(Value::as_bool)
        == Some(false)
}

fn artifact_failed(artifact: &Value) -> bool {
    if artifact
        .get("failure_reason")
        .is_some_and(|reason| !reason.is_null())
    {
        return true;
    }
    artifact
        .get("verdict")
        .and_then(|verdict| verdict.get("final_success"))
        .and_then(Value::as_bool)
        == Some(false)
}

fn trace_event_label(event: &air_runtime::TraceEvent) -> String {
    format!(
        "step={} rule={} action={} status={:?}",
        event.step, event.rule, event.action, event.status
    )
}

fn value_len(value: &Value) -> usize {
    serde_json::to_string(value)
        .map(|text| text.len())
        .unwrap_or_default()
}

fn value_to_search_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn collect_internal_path_evidence(text: &str, label: String, evidence: &mut Vec<String>) {
    for marker in [
        ".git/",
        ".git\\\\/",
        ".air/",
        ".air\\\\/",
        "target/",
        "target\\\\/",
        "node_modules/",
        "node_modules\\\\/",
    ] {
        if text.contains(marker) {
            evidence.push(format!(
                "{label}: contains `{}`",
                marker.replace("\\\\/", "/")
            ));
        }
    }
}

fn capped_evidence(evidence: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    evidence
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .take(8)
        .collect()
}

fn failure_category_and_message(
    failure: Option<&Value>,
    verdict: Option<&Value>,
    error: Option<&str>,
) -> (String, String) {
    if let Some(failure) = failure.filter(|failure| !failure.is_null()) {
        let raw_category = failure
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("agent_error")
            .to_string();
        let message = failure
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("run failed")
            .to_string();
        return (normalize_category(&raw_category, &message), message);
    }
    if let Some(error) = error {
        return (
            infer_category_from_text(error),
            format!("benchmark task errored: {}", truncate(error, 240)),
        );
    }
    if let Some(verdict) = verdict {
        if verdict.get("patch_applied").and_then(Value::as_bool) == Some(false) {
            return (
                "no_patch_applied".to_string(),
                "runtime verdict did not observe a workspace patch".to_string(),
            );
        }
        if verdict.get("verification_ran").and_then(Value::as_bool) == Some(false) {
            return (
                "verification_failed".to_string(),
                "runtime verdict did not observe a verification command".to_string(),
            );
        }
        if verdict.get("verification_passed").and_then(Value::as_bool) == Some(false) {
            return (
                "verification_failed".to_string(),
                "runtime verdict did not observe passing verification".to_string(),
            );
        }
        if [
            "allowed_files_ok",
            "required_files_ok",
            "forbidden_files_ok",
            "required_diff_ok",
            "max_diff_lines_ok",
        ]
        .iter()
        .any(|key| verdict.get(*key).and_then(Value::as_bool) == Some(false))
        {
            return (
                "diff_constraint_failed".to_string(),
                "workspace diff did not satisfy constraints".to_string(),
            );
        }
    }
    (
        "agent_error".to_string(),
        "run failed without a structured failure reason".to_string(),
    )
}

fn infer_category_from_text(text: &str) -> String {
    let text = text.to_ascii_lowercase();
    if text.contains("rate limit") || text.contains("429") {
        "provider_rate_limited".to_string()
    } else if text.contains("max_model_calls")
        || text.contains("max_tool_calls")
        || text.contains("budget")
    {
        "budget_exceeded".to_string()
    } else if text.starts_with("read ") || text.contains("no such file") {
        "configuration_error".to_string()
    } else if text.contains("replay") || text.contains("snapshot") || text.contains("stale") {
        "replay_mismatch".to_string()
    } else if text.contains("mcp") {
        "mcp_tool_failure".to_string()
    } else if text.contains("route") || text.contains("skill") {
        "skill_routing_miss".to_string()
    } else {
        "agent_error".to_string()
    }
}

fn apply_failure_classifier_advisory(
    provider: &mut Option<Box<dyn ModelProvider>>,
    observations: &mut [ImproveObservation],
) -> FailureClassifierRun {
    let mut summaries = Vec::new();
    let mut errors = Vec::new();
    let cache_root = Path::new(".air/memory/cache");
    let mut candidates = 0usize;
    if provider.is_none() || llm_advisory_disabled() {
        let status = if llm_advisory_disabled() {
            "needs_llm_advisory_disabled"
        } else {
            "needs_llm_advisory_model_config"
        };
        for observation in observations.iter_mut() {
            if matches!(
                observation.category.as_str(),
                "agent_error" | "unknown_failure"
            ) {
                observation.category_source = Some(status.to_string());
            }
        }
        return FailureClassifierRun {
            status: status.to_string(),
            summaries,
            errors,
        };
    }
    let provider: &mut dyn ModelProvider = &mut **provider.as_mut().expect("checked above");
    for observation in observations.iter_mut() {
        if !matches!(
            observation.category.as_str(),
            "agent_error" | "unknown_failure"
        ) {
            continue;
        }
        candidates += 1;
        let request = FailureClassifierRequest {
            observation_id: observation.id.clone(),
            source_kind: observation.source_kind.clone(),
            source_path: observation.source_path.clone(),
            task_id: observation.task_id.clone(),
            task: observation.task.clone(),
            deterministic_category: observation.category.clone(),
            message: observation.message.clone(),
            evidence: observation.evidence.clone(),
            changed_files: observation.changed_files.clone(),
            allowed_categories: vec![
                "agent_error",
                "unknown_failure",
                "verification_failed",
                "no_patch_applied",
                "diff_constraint_failed",
                "budget_exceeded",
                "configuration_error",
                "provider_rate_limited",
                "replay_mismatch",
                "mcp_tool_failure",
                "skill_routing_miss",
                "aborted_trace_without_artifact",
                "trace_hygiene_warning",
                "context_hygiene_warning",
                "tool_call_error",
                "model_output_malformed",
                "workspace_setup_failed",
            ],
        };
        let result = cached_llm_advisory(
            CachedLlmAdvisoryOptions {
                cache_root,
                out_file: None,
                purpose: "failure-classifier",
                model_alias: FAILURE_CLASSIFIER_MODEL,
                request: &request,
                generated_at_unix: unix_now(),
            },
            &mut *provider,
            |output| validate_failure_classifier_output(&request, output),
        );
        let advisory = match result {
            Ok(advisory) => advisory,
            Err(error) => {
                observation.category_source =
                    Some("deterministic_fallback_after_llm_error".to_string());
                errors.push(FailureClassifierError {
                    observation_id: observation.id.clone(),
                    deterministic_category: observation.category.clone(),
                    error: error.to_string(),
                });
                continue;
            }
        };
        let summary = FailureClassifierSummary {
            observation_id: observation.id.clone(),
            deterministic_category: observation.category.clone(),
            advisory_category: advisory.category.clone(),
            confidence: advisory.confidence,
            rationale: advisory.rationale.clone(),
            evidence: advisory.evidence.clone(),
        };
        observation.category_source = Some("llm_advisory".to_string());
        observation.effective_category = Some(advisory.category.clone());
        observation.advisory_category = Some(advisory);
        summaries.push(summary);
    }
    let status = if candidates == 0 {
        "not_needed"
    } else if errors.is_empty() {
        "complete"
    } else if summaries.is_empty() {
        "llm_advisory_failed_deterministic_fallback"
    } else {
        "partial_llm_advisory"
    };
    FailureClassifierRun {
        status: status.to_string(),
        summaries,
        errors,
    }
}

fn validate_failure_classifier_output(
    request: &FailureClassifierRequest,
    output: Value,
) -> Result<ImproveAdvisoryCategory> {
    let parsed: FailureClassifierOutput =
        serde_json::from_value(output).context("failure classifier output must be an object")?;
    let category = normalize_advisory_category(&parsed.category);
    if category.is_empty()
        || !request
            .allowed_categories
            .iter()
            .any(|allowed| *allowed == category)
    {
        anyhow::bail!(
            "failure classifier returned unsupported category `{}`",
            parsed.category
        );
    }
    let rationale = truncate(parsed.rationale.trim(), 600);
    if rationale.is_empty() {
        anyhow::bail!("failure classifier rationale is empty");
    }
    let evidence = parsed
        .evidence
        .into_iter()
        .map(|item| truncate(item.trim(), 240))
        .filter(|item| !item.is_empty())
        .take(5)
        .collect::<Vec<_>>();
    Ok(ImproveAdvisoryCategory {
        model: FAILURE_CLASSIFIER_MODEL.to_string(),
        category,
        confidence: parsed.confidence.unwrap_or(0.5).clamp(0.0, 1.0),
        rationale,
        evidence,
    })
}

fn normalize_advisory_category(category: &str) -> String {
    category
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn normalize_category(category: &str, message: &str) -> String {
    let message = message.to_ascii_lowercase();
    let mut normalized = String::new();
    for ch in category.chars() {
        if ch == '-' || ch == ' ' {
            normalized.push('_');
        } else if ch.is_ascii_uppercase() {
            normalized.push('_');
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push(ch);
        }
    }
    let normalized = normalized.trim_start_matches('_').to_string();
    let category = normalized.as_str();
    if message.contains("max_model_calls")
        || message.contains("max_tool_calls")
        || message.contains("budget")
    {
        return "budget_exceeded".to_string();
    }
    if message.starts_with("read ") || message.contains("no such file") {
        return "configuration_error".to_string();
    }
    match category {
        "no_patch" => "no_patch_applied".to_string(),
        "diff_constraint" => "diff_constraint_failed".to_string(),
        "verification" => "verification_failed".to_string(),
        category => category.to_string(),
    }
}

fn mine_findings(observations: &[ImproveObservation]) -> Vec<ImproveFinding> {
    let mut grouped: BTreeMap<String, Vec<&ImproveObservation>> = BTreeMap::new();
    for observation in observations {
        grouped
            .entry(normalize_category(
                observation
                    .effective_category
                    .as_deref()
                    .unwrap_or(&observation.category),
                &observation.message,
            ))
            .or_default()
            .push(observation);
    }
    let mut groups = grouped.into_iter().collect::<Vec<_>>();
    groups.sort_by(
        |(left_category, left_observations), (right_category, right_observations)| {
            finding_impact_score(right_category, right_observations)
                .cmp(&finding_impact_score(left_category, left_observations))
                .then_with(|| left_category.cmp(right_category))
        },
    );
    groups
        .into_iter()
        .enumerate()
        .map(|(index, (category, observations))| {
            let id = format!("IMP-{number:03}", number = index + 1);
            let count = observations.len();
            let evidence = observations
                .iter()
                .take(8)
                .map(|observation| {
                    let label = observation
                        .task_id
                        .as_deref()
                        .or(observation.task.as_deref())
                        .unwrap_or(&observation.source_path);
                    format!("{}: {}", observation.id, truncate(label, 160))
                })
                .collect::<Vec<_>>();
            ImproveFinding {
                id,
                category: category.clone(),
                summary: finding_summary(&category, count),
                count,
                impact_score: finding_impact_score(&category, &observations),
                priority_reason: priority_reason(&category, count),
                recommended_change_type: recommended_change_type(&category).to_string(),
                expected_regression: expected_regression(&category).to_string(),
                evidence,
                observation_ids: observations
                    .iter()
                    .map(|observation| observation.id.clone())
                    .collect(),
            }
        })
        .collect()
}

fn category_priority(category: &str) -> i32 {
    match category {
        "budget_exceeded" => 95,
        "configuration_error" => 90,
        "diff_constraint_failed" => 85,
        "no_patch_applied" => 80,
        "verification_failed" => 75,
        "skill_routing_miss" => 70,
        "mcp_tool_failure" => 65,
        "replay_mismatch" => 60,
        "provider_rate_limited" => 50,
        "aborted_trace_without_artifact" => 88,
        "context_hygiene_warning" => 72,
        "trace_hygiene_warning" => 68,
        _ => 40,
    }
}

fn finding_impact_score(category: &str, observations: &[&ImproveObservation]) -> i32 {
    let frequency = observations.len() as i32 * 100;
    let category_weight = category_priority(category);
    let file_spread = observations
        .iter()
        .flat_map(|observation| observation.changed_files.iter())
        .collect::<BTreeSet<_>>()
        .len() as i32
        * 4;
    let task_spread = observations
        .iter()
        .filter_map(|observation| observation.task_id.as_deref())
        .collect::<BTreeSet<_>>()
        .len() as i32
        * 20;
    frequency + category_weight + file_spread + task_spread
}

fn priority_reason(category: &str, count: usize) -> String {
    format!(
        "frequency={count}, category_weight={}, likely shared failure mode={}",
        category_priority(category),
        recommended_change_type(category)
    )
}

fn finding_summary(category: &str, count: usize) -> String {
    match category {
        "no_patch_applied" => format!("{count} run(s) failed because no patch was applied"),
        "verification_failed" => {
            format!("{count} run(s) failed because verification was missing or failed")
        }
        "diff_constraint_failed" => {
            format!("{count} run(s) failed benchmark or project diff constraints")
        }
        "policy_violation" => format!("{count} run(s) failed policy or permission checks"),
        "budget_exceeded" => format!("{count} run(s) exhausted model/tool budget"),
        "configuration_error" => format!("{count} run(s) failed due to missing or invalid config"),
        "provider_rate_limited" => format!("{count} run(s) hit provider rate limits"),
        "replay_mismatch" => format!("{count} run(s) had stale or mismatched replay artifacts"),
        "mcp_tool_failure" => format!("{count} run(s) failed in MCP tooling"),
        "skill_routing_miss" => format!("{count} run(s) suggest a skill routing miss"),
        "aborted_trace_without_artifact" => {
            format!("{count} trace(s) ended without a run artifact verdict")
        }
        "trace_hygiene_warning" => {
            format!("{count} trace(s) exposed generated or internal paths to the agent")
        }
        "context_hygiene_warning" => {
            format!("{count} trace(s) showed risky context growth or context pollution")
        }
        _ => format!("{count} run(s) failed with category `{category}`"),
    }
}

fn recommended_change_type(category: &str) -> &'static str {
    match category {
        "skill_routing_miss" => "skill_routing_patch",
        "mcp_tool_failure" => "mcp_governance_or_tool_patch",
        "replay_mismatch" => "artifact_replay_regression",
        "diff_constraint_failed" => "bench_or_project_constraint_regression",
        "verification_failed" => "verification_loop_regression",
        "no_patch_applied" => "code_edit_loop_regression",
        "provider_rate_limited" => "provider_budget_or_retry_policy",
        "budget_exceeded" => "loop_budget_or_context_policy",
        "aborted_trace_without_artifact" => "trace_artifact_completion_policy",
        "trace_hygiene_warning" => "tool_context_hygiene_policy",
        "context_hygiene_warning" => "loop_budget_or_context_policy",
        "configuration_error" => "profile_or_path_configuration_fix",
        _ => "failure_regression",
    }
}

fn expected_regression(category: &str) -> &'static str {
    match category {
        "skill_routing_miss" => "A routing fixture selects the expected instruction skill.",
        "mcp_tool_failure" => {
            "An MCP fixture reports the same risk or tool failure deterministically."
        }
        "replay_mismatch" => "A replay fixture fails when artifact and workspace snapshots differ.",
        "diff_constraint_failed" => {
            "A benchmark fixture fails when changed files or diff content violate constraints."
        }
        "verification_failed" => {
            "A verdict fixture fails when no passing verification is present after a patch."
        }
        "no_patch_applied" => {
            "A verdict fixture fails when an edit task completes without a patch."
        }
        "budget_exceeded" => {
            "A benchmark fixture captures the loop path that exhausts model/tool budget."
        }
        "aborted_trace_without_artifact" => {
            "A trace fixture fails when a run aborts without a return event or artifact verdict."
        }
        "trace_hygiene_warning" => {
            "A trace fixture warns when broad tools expose .git, .air, target, or dependency paths."
        }
        "context_hygiene_warning" => {
            "A trace fixture warns when model context grows rapidly after noisy observations."
        }
        "configuration_error" => {
            "A configuration fixture fails fast when profile, tool, or skill paths are invalid."
        }
        _ => "A regression fixture reproduces this failure category without a live model call.",
    }
}

fn suggest_regressions(findings: &[ImproveFinding]) -> Vec<SuggestedRegression> {
    findings
        .iter()
        .map(|finding| SuggestedRegression {
            finding_id: finding.id.clone(),
            kind: regression_kind(&finding.category).to_string(),
            category: finding.category.clone(),
            title: finding.summary.clone(),
            source_observations: finding.observation_ids.clone(),
            fixture_hint: json!({
                "expected": {
                    "category": finding.category,
                    "final_success": false
                },
                "notes": finding.expected_regression,
            }),
        })
        .collect()
}

fn regression_kind(category: &str) -> &'static str {
    match category {
        "skill_routing_miss" => "skill_route",
        "mcp_tool_failure" => "mcp_audit",
        "replay_mismatch" => "artifact_replay",
        "aborted_trace_without_artifact" | "trace_hygiene_warning" | "context_hygiene_warning" => {
            "trace_hygiene"
        }
        _ => "code_run_verdict",
    }
}

fn write_regression_files(dir: &Path, regressions: &[SuggestedRegression]) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    for regression in regressions {
        let path = dir.join(format!(
            "{}.json",
            regression.finding_id.to_ascii_lowercase()
        ));
        write_json(&path, regression)?;
    }
    Ok(())
}

fn infer_suite_path(roots: &[PathBuf], suite: &str, task_id: &str) -> Result<Option<String>> {
    let suite_name = suite.split(':').next().unwrap_or(suite);
    let mut files = Vec::new();
    for root in roots {
        collect_suite_files(root, &mut files)?;
    }
    files.sort();
    files.dedup();
    for path in files {
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let Ok(candidate) = serde_json::from_str::<BenchSuiteProbe>(&text) else {
            continue;
        };
        if candidate.name != suite_name {
            continue;
        }
        if candidate.tasks.iter().any(|task| task.id == task_id) {
            return Ok(Some(path.display().to_string()));
        }
    }
    Ok(None)
}

fn choose_next_action(
    findings: &[ImproveFinding],
    observations: &[ImproveObservation],
    statuses: &BTreeMap<String, ImproveFindingStatus>,
) -> Option<ImproveNextAction> {
    let finding = findings.iter().find(|finding| {
        !matches!(
            statuses
                .get(&finding.id)
                .map(|status| status.status.as_str()),
            Some("stale")
        )
    })?;
    if matches!(
        statuses
            .get(&finding.id)
            .map(|status| status.status.as_str()),
        Some("still_failing" | "check_failed" | "needs_manual_triage")
    ) {
        return Some(ImproveNextAction {
            finding: finding.id.clone(),
            category: finding.category.clone(),
            status: "needs_regression".to_string(),
            command: format!("air improve promote {}", finding.id),
            reason: "check did not clear this finding; promote it before changing code".to_string(),
        });
    }
    if matches!(
        statuses
            .get(&finding.id)
            .map(|status| status.status.as_str()),
        Some("promoted")
    ) {
        return Some(ImproveNextAction {
            finding: finding.id.clone(),
            category: finding.category.clone(),
            status: "ready_for_fix".to_string(),
            command: format!("air improve fix {}", finding.id),
            reason: "a regression candidate exists, so a reviewable fix can be planned".to_string(),
        });
    }
    let linked = observations_for_finding(finding, observations);
    if linked
        .iter()
        .any(|observation| observation.task_id.is_some())
    {
        return Some(ImproveNextAction {
            finding: finding.id.clone(),
            category: finding.category.clone(),
            status: "needs_check".to_string(),
            command: format!("air improve check {}", finding.id),
            reason:
                "finding has a benchmark task id, so AIR can rerun the smallest known reproduction"
                    .to_string(),
        });
    }
    Some(ImproveNextAction {
        finding: finding.id.clone(),
        category: finding.category.clone(),
        status: "needs_regression".to_string(),
        command: format!("air improve promote {}", finding.id),
        reason: "no runnable benchmark task was found; promote the evidence into a regression candidate first".to_string(),
    })
}

fn check_finding(
    finding_id: &str,
    out_dir: &Path,
    roots: &[PathBuf],
    findings: &[ImproveFinding],
    observations: &[ImproveObservation],
) -> Result<()> {
    let Some(finding) = find_finding(finding_id, findings) else {
        anyhow::bail!("unknown finding `{finding_id}`");
    };
    let linked = observations_for_finding(finding, observations);
    let selected_observation = linked
        .iter()
        .find(|observation| observation.task_id.is_some());
    let Some(task_id) = selected_observation.and_then(|observation| observation.task_id.clone())
    else {
        let output = ImproveCheckOutput {
            schema: "air.improve_check.v1",
            finding: finding.id.clone(),
            status: "needs_manual_triage".to_string(),
            command: Vec::new(),
            check_run_dir: String::new(),
            report: String::new(),
            failed_tasks: finding.count,
            message: "This finding is not tied to a benchmark task id, so AIR cannot rerun it automatically yet.".to_string(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    };
    let linked_suite_path = linked
        .iter()
        .find_map(|observation| observation.suite_path.clone());
    let inferred_suite_path = selected_observation
        .and_then(|observation| observation.suite.as_deref())
        .and_then(|suite| infer_suite_path(roots, suite, &task_id).transpose())
        .transpose()?;
    let suite_path = selected_observation
        .and_then(|observation| observation.suite_path.clone())
        .or(linked_suite_path)
        .or(inferred_suite_path)
        .unwrap_or_else(|| DEFAULT_CHECK_SUITE.to_string());

    let check_run_dir = out_dir.join("checks").join(&finding.id);
    let report = out_dir
        .join("checks")
        .join(format!("{}-check.md", finding.id.to_ascii_lowercase()));
    fs::create_dir_all(check_run_dir.parent().unwrap_or(out_dir))
        .with_context(|| format!("create checks under {}", out_dir.display()))?;
    let executable = std::env::current_exe().context("resolve current air executable")?;
    let command_args = vec![
        "bench".to_string(),
        "code".to_string(),
        "--suite".to_string(),
        suite_path,
        "--task".to_string(),
        task_id.clone(),
        "--out-dir".to_string(),
        check_run_dir.display().to_string(),
        "--report".to_string(),
        report.display().to_string(),
    ];
    let status = Command::new(&executable)
        .args(&command_args)
        .status()
        .with_context(|| format!("run {}", executable.display()))?;
    let run_json = check_run_dir.join("run.json");
    let failed_tasks = if run_json.exists() {
        count_failed_bench_tasks(&run_json)?
    } else {
        finding.count
    };
    let status_label = if !run_json.exists() {
        "needs_manual_triage"
    } else if status.success() && failed_tasks == 0 {
        "stale"
    } else if failed_tasks > 0 {
        "still_failing"
    } else {
        "check_failed"
    };
    let message = match status_label {
        "stale" => {
            "The smallest known benchmark reproduction now passes; no code change is recommended for this finding yet."
        }
        "still_failing" => {
            "The finding still reproduces. Promote it before changing runtime, tool policy, or skills."
        }
        "needs_manual_triage" => {
            "AIR could not rerun this finding automatically. Promote it or rerun the original suite manually."
        }
        _ => "The check command failed before producing a clean pass/fail result.",
    };
    let mut command = vec![executable.display().to_string()];
    command.extend(command_args);
    let output = ImproveCheckOutput {
        schema: "air.improve_check.v1",
        finding: finding.id.clone(),
        status: status_label.to_string(),
        command,
        check_run_dir: check_run_dir.display().to_string(),
        report: report.display().to_string(),
        failed_tasks,
        message: message.to_string(),
    };
    write_finding_status(&out_dir.join("status"), &finding.id, status_label, message)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn promote_finding(
    finding_id: &str,
    out_dir: &Path,
    regressions: &[SuggestedRegression],
) -> Result<()> {
    let Some(regression) = regressions
        .iter()
        .find(|item| item.finding_id == finding_id)
    else {
        anyhow::bail!("unknown finding `{finding_id}`");
    };
    let path = Path::new("skills/code-agent/benches/regressions").join(format!(
        "{}.json",
        regression.finding_id.to_ascii_lowercase()
    ));
    write_json(&path, regression)?;
    let output = ImprovePromoteOutput {
        schema: "air.improve_promote.v1",
        finding: regression.finding_id.clone(),
        status: "promoted".to_string(),
        regression_file: path.display().to_string(),
        message: "Suggested regression was written. Review it before using it as a hard gate."
            .to_string(),
    };
    write_finding_status(
        &out_dir.join("status"),
        &regression.finding_id,
        "promoted",
        "Suggested regression was written for review.",
    )?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn prepare_fix(
    finding_id: Option<&str>,
    findings: &[ImproveFinding],
    statuses: &BTreeMap<String, ImproveFindingStatus>,
) -> Result<()> {
    let selected = finding_id
        .and_then(|id| find_finding(id, findings))
        .or_else(|| findings.first());
    let Some(finding) = selected else {
        let output = ImproveFixOutput {
            schema: "air.improve_fix.v1",
            status: "no_findings".to_string(),
            finding: None,
            message: "No findings are available to fix.".to_string(),
            next_command: None,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    };
    if matches!(
        statuses
            .get(&finding.id)
            .map(|status| status.status.as_str()),
        Some("stale")
    ) {
        let output = ImproveFixOutput {
            schema: "air.improve_fix.v1",
            status: "no_fix_needed".to_string(),
            finding: Some(finding.id.clone()),
            message: "The finding is marked stale after a successful check; no source change is recommended.".to_string(),
            next_command: None,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    let regression = Path::new("skills/code-agent/benches/regressions")
        .join(format!("{}.json", finding.id.to_ascii_lowercase()));
    let (status, message, next_command) = if regression.exists() {
        (
            "ready_for_project_fix",
            "Regression candidate exists. Use project/code mode to implement a candidate patch in a separate reviewable change.",
            Some(format!(
                "air run \"fix {}: {}\" --mode project --execute",
                finding.id, finding.summary
            )),
        )
    } else {
        (
            "blocked_until_promoted",
            "Promote the finding into a regression candidate before asking AIR to change source code.",
            Some(format!("air improve promote {}", finding.id)),
        )
    };
    let output = ImproveFixOutput {
        schema: "air.improve_fix.v1",
        status: status.to_string(),
        finding: Some(finding.id.clone()),
        message: message.to_string(),
        next_command,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn evaluate_finding(
    finding: &str,
    report: Option<&Path>,
    regression_file: Option<&Path>,
    out_dir: &Path,
    roots: &[PathBuf],
) -> Result<()> {
    let eval_dir = out_dir.join("evaluations").join(finding);
    fs::create_dir_all(&eval_dir).with_context(|| format!("create {}", eval_dir.display()))?;
    let regression = run_regression_suite(RegressionRunConfig {
        finding: Some(finding.to_string()),
        file: regression_file.map(Path::to_path_buf),
        all: false,
        from: roots.to_vec(),
        out_dir: eval_dir.join("regression"),
    })?;
    let tests = run_evaluate_tests()?;
    let guard = run_improve_guard()?;
    let regression_gate = summarize_regression_gate(&regression);
    let hard_gates_passed = regression.failed == 0
        && regression_gate.causal_pass
        && tests.passed
        && guard.blocked.is_empty();
    let decision = if hard_gates_passed {
        "accept_candidate"
    } else if !guard.blocked.is_empty()
        || regression_gate.unresolved_stale > 0
        || regression_gate.unsupported > 0
    {
        "needs_review"
    } else {
        "reject_candidate"
    };
    let score_delta = evaluate_score_delta(&regression, &tests, &guard);
    let eval_json = eval_dir.join("eval.json");
    let eval_report = report
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| eval_dir.join("eval.md"));
    let output = ImproveEvaluateOutput {
        schema: "air.improve_evaluate.v1",
        finding: finding.to_string(),
        decision: decision.to_string(),
        regression_gate,
        regression,
        tests,
        guard,
        score_delta,
        eval_json: eval_json.display().to_string(),
        eval_report: eval_report.display().to_string(),
    };
    write_json(&eval_json, &output)?;
    write_evaluate_report(&eval_report, &output)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn run_evaluate_tests() -> Result<ImproveTestGate> {
    let command = vec![
        "cargo".to_string(),
        "test".to_string(),
        "-p".to_string(),
        "air-cli".to_string(),
        "--no-fail-fast".to_string(),
    ];
    let status = Command::new("cargo")
        .args(&command[1..])
        .status()
        .context("run cargo test -p air-cli --no-fail-fast")?;
    Ok(ImproveTestGate {
        command,
        passed: status.success(),
        status: status.code(),
    })
}

fn summarize_regression_gate(regression: &RegressionRunReport) -> RegressionGateSummary {
    let statuses = regression
        .results
        .iter()
        .map(|result| result.status.clone())
        .collect::<Vec<_>>();
    let unresolved_stale = regression
        .results
        .iter()
        .filter(|result| result.status == "stale")
        .count();
    let unsupported = regression
        .results
        .iter()
        .filter(|result| result.status == "unsupported")
        .count();
    let causal_pass = regression.total > 0
        && regression
            .results
            .iter()
            .all(|result| result.passed && matches!(result.status.as_str(), "passed" | "resolved"));
    RegressionGateSummary {
        causal_pass,
        unresolved_stale,
        unsupported,
        statuses,
    }
}

fn run_improve_guard() -> Result<ImproveGuardGate> {
    let output = Command::new("git")
        .args(["diff", "--name-status", "HEAD", "--"])
        .output()
        .context("run git diff --name-status HEAD")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut warnings = Vec::new();
    let mut blocked = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let status = parts.next().unwrap_or_default();
        let path = parts.next_back().unwrap_or_default();
        if status.starts_with('D') && is_protected_eval_path(path) {
            blocked.push(format!("deleted protected evaluation file: {path}"));
        }
        if status.starts_with('M') && is_protected_eval_path(path) {
            blocked.push(format!("modified protected evaluation file: {path}"));
        }
        if status.starts_with('M') && path.ends_with("skills.lock") {
            blocked.push("modified skills.lock trust state".to_string());
        }
    }
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()
        .context("run git ls-files --others")?;
    for line in String::from_utf8_lossy(&untracked.stdout).lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        if is_protected_eval_path(path) {
            blocked.push(format!("added untracked protected evaluation file: {path}"));
        }
        if let Ok(text) = std::fs::read_to_string(path) {
            for trimmed in text.lines().map(str::trim_start) {
                if disables_required_verification(trimmed) {
                    blocked.push(format!(
                        "untracked file adds require_verification: false: {path}"
                    ));
                }
                if exposes_sensitive_capability(trimmed, "shell.unrestricted") {
                    warnings.push(format!(
                        "untracked file changes shell.unrestricted capability text: {path}"
                    ));
                }
                if exposes_sensitive_capability(trimmed, "secrets.read") {
                    warnings.push(format!(
                        "untracked file changes secrets.read capability text: {path}"
                    ));
                }
            }
        }
    }

    let diff = Command::new("git")
        .args(["diff", "--unified=0", "HEAD", "--"])
        .output()
        .context("run git diff HEAD")?;
    let diff_text = String::from_utf8_lossy(&diff.stdout);
    for line in diff_text.lines().filter(|line| line.starts_with('+')) {
        let added = line.trim_start_matches('+');
        let trimmed = added.trim_start();
        if trimmed.starts_with("+++") {
            continue;
        }
        if disables_required_verification(trimmed) {
            blocked.push("added require_verification: false".to_string());
        }
        if exposes_sensitive_capability(trimmed, "shell.unrestricted") {
            warnings.push("changed shell.unrestricted capability text".to_string());
        }
        if exposes_sensitive_capability(trimmed, "secrets.read") {
            warnings.push("changed secrets.read capability text".to_string());
        }
    }
    if let Some(eval_report) = check_default_eval_manifest_if_present()? {
        if !eval_report.passed {
            for path in eval_report.missing {
                blocked.push(format!("eval manifest missing protected file: {path}"));
            }
            for path in eval_report.changed {
                blocked.push(format!("eval manifest hash mismatch: {path}"));
            }
            for path in eval_report.protected_changed {
                blocked.push(format!("modified eval-protected path: {path}"));
            }
        }
    }
    blocked.sort();
    blocked.dedup();
    warnings.sort();
    warnings.dedup();
    Ok(ImproveGuardGate {
        passed: blocked.is_empty(),
        warnings,
        blocked,
    })
}

fn disables_required_verification(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    lowered.starts_with("require_verification: false")
        || lowered.starts_with("require_verification = false")
        || lowered.starts_with("\"require_verification\": false")
        || lowered.starts_with("'require_verification': false")
}

fn exposes_sensitive_capability(line: &str, capability: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    if lowered.contains("deny") {
        return false;
    }
    lowered.starts_with(&format!("- {capability}"))
        || lowered.starts_with(capability)
        || lowered.starts_with(&format!("\"{capability}\""))
        || lowered.starts_with(&format!("'{capability}'"))
}

fn is_protected_eval_path(path: &str) -> bool {
    path.contains("benches/")
        || path.contains("regressions/")
        || path.starts_with(".air/evals/")
        || path.ends_with("skills.lock")
        || path.ends_with("air-skill.yaml")
        || path.ends_with("audit.rs")
        || path.ends_with("dream.rs")
        || path.ends_with("improve.rs")
        || path.ends_with("regression.rs")
        || path.ends_with("self_lab.rs")
        || path.ends_with("eval_manifest.rs")
        || path.ends_with("code_artifact.rs")
}

fn evaluate_score_delta(
    regression: &RegressionRunReport,
    tests: &ImproveTestGate,
    guard: &ImproveGuardGate,
) -> i32 {
    let mut score = 0;
    if regression.failed == 0 {
        score += 100;
    } else {
        score -= 100 * regression.failed as i32;
    }
    if tests.passed {
        score += 40;
    } else {
        score -= 80;
    }
    score -= 120 * guard.blocked.len() as i32;
    score -= 10 * guard.warnings.len() as i32;
    score
}

fn write_evaluate_report(path: &Path, output: &ImproveEvaluateOutput) -> Result<()> {
    let mut out = String::new();
    out.push_str("# AIR Improve Evaluation\n\n");
    out.push_str(&format!("- finding: `{}`\n", output.finding));
    out.push_str(&format!("- decision: `{}`\n", output.decision));
    out.push_str(&format!("- score_delta: `{}`\n\n", output.score_delta));
    out.push_str("## Hard Gates\n\n");
    out.push_str(&format!(
        "- regression: `{}/{}` passed\n",
        output.regression.passed, output.regression.total
    ));
    out.push_str(&format!("- tests: `{}`\n", output.tests.passed));
    out.push_str(&format!("- guard: `{}`\n\n", output.guard.passed));
    out.push_str("## Regression Results\n\n");
    out.push_str("| Regression | Kind | Status | Pass | Message |\n");
    out.push_str("|---|---|---|---:|---|\n");
    for result in &output.regression.results {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            md_cell(&result.id),
            md_cell(&result.kind),
            md_cell(&result.status),
            if result.passed { "yes" } else { "no" },
            md_cell(&result.message)
        ));
    }
    if !output.guard.blocked.is_empty() {
        out.push_str("\n## Blocked\n\n");
        for item in &output.guard.blocked {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if !output.guard.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for item in &output.guard.warnings {
            out.push_str(&format!("- {}\n", item));
        }
    }
    fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

fn count_failed_bench_tasks(path: &Path) -> Result<usize> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let run: BenchRunLite =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(count_failed_bench_tasks_inner(&run))
}

fn count_failed_bench_tasks_inner(run: &BenchRunLite) -> usize {
    let own = run
        .tasks
        .iter()
        .filter(|task| bench_task_failed(task))
        .count();
    let nested = run
        .runs
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(count_failed_bench_tasks_inner)
        .sum::<usize>();
    own + nested
}

fn find_finding<'a>(
    finding_id: &str,
    findings: &'a [ImproveFinding],
) -> Option<&'a ImproveFinding> {
    findings.iter().find(|finding| finding.id == finding_id)
}

fn observations_for_finding<'a>(
    finding: &ImproveFinding,
    observations: &'a [ImproveObservation],
) -> Vec<&'a ImproveObservation> {
    let ids = finding
        .observation_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    observations
        .iter()
        .filter(|observation| ids.contains(&observation.id))
        .collect()
}

fn load_finding_statuses(out_dir: &Path) -> Result<BTreeMap<String, ImproveFindingStatus>> {
    let mut statuses = BTreeMap::new();
    let status_dir = out_dir.join("status");
    if !status_dir.exists() {
        return Ok(statuses);
    }
    for entry in
        fs::read_dir(&status_dir).with_context(|| format!("read {}", status_dir.display()))?
    {
        let path = entry
            .with_context(|| format!("read entry under {}", status_dir.display()))?
            .path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let status: ImproveFindingStatus = serde_json::from_str(
            &fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        statuses.insert(status.finding.clone(), status);
    }
    Ok(statuses)
}

fn write_finding_status(
    status_dir: &Path,
    finding: &str,
    status: &str,
    message: &str,
) -> Result<()> {
    fs::create_dir_all(status_dir).with_context(|| format!("create {}", status_dir.display()))?;
    let record = ImproveFindingStatus {
        schema: "air.improve_finding_status.v1".to_string(),
        finding: finding.to_string(),
        status: status.to_string(),
        message: message.to_string(),
        updated_at_unix: unix_now(),
    };
    write_json(
        &status_dir.join(format!("{}.json", finding.to_ascii_lowercase())),
        &record,
    )
}

fn write_report(
    path: &Path,
    roots: &[PathBuf],
    observations: &[ImproveObservation],
    findings: &[ImproveFinding],
    regressions: &[SuggestedRegression],
    statuses: &BTreeMap<String, ImproveFindingStatus>,
) -> Result<()> {
    let mut out = String::new();
    out.push_str("# AIR Improve Report\n\n");
    out.push_str("## Scope\n\n");
    for root in roots {
        out.push_str(&format!("- `{}`\n", root.display()));
    }
    out.push_str("\n## Summary\n\n");
    out.push_str(&format!("- observations: `{}`\n", observations.len()));
    out.push_str(&format!("- findings: `{}`\n", findings.len()));
    out.push_str(&format!(
        "- suggested_regressions: `{}`\n",
        regressions.len()
    ));
    if findings.is_empty() {
        out.push_str("\nNo failing AIR artifacts or benchmark tasks were found in scope.\n");
    } else {
        if let Some(next_action) = choose_next_action(findings, observations, statuses) {
            out.push_str("\n## Next Action\n\n");
            out.push_str(&format!(
                "- `{}`: {}\n",
                next_action.finding, next_action.reason
            ));
            out.push_str(&format!("- Run: `{}`\n", next_action.command));
        }
        out.push_str("\n## Findings\n\n");
        out.push_str("| Finding | Category | Status | Count | Impact | Priority Reason | Recommended Change | Next | Expected Regression |\n");
        out.push_str("|---|---|---|---:|---:|---|---|---|---|\n");
        for finding in findings {
            let status = statuses
                .get(&finding.id)
                .map(|status| status.status.as_str())
                .unwrap_or("open");
            let next = choose_next_action(std::slice::from_ref(finding), observations, statuses)
                .map(|action| action.command)
                .unwrap_or_else(|| "none".to_string());
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                finding.id,
                md_cell(&finding.category),
                md_cell(status),
                finding.count,
                finding.impact_score,
                md_cell(&finding.priority_reason),
                md_cell(&finding.recommended_change_type),
                md_cell(&next),
                md_cell(&finding.expected_regression),
            ));
        }
        out.push_str("\n## Evidence\n\n");
        for finding in findings {
            out.push_str(&format!("### {}: {}\n\n", finding.id, finding.summary));
            for evidence in &finding.evidence {
                out.push_str(&format!("- {}\n", evidence));
            }
            out.push('\n');
        }
        out.push_str("## Suggested Next Steps\n\n");
        out.push_str("1. Review `suggested_regressions.json` and promote useful entries into a real benchmark or replay fixture.\n");
        out.push_str("2. Add or update the regression before changing runtime, skill routing, or tool policy.\n");
        out.push_str("3. Run `air bench code` or `air bench skill` with `--report` to verify the candidate fix.\n");
    }
    fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

fn renumber_observations(observations: &mut [ImproveObservation]) {
    for (index, observation) in observations.iter_mut().enumerate() {
        observation.id = format!("OBS-{number:04}", number = index + 1);
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("write {}", path.display()))
}

fn string_array(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn compact_changed_files(
    mut changed_files: Vec<String>,
    evidence: &mut Vec<String>,
) -> Vec<String> {
    const MAX_CHANGED_FILES: usize = 40;
    changed_files.sort();
    changed_files.dedup();
    let total = changed_files.len();
    if total > MAX_CHANGED_FILES {
        evidence.push(format!(
            "changed_files truncated: showing {MAX_CHANGED_FILES} of {total}"
        ));
        changed_files.truncate(MAX_CHANGED_FILES);
    }
    changed_files
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn md_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn absolutize(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "air-improve-test-{}-{}-{name}",
            std::process::id(),
            unix_now()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn verdict_without_patch_maps_to_no_patch() {
        let verdict = json!({
            "final_success": false,
            "patch_applied": false,
            "verification_ran": true,
            "verification_passed": true
        });
        let (category, _) = failure_category_and_message(None, Some(&verdict), None);
        assert_eq!(category, "no_patch_applied");
    }

    #[test]
    fn verdict_without_verification_maps_to_verification_failed() {
        let verdict = json!({
            "final_success": false,
            "patch_applied": true,
            "verification_ran": false,
            "verification_passed": false
        });
        let (category, _) = failure_category_and_message(None, Some(&verdict), None);
        assert_eq!(category, "verification_failed");
    }

    #[test]
    fn findings_group_observations_by_category() {
        let observations = vec![
            ImproveObservation {
                id: "OBS-0001".to_string(),
                source_kind: "bench_task".to_string(),
                source_path: "run.json".to_string(),
                artifact_path: None,
                suite: Some("suite".to_string()),
                suite_path: Some("suite.json".to_string()),
                task_id: Some("task-a".to_string()),
                task: None,
                category: "verification_failed".to_string(),
                category_source: None,
                effective_category: None,
                advisory_category: None,
                message: "failed".to_string(),
                final_success: false,
                changed_files: Vec::new(),
                skills: Vec::new(),
                evidence: Vec::new(),
            },
            ImproveObservation {
                id: "OBS-0002".to_string(),
                source_kind: "bench_task".to_string(),
                source_path: "run.json".to_string(),
                artifact_path: None,
                suite: Some("suite".to_string()),
                suite_path: Some("suite.json".to_string()),
                task_id: Some("task-b".to_string()),
                task: None,
                category: "verification_failed".to_string(),
                category_source: None,
                effective_category: None,
                advisory_category: None,
                message: "failed".to_string(),
                final_success: false,
                changed_files: Vec::new(),
                skills: Vec::new(),
                evidence: Vec::new(),
            },
        ];
        let findings = mine_findings(&observations);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].count, 2);
        assert_eq!(findings[0].category, "verification_failed");
    }

    #[test]
    fn trace_only_aborted_run_produces_hygiene_findings() {
        let root = temp_root("trace-hygiene");
        let trace = root.join("trace.jsonl");
        fs::write(
            &trace,
            concat!(
                r#"{"agent":"code","step":1,"rule":"choose","action":"model_call_start","input":{"task":"refactor"},"meta":{"input_bytes":2000},"status":"ok"}"#,
                "\n",
                r#"{"agent":"code","step":2,"rule":"act","action":"tool_batch_dispatch_item","input":{"pattern":"**/*"},"output":{"files":["src/app.js",".git/HEAD",".air/dream/state.json","target/generated/run/trace.jsonl"]},"meta":{"tool":"glob"},"status":"ok"}"#,
                "\n",
                r#"{"agent":"code","step":3,"rule":"choose","action":"model_call_start","input":{"observations":"glob exposed .git/HEAD .air/dream/state.json target/generated/run/trace.jsonl"},"meta":{"input_bytes":60000},"status":"ok"}"#,
                "\n",
                r#"{"agent":"code","step":4,"rule":"choose","action":"model_call_start","input":{"observations":"continued noisy trace context with .git/HEAD .air/dream/state.json target/generated/run/trace.jsonl"},"meta":{"input_bytes":80000},"status":"ok"}"#,
                "\n"
            ),
        )
        .unwrap();

        let mut observations = Vec::new();
        let mut seen = BTreeSet::new();
        collect_trace_hygiene(&trace, &mut observations, &mut seen).unwrap();
        renumber_observations(&mut observations);
        let findings = mine_findings(&observations);
        let categories = findings
            .iter()
            .map(|finding| finding.category.as_str())
            .collect::<BTreeSet<_>>();

        assert!(categories.contains("aborted_trace_without_artifact"));
        assert!(categories.contains("trace_hygiene_warning"));
        assert!(categories.contains("context_hygiene_warning"));
        assert!(observations
            .iter()
            .any(|observation| observation.source_kind == "trace_jsonl"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn infer_suite_path_matches_group_suite_names() {
        let root = std::env::temp_dir().join(format!(
            "air-improve-suite-test-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let suite_dir = root.join("web-design-bench");
        fs::create_dir_all(&suite_dir).unwrap();
        let suite_path = suite_dir.join("suite.json");
        fs::write(
            &suite_path,
            r#"{
              "name": "web-design-smoke",
              "tasks": [
                {
                  "id": "analytics_landing_redesign",
                  "fixture": "fixture",
                  "prompt": "task"
                }
              ]
            }"#,
        )
        .unwrap();

        let inferred = infer_suite_path(
            std::slice::from_ref(&root),
            "web-design-smoke:no-skill",
            "analytics_landing_redesign",
        )
        .unwrap();
        assert_eq!(inferred, Some(suite_path.display().to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn improve_guard_patterns_ignore_their_own_string_literals() {
        assert!(!disables_required_verification(
            "if added.contains(\"require_verification: false\") {"
        ));
        assert!(disables_required_verification(
            "require_verification: false"
        ));
        assert!(disables_required_verification(
            "\"require_verification\": false"
        ));

        assert!(!exposes_sensitive_capability(
            "warnings.push(\"changed shell.unrestricted capability text\".to_string());",
            "shell.unrestricted"
        ));
        assert!(exposes_sensitive_capability(
            "- shell.unrestricted",
            "shell.unrestricted"
        ));
        assert!(!exposes_sensitive_capability(
            "- shell.unrestricted # deny",
            "shell.unrestricted"
        ));
    }

    #[test]
    fn protected_eval_paths_match_relative_paths() {
        assert!(is_protected_eval_path(
            "skills/code-agent/benches/rust-small/suite.json"
        ));
        assert!(is_protected_eval_path(
            "skills/code-agent/benches/regressions/imp-001.json"
        ));
        assert!(is_protected_eval_path("skills.lock"));
    }
}
