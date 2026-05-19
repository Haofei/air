use crate::code_artifact::sha256_hex;
use crate::memory::{advance_memory, extract_memory, MemoryAdvanceOptions, MemoryExtractOptions};
use crate::ops::{
    append_jsonl_locked, run_current_air_stage, with_jsonl_lock, write_json_atomic, AirLayout,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DREAM_SCHEMA: &str = "air.dream.v1";
const DREAM_STATE_SCHEMA: &str = "air.dream_state.v1";
const DREAM_WINDOW_SCHEMA: &str = "air.dream_window.v1";
const DREAM_FINDING_RECORD_SCHEMA: &str = "air.dream_finding_record.v1";
const DREAM_LEDGER_SCHEMA: &str = "air.dream_ledger_event.v1";
const DREAM_OUTPUT_MARKER: &str = ".air-dream-output";
const DREAM_CURSOR_OVERLAP_SECONDS: u64 = 300;
const DREAM_FINDING_LIST_LIMIT: usize = 50;
const DREAM_FINDINGS_COMPACT_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DreamMode {
    Micro,
    Deep,
    Evolution,
}

impl DreamMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Micro => "micro",
            Self::Deep => "deep",
            Self::Evolution => "evolution",
        }
    }
}

pub(crate) struct DreamRunOptions {
    pub(crate) from: Vec<PathBuf>,
    pub(crate) since_unix: Option<u64>,
    pub(crate) limit: Option<usize>,
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) write_regressions: bool,
    pub(crate) full: bool,
    pub(crate) mode: DreamMode,
    pub(crate) experiment: bool,
    pub(crate) top_findings: usize,
    pub(crate) budget_seconds: Option<u64>,
    pub(crate) candidates: usize,
    pub(crate) advance: bool,
    pub(crate) advance_limit: Option<usize>,
    pub(crate) advance_timeout_seconds: Option<u64>,
}

pub(crate) struct DreamStateOptions;

pub(crate) struct DreamFindingsListOptions {
    pub(crate) status: Option<String>,
    pub(crate) limit: Option<usize>,
}

pub(crate) struct DreamFindingUpdateOptions {
    pub(crate) finding: String,
    pub(crate) reason: Option<String>,
    pub(crate) fixed_by: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DreamState {
    schema: String,
    status: String,
    state_file: String,
    last_started_at_unix: Option<u64>,
    last_completed_at_unix: Option<u64>,
    last_out_dir: Option<String>,
    last_roots: Vec<String>,
    last_findings: usize,
    last_top_finding: Option<DreamFindingSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_window: Option<DreamWindowState>,
    #[serde(default)]
    root_cursors: Vec<DreamRootCursor>,
}

#[derive(Debug, Serialize)]
struct DreamRunOutput {
    schema: &'static str,
    status: String,
    run_id: String,
    mode: String,
    incremental: bool,
    incremental_source: String,
    since_unix: Option<u64>,
    state_file: String,
    out_dir: String,
    window_manifest: String,
    audit: DreamStage,
    improve: DreamStage,
    memory: DreamMemoryStage,
    memory_advance: DreamMemoryAdvanceStage,
    finding_store: DreamFindingStoreStage,
    experiment: DreamExperimentStage,
    window_inputs: usize,
    window_artifacts: usize,
    max_input_mtime_unix: Option<u64>,
    findings: usize,
    top_finding: Option<DreamFindingSummary>,
    dream_json: String,
    dream_report: String,
    next_commands: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DreamStage {
    status: String,
    out_dir: String,
    report: String,
    log: String,
}

#[derive(Debug, Serialize)]
struct DreamMemoryStage {
    status: String,
    memory_dir: String,
    report: String,
    episodes_written: usize,
    cards_written: usize,
    failure_candidates: usize,
    procedure_candidates: usize,
    routing_hints: usize,
    concept_candidates: usize,
    hypothesis_candidates: usize,
    policy_candidates: usize,
    graph_edges: usize,
    dream_ir_file: String,
}

#[derive(Debug, Serialize)]
struct DreamMemoryAdvanceStage {
    status: String,
    out_dir: String,
    promoted_memories: usize,
    validated_memories: usize,
    retired_memories: usize,
    validated_skills: usize,
    routing_measurements: usize,
    reviewed_guards: usize,
    report: String,
}

#[derive(Debug, Serialize)]
struct DreamFindingStoreStage {
    status: String,
    path: String,
    observed: usize,
    opened: usize,
    recurred: usize,
}

#[derive(Debug, Serialize)]
struct DreamExperimentStage {
    status: String,
    requested: bool,
    top_findings: usize,
    budget_seconds: Option<u64>,
    attempted: usize,
    results: Vec<DreamExperimentResult>,
}

#[derive(Debug, Serialize)]
struct DreamExperimentResult {
    finding: String,
    dream_run_id: String,
    finding_stable_key: String,
    window_manifest_sha: String,
    status: String,
    self_fix_log: Option<String>,
    compare_log: Option<String>,
    candidates_dir: String,
    report: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DreamFindingSummary {
    id: String,
    stable_key: String,
    persistent_id: String,
    display_rank: Option<u64>,
    category: String,
    summary: String,
    impact_score: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DreamWindowInput {
    kind: String,
    path: String,
    fingerprint: String,
    mtime_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DreamWindowState {
    from_unix: Option<u64>,
    to_unix: u64,
    max_input_mtime_unix: Option<u64>,
    inputs: Vec<DreamWindowInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DreamRootCursor {
    root: String,
    root_exists: bool,
    last_seen_max_mtime_unix: Option<u64>,
    last_scan_completed_at_unix: Option<u64>,
}

#[derive(Debug, Serialize)]
struct DreamWindowManifest {
    schema: &'static str,
    roots: Vec<String>,
    incremental: bool,
    incremental_source: String,
    since_unix: Option<u64>,
    overlap_seconds: u64,
    limit: Option<usize>,
    dedupe_previous_window: bool,
    inputs: Vec<DreamWindowInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DreamFindingRecord {
    schema: String,
    id: String,
    #[serde(default)]
    stable_key: String,
    #[serde(default)]
    display_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_rank: Option<u64>,
    #[serde(default)]
    aliases: Vec<String>,
    category: String,
    summary: String,
    impact_score: i64,
    status: String,
    first_seen_unix: u64,
    last_seen_unix: u64,
    recurrence_count: u64,
    #[serde(default)]
    seen_in: Vec<String>,
    #[serde(default)]
    linked_regressions: Vec<String>,
    #[serde(default)]
    linked_candidates: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_at_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dismissed_at_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fixed_by: Option<String>,
    #[serde(default)]
    recurred_after_fix: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct DreamFindingsListOutput {
    schema: &'static str,
    findings_file: String,
    findings: Vec<DreamFindingRecord>,
}

#[derive(Debug, Serialize)]
struct DreamFindingUpdateOutput {
    schema: &'static str,
    status: String,
    findings_file: String,
    finding: DreamFindingRecord,
}

#[derive(Debug, Serialize)]
struct DreamLedgerEvent {
    schema: &'static str,
    event: String,
    finding: String,
    at_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

pub(crate) fn run_dream(options: DreamRunOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let layout = AirLayout::new(cwd.clone());
    let started_at = unix_now();
    let run_id = dream_run_id(started_at);
    let state_file = layout.dream_state_path();
    let prior_state = read_dream_state(&state_file)?;
    let state_cursor = prior_state.as_ref().and_then(state_cursor_unix);
    let since_unix = if options.full {
        None
    } else {
        options.since_unix.or(state_cursor)
    };
    let incremental_source = if options.full {
        "full".to_string()
    } else if options.since_unix.is_some() {
        "explicit_since".to_string()
    } else if state_cursor.is_some() {
        "state".to_string()
    } else {
        "default".to_string()
    };
    let incremental = !options.full;
    let dedupe_previous_window = incremental && options.since_unix.is_none();
    let roots = if options.from.is_empty() {
        vec![PathBuf::from("target/generated")]
    } else {
        options.from.clone()
    };
    let default_out_dir = options.out_dir.is_none();
    let out_dir = absolutize(
        &cwd,
        options
            .out_dir
            .unwrap_or_else(|| PathBuf::from(".air/dream/runs").join(&run_id)),
    );
    if options.experiment {
        ensure_clean_experiment_workspace(&cwd, Some(&out_dir))?;
    }
    clear_dream_out_dir(&out_dir)?;
    let audit_dir = out_dir.join("audit");
    let improve_dir = out_dir.join("improve");
    let log_dir = out_dir.join("logs");
    fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;
    let window_inputs = discover_dream_window_inputs(
        &cwd,
        &roots,
        if options.full {
            None
        } else {
            options.since_unix
        },
        options.limit,
        dedupe_previous_window,
        prior_state.as_ref(),
        options.full,
    )?;
    let max_input_mtime_unix = window_inputs.iter().map(|input| input.mtime_unix).max();
    let audit_roots = window_inputs
        .iter()
        .filter(|input| input.kind == "code_run_artifact")
        .map(|input| PathBuf::from(&input.path))
        .collect::<Vec<_>>();
    let improve_roots = window_inputs
        .iter()
        .map(|input| PathBuf::from(&input.path))
        .collect::<Vec<_>>();
    let window_dir = out_dir.join("window");
    fs::create_dir_all(&window_dir).with_context(|| format!("create {}", window_dir.display()))?;
    let window_manifest = window_dir.join("window.json");
    let manifest = DreamWindowManifest {
        schema: DREAM_WINDOW_SCHEMA,
        roots: roots
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        incremental,
        incremental_source: incremental_source.clone(),
        since_unix,
        overlap_seconds: DREAM_CURSOR_OVERLAP_SECONDS,
        limit: options.limit,
        dedupe_previous_window,
        inputs: window_inputs.clone(),
    };
    fs::write(&window_manifest, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("write {}", window_manifest.display()))?;

    let effective_audit_roots = if audit_roots.is_empty() {
        let empty = window_dir.join("empty-audit");
        fs::create_dir_all(&empty).with_context(|| format!("create {}", empty.display()))?;
        vec![empty]
    } else {
        audit_roots.clone()
    };
    let effective_improve_roots = if improve_roots.is_empty() {
        let empty = out_dir.join("window").join("empty");
        fs::create_dir_all(&empty).with_context(|| format!("create {}", empty.display()))?;
        vec![empty]
    } else {
        improve_roots.clone()
    };

    let audit_report = audit_dir.join("report.md");
    let improve_report = improve_dir.join("report.md");
    let audit_log = log_dir.join("audit.log");
    let improve_log = log_dir.join("improve.log");
    if options.mode == DreamMode::Micro {
        fs::create_dir_all(&audit_dir)
            .with_context(|| format!("create {}", audit_dir.display()))?;
        fs::create_dir_all(&improve_dir)
            .with_context(|| format!("create {}", improve_dir.display()))?;
        write_skipped_stage(
            &audit_report,
            &audit_log,
            "micro mode skips audit collection; it only records the window and episode memory.",
        )?;
        write_micro_improve_outputs(&improve_dir, &improve_report, &improve_log)?;
    } else {
        let mut audit_args = vec!["audit".to_string(), "collect".to_string()];
        for root in &effective_audit_roots {
            audit_args.push("--from".to_string());
            audit_args.push(root.display().to_string());
        }
        audit_args.push("--out-dir".to_string());
        audit_args.push(audit_dir.display().to_string());
        audit_args.push("--report".to_string());
        audit_args.push(audit_report.display().to_string());
        run_air_stage(&cwd, &audit_args, &audit_log)?;

        let mut improve_args = vec!["improve".to_string()];
        for root in &effective_improve_roots {
            improve_args.push("--from".to_string());
            improve_args.push(root.display().to_string());
        }
        improve_args.push("--out-dir".to_string());
        improve_args.push(improve_dir.display().to_string());
        if options.write_regressions {
            improve_args.push("--write-regressions".to_string());
        }
        run_air_stage(&cwd, &improve_args, &improve_log)?;
        validate_dream_stage_outputs(&audit_dir, &improve_dir)?;
    }

    let findings_file = improve_dir.join("findings.json");
    let findings = read_findings(&findings_file)?;
    let finding_store = update_finding_store(
        &cwd,
        &out_dir,
        &findings,
        &improve_dir,
        options.write_regressions,
        started_at,
    )?;
    let memory_out_dir = out_dir.join("memory");
    let memory = extract_memory(MemoryExtractOptions {
        memory_dir: None,
        out_dir: memory_out_dir,
        window_manifest: window_manifest.clone(),
        observations_file: improve_dir.join("observations.json"),
        findings_file: findings_file.clone(),
        suggested_regressions_file: Some(improve_dir.join("suggested_regressions.json")),
        mode: options.mode.as_str().to_string(),
    })?;
    let memory_advance_out_dir = out_dir.join("memory-advance");
    let memory_advance = if options.advance && options.mode != DreamMode::Micro {
        advance_memory(MemoryAdvanceOptions {
            memory_dir: None,
            out_dir: Some(memory_advance_out_dir.clone()),
            skill_bench: false,
            limit: options.advance_limit,
            timeout_seconds: options.advance_timeout_seconds,
        })?
    } else {
        let status = if options.mode == DreamMode::Micro {
            "skipped_micro"
        } else {
            "skipped"
        };
        crate::memory::MemoryAdvanceOutput::skipped(&memory_advance_out_dir, status)?
    };
    let experiment = if options.experiment {
        run_dream_experiments(
            &cwd,
            &out_dir,
            &run_id,
            &window_manifest,
            &roots,
            &findings,
            &improve_dir,
            options.top_findings,
            options.candidates,
            options.budget_seconds,
            started_at,
        )?
    } else {
        DreamExperimentStage {
            status: "not_requested".to_string(),
            requested: false,
            top_findings: options.top_findings,
            budget_seconds: options.budget_seconds,
            attempted: 0,
            results: Vec::new(),
        }
    };
    let top_finding = findings.first().map(finding_summary);
    let next_commands = dream_next_commands(
        &roots,
        &audit_roots,
        &improve_roots,
        &out_dir,
        options.write_regressions,
        top_finding.as_ref(),
    );
    let dream_json = out_dir.join("dream.json");
    let dream_report = out_dir.join("dream.md");
    let output = DreamRunOutput {
        schema: DREAM_SCHEMA,
        status: if findings.is_empty() {
            "no_findings".to_string()
        } else {
            "findings_ready".to_string()
        },
        run_id: run_id.clone(),
        mode: options.mode.as_str().to_string(),
        incremental,
        incremental_source,
        since_unix,
        state_file: state_file.display().to_string(),
        out_dir: out_dir.display().to_string(),
        window_manifest: window_manifest.display().to_string(),
        audit: DreamStage {
            status: if options.mode == DreamMode::Micro {
                "skipped_micro".to_string()
            } else {
                "completed".to_string()
            },
            out_dir: audit_dir.display().to_string(),
            report: audit_report.display().to_string(),
            log: audit_log.display().to_string(),
        },
        improve: DreamStage {
            status: if options.mode == DreamMode::Micro {
                "skipped_micro".to_string()
            } else {
                "completed".to_string()
            },
            out_dir: improve_dir.display().to_string(),
            report: improve_report.display().to_string(),
            log: improve_log.display().to_string(),
        },
        memory: DreamMemoryStage {
            status: memory.status,
            memory_dir: memory.memory_dir,
            report: memory.report,
            episodes_written: memory.episodes_written,
            cards_written: memory.cards_written,
            failure_candidates: memory.failure_candidates,
            procedure_candidates: memory.procedure_candidates,
            routing_hints: memory.routing_hints,
            concept_candidates: memory.concept_candidates,
            hypothesis_candidates: memory.hypothesis_candidates,
            policy_candidates: memory.policy_candidates,
            graph_edges: memory.graph_edges,
            dream_ir_file: memory.dream_ir_file,
        },
        memory_advance: DreamMemoryAdvanceStage {
            status: memory_advance.status,
            out_dir: memory_advance.out_dir,
            promoted_memories: memory_advance.promoted_memories.len(),
            validated_memories: memory_advance.validated_memories.len(),
            retired_memories: memory_advance.retired_memories.len(),
            validated_skills: memory_advance.validated_skills.len(),
            routing_measurements: memory_advance.routing_measurements.len(),
            reviewed_guards: memory_advance.reviewed_guards.len(),
            report: memory_advance_out_dir
                .join("advance.md")
                .display()
                .to_string(),
        },
        finding_store,
        experiment,
        window_inputs: improve_roots.len(),
        window_artifacts: audit_roots.len(),
        max_input_mtime_unix,
        findings: findings.len(),
        top_finding: top_finding.clone(),
        dream_json: dream_json.display().to_string(),
        dream_report: dream_report.display().to_string(),
        next_commands,
    };
    fs::write(&dream_json, serde_json::to_string_pretty(&output)?)
        .with_context(|| format!("write {}", dream_json.display()))?;
    write_dream_report(&dream_report, &output)?;
    if default_out_dir {
        update_latest_dream_link(&cwd, &out_dir)?;
    }
    let completed_at = unix_now();
    let state_window = if window_inputs.is_empty() {
        prior_state
            .as_ref()
            .and_then(|state| state.last_window.clone())
    } else {
        Some(DreamWindowState {
            from_unix: since_unix,
            to_unix: started_at,
            max_input_mtime_unix,
            inputs: window_inputs.clone(),
        })
    };
    let root_cursors = update_root_cursors(
        &cwd,
        &roots,
        prior_state.as_ref(),
        &window_inputs,
        completed_at,
    );
    write_dream_state(
        &state_file,
        &DreamState {
            schema: DREAM_STATE_SCHEMA.to_string(),
            status: output.status.clone(),
            state_file: state_file.display().to_string(),
            last_started_at_unix: Some(started_at),
            last_completed_at_unix: Some(completed_at),
            last_out_dir: Some(out_dir.display().to_string()),
            last_roots: roots
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            last_findings: output.findings,
            last_top_finding: output.top_finding.clone(),
            last_window: state_window,
            root_cursors,
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) fn show_dream_state(_options: DreamStateOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let state_file = AirLayout::new(cwd).dream_state_path();
    let state = read_dream_state(&state_file)?.unwrap_or_else(|| DreamState {
        schema: DREAM_STATE_SCHEMA.to_string(),
        status: "never_run".to_string(),
        state_file: state_file.display().to_string(),
        last_started_at_unix: None,
        last_completed_at_unix: None,
        last_out_dir: None,
        last_roots: Vec::new(),
        last_findings: 0,
        last_top_finding: None,
        last_window: None,
        root_cursors: Vec::new(),
    });
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

pub(crate) fn list_dream_findings(options: DreamFindingsListOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let findings_file = AirLayout::new(cwd).dream_findings_path();
    let mut findings = read_latest_finding_records(&findings_file)?;
    if let Some(status) = options.status.as_deref() {
        findings.retain(|finding| finding.status == status);
    }
    findings.sort_by(|left, right| {
        right
            .impact_score
            .cmp(&left.impact_score)
            .then_with(|| right.recurrence_count.cmp(&left.recurrence_count))
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = options.limit {
        findings.truncate(limit);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&DreamFindingsListOutput {
            schema: "air.dream_findings.v1",
            findings_file: findings_file.display().to_string(),
            findings,
        })?
    );
    Ok(())
}

pub(crate) fn open_dream_finding(options: DreamFindingUpdateOptions) -> Result<()> {
    update_dream_finding_status(options, "open")
}

pub(crate) fn resolve_dream_finding(options: DreamFindingUpdateOptions) -> Result<()> {
    update_dream_finding_status(options, "resolved")
}

pub(crate) fn dismiss_dream_finding(options: DreamFindingUpdateOptions) -> Result<()> {
    update_dream_finding_status(options, "dismissed")
}

fn run_air_stage(cwd: &Path, args: &[String], log_path: &Path) -> Result<()> {
    let output = run_current_air_stage(cwd, args, log_path, None)?;
    if !output.success {
        bail!(
            "dream stage failed: air {} ({}); see {}",
            args.join(" "),
            output.status,
            log_path.display()
        );
    }
    Ok(())
}

fn run_dream_experiments(
    cwd: &Path,
    out_dir: &Path,
    run_id: &str,
    window_manifest: &Path,
    roots: &[PathBuf],
    findings: &[Value],
    improve_dir: &Path,
    top_findings: usize,
    candidates: usize,
    budget_seconds: Option<u64>,
    now: u64,
) -> Result<DreamExperimentStage> {
    let count = top_findings;
    let candidate_count = candidates;
    let budget = budget_seconds.map(Duration::from_secs);
    let started = Instant::now();
    let mut results = Vec::new();
    let window_manifest_sha = file_fingerprint(window_manifest)?;
    for finding in findings.iter().take(count) {
        if let Some(budget) = budget {
            if started.elapsed() >= budget {
                break;
            }
        }
        let summary = finding_summary(finding);
        let regression_file = improve_dir
            .join("suggested-regressions")
            .join(format!("{}.json", summary.id.to_ascii_lowercase()));
        if !regression_file.exists() {
            results.push(DreamExperimentResult {
                finding: summary.id.clone(),
                dream_run_id: run_id.to_string(),
                finding_stable_key: summary.stable_key.clone(),
                window_manifest_sha: window_manifest_sha.clone(),
                status: "missing_regression_file".to_string(),
                self_fix_log: None,
                compare_log: None,
                candidates_dir: out_dir
                    .join("candidates")
                    .join(&summary.id)
                    .display()
                    .to_string(),
                report: None,
            });
            append_dream_ledger(
                cwd,
                "experiment_skipped",
                &summary.persistent_id,
                now,
                Some(regression_file.display().to_string()),
                Some("missing_regression_file".to_string()),
                Some("run dream with --write-regressions before --experiment".to_string()),
            )?;
            continue;
        }
        let regression_kind = read_regression_kind(&regression_file)?;
        if regression_kind != "code_run_verdict" {
            let status = format!("unsupported_regression_kind_{regression_kind}");
            results.push(DreamExperimentResult {
                finding: summary.id.clone(),
                dream_run_id: run_id.to_string(),
                finding_stable_key: summary.stable_key.clone(),
                window_manifest_sha: window_manifest_sha.clone(),
                status: status.clone(),
                self_fix_log: None,
                compare_log: None,
                candidates_dir: out_dir
                    .join("candidates")
                    .join(&summary.id)
                    .display()
                    .to_string(),
                report: None,
            });
            append_dream_ledger(
                cwd,
                "experiment_skipped",
                &summary.persistent_id,
                now,
                Some(regression_file.display().to_string()),
                Some(status),
                Some(
                    "dream experiment only runs executable code_run_verdict regressions"
                        .to_string(),
                ),
            )?;
            continue;
        }
        let remaining = budget.map(|budget| budget.saturating_sub(started.elapsed()));
        let fix_log = out_dir
            .join("logs")
            .join(format!("experiment-{}-self-fix.log", summary.id));
        let compare_log = out_dir
            .join("logs")
            .join(format!("experiment-{}-compare.log", summary.id));
        let candidates_dir = out_dir.join("candidates");
        let provenance_file = candidates_dir
            .join(&summary.id)
            .join("dream_provenance.json");
        let mut fix_args = vec![
            "self".to_string(),
            "fix".to_string(),
            summary.id.clone(),
            "--candidates".to_string(),
            candidate_count.to_string(),
            "--out-dir".to_string(),
            candidates_dir.display().to_string(),
            "--regression-file".to_string(),
            regression_file.display().to_string(),
            "--evaluate".to_string(),
        ];
        for root in roots {
            fix_args.push("--from".to_string());
            fix_args.push(root.display().to_string());
        }
        let fix_status = run_air_stage_with_timeout(cwd, &fix_args, &fix_log, remaining)?;
        write_experiment_provenance(
            &provenance_file,
            run_id,
            &summary,
            &window_manifest_sha,
            window_manifest,
            &regression_file,
        )?;
        let mut status = if fix_status == "completed" {
            "self_fix_completed".to_string()
        } else {
            fix_status
        };
        if status != "self_fix_completed" {
            append_dream_ledger(
                cwd,
                "experiment_ran",
                &summary.persistent_id,
                now,
                Some(
                    out_dir
                        .join("candidates")
                        .join(&summary.id)
                        .display()
                        .to_string(),
                ),
                Some(status.clone()),
                Some("self_fix did not complete; compare skipped".to_string()),
            )?;
            results.push(DreamExperimentResult {
                finding: summary.id,
                dream_run_id: run_id.to_string(),
                finding_stable_key: summary.stable_key,
                window_manifest_sha: window_manifest_sha.clone(),
                status,
                self_fix_log: Some(fix_log.display().to_string()),
                compare_log: None,
                candidates_dir: out_dir
                    .join("candidates")
                    .join(&finding_summary(finding).id)
                    .display()
                    .to_string(),
                report: None,
            });
            continue;
        }
        let report = out_dir
            .join("candidates")
            .join(format!("{}.md", summary.id));
        let compare_args = vec![
            "self".to_string(),
            "compare".to_string(),
            summary.id.clone(),
            "--from".to_string(),
            out_dir
                .join("candidates")
                .join(&summary.id)
                .display()
                .to_string(),
            "--out".to_string(),
            report.display().to_string(),
        ];
        let compare_status = run_air_stage_with_timeout(
            cwd,
            &compare_args,
            &compare_log,
            budget.map(|budget| budget.saturating_sub(started.elapsed())),
        )?;
        if compare_status != "completed" {
            status = compare_status;
        }
        append_dream_ledger(
            cwd,
            "experiment_ran",
            &summary.persistent_id,
            now,
            Some(
                out_dir
                    .join("candidates")
                    .join(&summary.id)
                    .display()
                    .to_string(),
            ),
            Some(status.clone()),
            None,
        )?;
        link_finding_candidate(
            &AirLayout::new(cwd.to_path_buf()).dream_findings_path(),
            &summary.stable_key,
            out_dir
                .join("candidates")
                .join(&summary.id)
                .display()
                .to_string(),
        )?;
        results.push(DreamExperimentResult {
            finding: summary.id,
            dream_run_id: run_id.to_string(),
            finding_stable_key: summary.stable_key,
            window_manifest_sha: window_manifest_sha.clone(),
            status,
            self_fix_log: Some(fix_log.display().to_string()),
            compare_log: Some(compare_log.display().to_string()),
            candidates_dir: out_dir
                .join("candidates")
                .join(&finding_summary(finding).id)
                .display()
                .to_string(),
            report: Some(report.display().to_string()),
        });
    }
    Ok(DreamExperimentStage {
        status: if results.is_empty() {
            "no_experiments_run".to_string()
        } else {
            "completed".to_string()
        },
        requested: true,
        top_findings,
        budget_seconds,
        attempted: results.len(),
        results,
    })
}

fn run_air_stage_with_timeout(
    cwd: &Path,
    args: &[String],
    log_path: &Path,
    timeout: Option<Duration>,
) -> Result<String> {
    Ok(run_current_air_stage(cwd, args, log_path, timeout)?.status)
}

fn write_experiment_provenance(
    path: &Path,
    run_id: &str,
    summary: &DreamFindingSummary,
    window_manifest_sha: &str,
    window_manifest: &Path,
    regression_file: &Path,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "air.dream_candidate_provenance.v1",
            "dream_run_id": run_id,
            "finding_display_id": summary.id,
            "finding_stable_key": summary.stable_key,
            "finding_persistent_id": summary.persistent_id,
            "window_manifest": window_manifest.display().to_string(),
            "window_manifest_sha": window_manifest_sha,
            "regression_file": regression_file.display().to_string()
        }))?,
    )
    .with_context(|| format!("write {}", path.display()))
}

fn read_findings(path: &Path) -> Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    Ok(value
        .get("findings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn validate_dream_stage_outputs(audit_dir: &Path, improve_dir: &Path) -> Result<()> {
    validate_json_schema(&audit_dir.join("collection.json"), "air.audit_collection.v")?;
    validate_json_schema(
        &improve_dir.join("observations.json"),
        "air.improve_observations.v",
    )?;
    validate_json_schema(&improve_dir.join("findings.json"), "air.improve_findings.v")?;
    validate_json_schema(
        &improve_dir.join("suggested_regressions.json"),
        "air.improve_suggested_regressions.v",
    )
}

fn validate_json_schema(path: &Path, expected_prefix: &str) -> Result<()> {
    if !path.exists() {
        bail!(
            "dream stage incomplete: expected {} with schema prefix {}",
            path.display(),
            expected_prefix
        );
    }
    let value: Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    let schema = value.get("schema").and_then(Value::as_str);
    if !schema.is_some_and(|schema| schema.starts_with(expected_prefix)) {
        bail!(
            "dream stage incomplete: {} schema {:?}, expected prefix {}",
            path.display(),
            schema,
            expected_prefix
        );
    }
    Ok(())
}

fn read_regression_kind(path: &Path) -> Result<String> {
    let value: Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    Ok(value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string())
}

fn ensure_clean_experiment_workspace(cwd: &Path, ignored_out_dir: Option<&Path>) -> Result<()> {
    let tracked = Command::new("git")
        .args(["diff", "--quiet", "HEAD", "--"])
        .current_dir(cwd)
        .status()
        .context("check tracked workspace diff before dream experiment")?;
    let staged = Command::new("git")
        .args(["diff", "--cached", "--quiet", "--"])
        .current_dir(cwd)
        .status()
        .context("check staged workspace diff before dream experiment")?;
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(cwd)
        .output()
        .context("check untracked files before dream experiment")?;
    let untracked_stdout = String::from_utf8_lossy(&untracked.stdout);
    let dream_output_root = cwd.join(".air/dream");
    let untracked_blockers = untracked_stdout
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .filter(|path| {
            let absolute = absolutize(cwd, PathBuf::from(path));
            !path_is_under(&absolute, &dream_output_root)
                && ignored_out_dir.is_none_or(|out_dir| !path_is_under(&absolute, out_dir))
        })
        .collect::<Vec<_>>();
    if !tracked.success() || !staged.success() || !untracked_blockers.is_empty() {
        bail!(
            "dream experiment requires a clean git workspace; commit or stash tracked, staged, and untracked changes before running --experiment"
        );
    }
    Ok(())
}

fn write_skipped_stage(report: &Path, log: &Path, reason: &str) -> Result<()> {
    if let Some(parent) = report.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(report, format!("# Skipped\n\n{reason}\n"))
        .with_context(|| format!("write {}", report.display()))?;
    fs::write(log, format!("stage skipped: {reason}\n"))
        .with_context(|| format!("write {}", log.display()))
}

fn write_micro_improve_outputs(improve_dir: &Path, report: &Path, log: &Path) -> Result<()> {
    fs::create_dir_all(improve_dir).with_context(|| format!("create {}", improve_dir.display()))?;
    fs::write(
        improve_dir.join("observations.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "air.improve_observations.v1",
            "observations": []
        }))?,
    )
    .with_context(|| format!("write {}", improve_dir.join("observations.json").display()))?;
    fs::write(
        improve_dir.join("findings.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "air.improve_findings.v1",
            "findings": []
        }))?,
    )
    .with_context(|| format!("write {}", improve_dir.join("findings.json").display()))?;
    fs::write(
        improve_dir.join("suggested_regressions.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "air.improve_suggested_regressions.v1",
            "regressions": []
        }))?,
    )
    .with_context(|| {
        format!(
            "write {}",
            improve_dir.join("suggested_regressions.json").display()
        )
    })?;
    write_skipped_stage(
        report,
        log,
        "micro mode skips improve mining; it only records the selected window and episode memory.",
    )
}

fn update_finding_store(
    cwd: &Path,
    out_dir: &Path,
    findings: &[Value],
    improve_dir: &Path,
    wrote_regressions: bool,
    now: u64,
) -> Result<DreamFindingStoreStage> {
    let findings_file = AirLayout::new(cwd.to_path_buf()).dream_findings_path();
    let mut records = read_latest_finding_records(&findings_file)?;
    let mut opened = 0;
    let mut recurred = 0;
    for finding in findings {
        let summary = finding_summary(finding);
        let linked_regression = if wrote_regressions {
            Some(
                improve_dir
                    .join("suggested-regressions")
                    .join(format!("{}.json", summary.id.to_ascii_lowercase()))
                    .display()
                    .to_string(),
            )
        } else {
            None
        };
        let display_alias = summary.id.clone();
        let run_alias = format!("{} from {}", summary.id, out_dir.display());
        let Some(existing) = records
            .iter_mut()
            .find(|record| record_matches_finding(record, &summary.stable_key))
        else {
            let record = DreamFindingRecord {
                schema: DREAM_FINDING_RECORD_SCHEMA.to_string(),
                id: summary.persistent_id.clone(),
                stable_key: summary.stable_key.clone(),
                display_id: summary.id.clone(),
                display_rank: summary.display_rank,
                aliases: vec![display_alias, run_alias],
                category: summary.category.clone(),
                summary: summary.summary.clone(),
                impact_score: summary.impact_score,
                status: "open".to_string(),
                first_seen_unix: now,
                last_seen_unix: now,
                recurrence_count: 1,
                seen_in: vec![out_dir.display().to_string()],
                linked_regressions: linked_regression.into_iter().collect(),
                linked_candidates: Vec::new(),
                resolved_at_unix: None,
                dismissed_at_unix: None,
                fixed_by: None,
                recurred_after_fix: false,
                reason: None,
            };
            append_finding_record(&findings_file, &record)?;
            append_dream_ledger(
                cwd,
                "finding_observed",
                &record.id,
                now,
                Some(out_dir.display().to_string()),
                Some(record.status.clone()),
                Some(record.summary.clone()),
            )?;
            records.push(record);
            opened += 1;
            continue;
        };
        if matches!(existing.status.as_str(), "resolved" | "dismissed") {
            existing.recurred_after_fix = true;
            existing.status = "open".to_string();
            existing.resolved_at_unix = None;
            existing.dismissed_at_unix = None;
            recurred += 1;
        }
        existing.category = summary.category;
        existing.summary = summary.summary;
        existing.impact_score = summary.impact_score;
        existing.display_id = summary.id.clone();
        existing.display_rank = summary.display_rank;
        if existing.stable_key.is_empty() {
            existing.stable_key = summary.stable_key.clone();
        }
        push_unique_capped(
            &mut existing.aliases,
            display_alias,
            DREAM_FINDING_LIST_LIMIT,
        );
        push_unique_capped(&mut existing.aliases, run_alias, DREAM_FINDING_LIST_LIMIT);
        existing.last_seen_unix = now;
        existing.recurrence_count = existing.recurrence_count.saturating_add(1);
        push_unique_capped(
            &mut existing.seen_in,
            out_dir.display().to_string(),
            DREAM_FINDING_LIST_LIMIT,
        );
        if let Some(regression) = linked_regression {
            push_unique_capped(
                &mut existing.linked_regressions,
                regression,
                DREAM_FINDING_LIST_LIMIT,
            );
        }
        append_finding_record(&findings_file, existing)?;
        append_dream_ledger(
            cwd,
            "finding_observed",
            &existing.id,
            now,
            Some(out_dir.display().to_string()),
            Some(existing.status.clone()),
            Some(existing.summary.clone()),
        )?;
    }
    Ok(DreamFindingStoreStage {
        status: if findings.is_empty() {
            "no_findings".to_string()
        } else {
            "updated".to_string()
        },
        path: findings_file.display().to_string(),
        observed: findings.len(),
        opened,
        recurred,
    })
}

fn update_dream_finding_status(options: DreamFindingUpdateOptions, status: &str) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let findings_file = AirLayout::new(cwd.clone()).dream_findings_path();
    let mut records = read_latest_finding_records(&findings_file)?;
    let Some(index) = find_unique_record_index(&records, &options.finding)? else {
        bail!("dream finding not found: {}", options.finding);
    };
    let record = &mut records[index];
    let now = unix_now();
    record.status = status.to_string();
    record.reason = options.reason.clone();
    match status {
        "open" => {
            record.resolved_at_unix = None;
            record.dismissed_at_unix = None;
        }
        "resolved" => {
            record.resolved_at_unix = Some(now);
            record.dismissed_at_unix = None;
            record.fixed_by = options.fixed_by.clone();
        }
        "dismissed" => {
            record.dismissed_at_unix = Some(now);
            record.resolved_at_unix = None;
        }
        _ => {}
    }
    append_finding_record(&findings_file, record)?;
    append_dream_ledger(
        &cwd,
        &format!("finding_{status}"),
        &record.id,
        now,
        None,
        Some(status.to_string()),
        options.reason,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&DreamFindingUpdateOutput {
            schema: "air.dream_finding_update.v1",
            status: status.to_string(),
            findings_file: findings_file.display().to_string(),
            finding: record.clone(),
        })?
    );
    Ok(())
}

fn read_latest_finding_records(path: &Path) -> Result<Vec<DreamFindingRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut records = Vec::<DreamFindingRecord>::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut record: DreamFindingRecord =
            serde_json::from_str(line).with_context(|| format!("parse {}", path.display()))?;
        normalize_finding_record(&mut record);
        if let Some(existing) = records
            .iter_mut()
            .find(|existing| same_finding_record(existing, &record))
        {
            *existing = record;
        } else {
            records.push(record);
        }
    }
    Ok(records)
}

fn append_finding_record(path: &Path, record: &DreamFindingRecord) -> Result<()> {
    let _lock = FindingStoreLock::acquire(path)?;
    append_jsonl(path, record)?;
    if fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        > DREAM_FINDINGS_COMPACT_BYTES
    {
        compact_finding_records(path)?;
    }
    Ok(())
}

struct FindingStoreLock {
    path: PathBuf,
}

impl FindingStoreLock {
    fn acquire(path: &Path) -> Result<Self> {
        let lock_path = path.with_extension("jsonl.lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        for _ in 0..50 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { path: lock_path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    remove_stale_finding_lock(&lock_path);
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("create {}", lock_path.display()));
                }
            }
        }
        bail!(
            "timed out waiting for finding store lock {}",
            lock_path.display()
        )
    }
}

impl Drop for FindingStoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn remove_stale_finding_lock(path: &Path) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return;
    };
    if age > Duration::from_secs(300) {
        let _ = fs::remove_file(path);
    }
}

fn compact_finding_records(path: &Path) -> Result<()> {
    with_jsonl_lock(path, || {
        let records = read_latest_finding_records(path)?;
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend_from_slice(serde_json::to_string(&record)?.as_bytes());
            bytes.push(b'\n');
        }
        write_json_atomic(path, &bytes)
    })
}

fn append_dream_ledger(
    cwd: &Path,
    event: &str,
    finding: &str,
    at_unix: u64,
    path: Option<String>,
    status: Option<String>,
    note: Option<String>,
) -> Result<()> {
    append_jsonl(
        &AirLayout::new(cwd.to_path_buf()).dream_ledger_path(),
        &DreamLedgerEvent {
            schema: DREAM_LEDGER_SCHEMA,
            event: event.to_string(),
            finding: finding.to_string(),
            at_unix,
            path,
            status,
            note,
        },
    )
}

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    append_jsonl_locked(path, value)
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn push_unique_capped(values: &mut Vec<String>, value: String, limit: usize) {
    push_unique(values, value);
    if values.len() > limit {
        values.drain(0..values.len() - limit);
    }
}

fn normalize_finding_record(record: &mut DreamFindingRecord) {
    if record.stable_key.is_empty() {
        record.stable_key = record.id.clone();
    }
    if record.display_id.is_empty() {
        record.display_id = record.id.clone();
    }
    if record.aliases.is_empty() && record.display_id != record.id {
        record.aliases.push(record.display_id.clone());
    }
    if record.aliases.len() > DREAM_FINDING_LIST_LIMIT {
        record
            .aliases
            .drain(0..record.aliases.len() - DREAM_FINDING_LIST_LIMIT);
    }
    if record.linked_candidates.len() > DREAM_FINDING_LIST_LIMIT {
        record
            .linked_candidates
            .drain(0..record.linked_candidates.len() - DREAM_FINDING_LIST_LIMIT);
    }
    if record.linked_regressions.len() > DREAM_FINDING_LIST_LIMIT {
        record
            .linked_regressions
            .drain(0..record.linked_regressions.len() - DREAM_FINDING_LIST_LIMIT);
    }
    if record.seen_in.len() > DREAM_FINDING_LIST_LIMIT {
        record
            .seen_in
            .drain(0..record.seen_in.len() - DREAM_FINDING_LIST_LIMIT);
    }
}

fn same_finding_record(left: &DreamFindingRecord, right: &DreamFindingRecord) -> bool {
    if !left.stable_key.is_empty() && !right.stable_key.is_empty() {
        return left.stable_key == right.stable_key;
    }
    left.id == right.id
}

fn record_matches_finding(record: &DreamFindingRecord, stable_key: &str) -> bool {
    record.stable_key == stable_key || (record.stable_key.is_empty() && record.id == stable_key)
}

fn record_matches_query(record: &DreamFindingRecord, query: &str) -> bool {
    record.id == query
        || record.stable_key == query
        || record.display_id == query
        || record.aliases.iter().any(|alias| alias == query)
}

fn find_unique_record_index(records: &[DreamFindingRecord], query: &str) -> Result<Option<usize>> {
    let matches = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| record_matches_query(record, query).then_some(index))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [index] => Ok(Some(*index)),
        _ => bail!(
            "dream finding query `{query}` is ambiguous; use the stable_key or FND-* id from `air dream findings list`"
        ),
    }
}

fn link_finding_candidate(path: &Path, finding: &str, candidate: String) -> Result<()> {
    let mut records = read_latest_finding_records(path)?;
    let Some(index) = find_unique_record_index(&records, finding)? else {
        return Ok(());
    };
    let record = &mut records[index];
    push_unique_capped(
        &mut record.linked_candidates,
        candidate,
        DREAM_FINDING_LIST_LIMIT,
    );
    append_finding_record(path, record)
}

fn discover_dream_window_inputs(
    cwd: &Path,
    roots: &[PathBuf],
    since_unix: Option<u64>,
    limit: Option<usize>,
    dedupe_previous_window: bool,
    prior_state: Option<&DreamState>,
    full: bool,
) -> Result<Vec<DreamWindowInput>> {
    let mut files = Vec::new();
    for root in roots {
        let root = absolutize(cwd, root.clone());
        let root_since = if full {
            None
        } else {
            since_unix.or_else(|| root_cursor_unix(prior_state, &root))
        };
        collect_improve_input_files(&root, root_since, &mut files)?;
    }
    files.sort_by(|left, right| {
        file_mtime_unix(right)
            .cmp(&file_mtime_unix(left))
            .then_with(|| left.cmp(right))
    });
    files.dedup();
    if let Some(since_unix) = since_unix {
        files.retain(|path| file_mtime_unix(path).unwrap_or(0) >= since_unix);
    }
    let previous_keys = if dedupe_previous_window {
        prior_state
            .and_then(|state| state.last_window.as_ref())
            .map(|window| {
                window
                    .inputs
                    .iter()
                    .map(window_input_key)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default()
    } else {
        BTreeSet::new()
    };
    let mut inputs = Vec::new();
    for path in files {
        let Some(mtime_unix) = file_mtime_unix(&path) else {
            continue;
        };
        let input = DreamWindowInput {
            kind: dream_input_kind(&path).to_string(),
            path: path.display().to_string(),
            fingerprint: file_fingerprint(&path)?,
            mtime_unix,
        };
        if previous_keys.contains(&window_input_key(&input)) {
            continue;
        }
        inputs.push(input);
    }
    if let Some(limit) = limit {
        inputs.truncate(limit);
    }
    Ok(inputs)
}

fn collect_improve_input_files(
    root: &Path,
    since_unix: Option<u64>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if is_improve_input(root)
            && since_unix.is_none_or(|since| file_mtime_unix(root).unwrap_or(0) >= since)
        {
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
                ".git" | "node_modules" | ".venv" | ".cache" | "tmp"
            ) {
                continue;
            }
            collect_improve_input_files(&path, since_unix, files)?;
        } else if is_improve_input(&path)
            && since_unix.is_none_or(|since| file_mtime_unix(&path).unwrap_or(0) >= since)
        {
            files.push(path);
        }
    }
    Ok(())
}

fn is_improve_input(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("run.json" | "artifact.json")
    )
}

fn finding_summary(value: &Value) -> DreamFindingSummary {
    let id = string_field(value, "id").unwrap_or_else(|| "unknown".to_string());
    let category = string_field(value, "category").unwrap_or_else(|| "unknown".to_string());
    let summary = string_field(value, "summary").unwrap_or_else(|| "No summary".to_string());
    let stable_key = stable_finding_key(value, &category, &summary);
    DreamFindingSummary {
        persistent_id: persistent_finding_id(&stable_key),
        display_rank: parse_display_rank(&id),
        id,
        stable_key,
        category,
        summary,
        impact_score: value
            .get("impact_score")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    }
}

fn stable_finding_key(value: &Value, category: &str, summary: &str) -> String {
    let expected_regression = string_field(value, "expected_regression").unwrap_or_default();
    let recommended_change_type =
        string_field(value, "recommended_change_type").unwrap_or_default();
    let material = format!(
        "{}\n{}\n{}\n{}",
        normalize_key_part(category),
        normalize_key_part(&expected_regression),
        normalize_summary_template(summary),
        normalize_key_part(&recommended_change_type)
    );
    format!(
        "finding:{}:{}",
        normalize_key_part(category),
        &sha256_hex(material.as_bytes())[..16]
    )
}

fn persistent_finding_id(stable_key: &str) -> String {
    let suffix = stable_key.rsplit(':').next().unwrap_or(stable_key);
    format!("FND-{}", suffix.to_ascii_uppercase())
}

fn parse_display_rank(id: &str) -> Option<u64> {
    id.strip_prefix("IMP-")
        .and_then(|rank| rank.parse::<u64>().ok())
}

fn normalize_summary_template(value: &str) -> String {
    let mut out = String::new();
    let mut previous_space = false;
    let mut previous_digit = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_digit() {
            if !previous_digit {
                out.push('#');
            }
            previous_digit = true;
            previous_space = false;
        } else if ch.is_whitespace() {
            if !previous_space {
                out.push(' ');
            }
            previous_space = true;
            previous_digit = false;
        } else {
            out.push(ch);
            previous_space = false;
            previous_digit = false;
        }
    }
    out.trim().to_string()
}

fn normalize_key_part(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn dream_next_commands(
    roots: &[PathBuf],
    audit_roots: &[PathBuf],
    improve_roots: &[PathBuf],
    out_dir: &Path,
    wrote_regressions: bool,
    top_finding: Option<&DreamFindingSummary>,
) -> Vec<String> {
    let mut commands = Vec::new();
    if !audit_roots.is_empty() {
        commands.push(format!(
            "air audit collect --from {} --out-dir {}",
            roots_display(audit_roots),
            shell_quote_path(&out_dir.join("audit"))
        ));
    }
    if !improve_roots.is_empty() {
        commands.push(format!(
            "air improve --from {} --out-dir {}",
            roots_display(improve_roots),
            shell_quote_path(&out_dir.join("improve"))
        ));
    }
    if let Some(finding) = top_finding {
        let regression_file = if wrote_regressions {
            out_dir
                .join("improve")
                .join("suggested-regressions")
                .join(format!("{}.json", finding.id.to_ascii_lowercase()))
        } else {
            commands.push(format!(
                "air improve --out-dir {} promote {}",
                shell_quote_path(&out_dir.join("improve")),
                shell_quote_arg(&finding.id)
            ));
            PathBuf::from("skills/code-agent/benches/regressions")
                .join(format!("{}.json", finding.id.to_ascii_lowercase()))
        };
        commands.push(format!(
            "air self fix {} --from {} --out-dir {} --regression-file {} --evaluate",
            shell_quote_arg(&finding.id),
            roots_display(roots),
            shell_quote_path(&out_dir.join("candidates")),
            shell_quote_path(&regression_file)
        ));
        commands.push(format!(
            "air self compare {} --from {} --out {}",
            shell_quote_arg(&finding.id),
            shell_quote_path(&out_dir.join("candidates").join(&finding.id)),
            shell_quote_path(
                &out_dir
                    .join("candidates")
                    .join(format!("{}.md", finding.id))
            )
        ));
    }
    commands
}

fn write_dream_report(path: &Path, output: &DreamRunOutput) -> Result<()> {
    let mut out = String::new();
    out.push_str("# AIR Dream Report\n\n");
    out.push_str("Dream is an offline, auditable optimization cycle. It reviews recent run artifacts, mines real failure patterns, and prepares safe next steps without changing source code automatically.\n\n");
    out.push_str(&format!("- status: `{}`\n", output.status));
    out.push_str(&format!("- mode: `{}`\n", output.mode));
    out.push_str(&format!("- incremental: `{}`\n", output.incremental));
    out.push_str(&format!(
        "- incremental_source: `{}`\n",
        output.incremental_source
    ));
    out.push_str(&format!("- since_unix: `{:?}`\n", output.since_unix));
    out.push_str(&format!("- window_inputs: `{}`\n", output.window_inputs));
    out.push_str(&format!(
        "- window_artifacts: `{}`\n",
        output.window_artifacts
    ));
    out.push_str(&format!(
        "- max_input_mtime_unix: `{:?}`\n",
        output.max_input_mtime_unix
    ));
    out.push_str(&format!(
        "- window manifest: `{}`\n",
        output.window_manifest
    ));
    out.push_str(&format!("- findings: `{}`\n", output.findings));
    out.push_str(&format!(
        "- finding_store: `{}` observed `{}` opened `{}` recurred `{}`\n",
        output.finding_store.path,
        output.finding_store.observed,
        output.finding_store.opened,
        output.finding_store.recurred
    ));
    out.push_str(&format!(
        "- memory_cards: `{}`\n",
        output.memory.cards_written
    ));
    out.push_str(&format!(
        "- memory_episodes: `{}`\n",
        output.memory.episodes_written
    ));
    out.push_str(&format!("- state file: `{}`\n", output.state_file));
    out.push_str(&format!("- audit report: `{}`\n", output.audit.report));
    out.push_str(&format!(
        "- improve report: `{}`\n\n",
        output.improve.report
    ));
    out.push_str(&format!("- memory report: `{}`\n", output.memory.report));
    out.push_str(&format!(
        "- memory advance report: `{}`\n\n",
        output.memory_advance.report
    ));
    if let Some(finding) = &output.top_finding {
        out.push_str("## Top Finding\n\n");
        out.push_str(&format!("- id: `{}`\n", finding.id));
        out.push_str(&format!("- category: `{}`\n", finding.category));
        out.push_str(&format!("- impact_score: `{}`\n", finding.impact_score));
        out.push_str(&format!("- summary: {}\n\n", finding.summary));
    }
    out.push_str("## Memory Changes\n\n");
    out.push_str(&format!(
        "- status: `{}`\n- failure_candidates: `{}`\n- procedure_candidates: `{}`\n- routing_hints: `{}`\n- concept_candidates: `{}`\n- hypothesis_candidates: `{}`\n- policy_candidates: `{}`\n- graph_edges: `{}`\n- dream_ir: `{}`\n\n",
        output.memory.status,
        output.memory.failure_candidates,
        output.memory.procedure_candidates,
        output.memory.routing_hints,
        output.memory.concept_candidates,
        output.memory.hypothesis_candidates,
        output.memory.policy_candidates,
        output.memory.graph_edges,
        output.memory.dream_ir_file
    ));
    out.push_str("## Memory Advance\n\n");
    out.push_str(&format!(
        "- status: `{}`\n- promoted_memories: `{}`\n- validated_memories: `{}`\n- retired_memories: `{}`\n- validated_skills: `{}`\n- routing_measurements: `{}`\n- reviewed_guards: `{}`\n- report: `{}`\n\n",
        output.memory_advance.status,
        output.memory_advance.promoted_memories,
        output.memory_advance.validated_memories,
        output.memory_advance.retired_memories,
        output.memory_advance.validated_skills,
        output.memory_advance.routing_measurements,
        output.memory_advance.reviewed_guards,
        output.memory_advance.report
    ));
    out.push_str("## Finding Store\n\n");
    out.push_str(&format!(
        "- status: `{}`\n- path: `{}`\n- observed: `{}`\n- opened: `{}`\n- recurred: `{}`\n\n",
        output.finding_store.status,
        output.finding_store.path,
        output.finding_store.observed,
        output.finding_store.opened,
        output.finding_store.recurred
    ));
    out.push_str("## Experiments\n\n");
    out.push_str(&format!(
        "- status: `{}`\n- requested: `{}`\n- attempted: `{}`\n- budget_seconds: `{:?}`\n\n",
        output.experiment.status,
        output.experiment.requested,
        output.experiment.attempted,
        output.experiment.budget_seconds
    ));
    for result in &output.experiment.results {
        out.push_str(&format!(
            "- `{}` `{}` candidates `{}` report `{:?}`\n",
            result.finding, result.status, result.candidates_dir, result.report
        ));
    }
    if !output.experiment.results.is_empty() {
        out.push('\n');
    }
    out.push_str("## Next Commands\n\n");
    if output.next_commands.is_empty() {
        out.push_str("No follow-up commands.\n");
    } else {
        for command in &output.next_commands {
            out.push_str("```bash\n");
            out.push_str(command);
            out.push_str("\n```\n\n");
        }
    }
    fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

fn roots_display(roots: &[PathBuf]) -> String {
    if roots.is_empty() {
        return "target/generated".to_string();
    }
    roots
        .iter()
        .map(|path| shell_quote_path(path))
        .collect::<Vec<_>>()
        .join(" --from ")
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote_arg(&path.display().to_string())
}

fn shell_quote_arg(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn state_cursor_unix(state: &DreamState) -> Option<u64> {
    if state
        .root_cursors
        .iter()
        .any(|cursor| cursor.last_seen_max_mtime_unix.is_some())
    {
        return state
            .root_cursors
            .iter()
            .filter_map(|cursor| cursor.last_seen_max_mtime_unix)
            .min()
            .map(|cursor| cursor.saturating_sub(DREAM_CURSOR_OVERLAP_SECONDS));
    }
    match state.last_window.as_ref() {
        Some(window) => window
            .max_input_mtime_unix
            .map(|cursor| cursor.saturating_sub(DREAM_CURSOR_OVERLAP_SECONDS)),
        None => state
            .last_completed_at_unix
            .map(|cursor| cursor.saturating_sub(DREAM_CURSOR_OVERLAP_SECONDS)),
    }
}

fn root_cursor_unix(state: Option<&DreamState>, root: &Path) -> Option<u64> {
    let root = dream_root_key(root);
    state
        .and_then(|state| {
            state
                .root_cursors
                .iter()
                .find(|cursor| cursor.root == root)
                .and_then(|cursor| cursor.last_seen_max_mtime_unix)
        })
        .map(|cursor| cursor.saturating_sub(DREAM_CURSOR_OVERLAP_SECONDS))
}

fn update_root_cursors(
    cwd: &Path,
    roots: &[PathBuf],
    prior_state: Option<&DreamState>,
    inputs: &[DreamWindowInput],
    completed_at: u64,
) -> Vec<DreamRootCursor> {
    let mut cursors = prior_state
        .map(|state| {
            state
                .root_cursors
                .iter()
                .map(|cursor| (cursor.root.clone(), cursor.clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    for root in roots {
        let absolute = normalize_dream_root(&absolutize(cwd, root.clone()));
        let root_key = dream_root_key(&absolute);
        let root_exists = absolute.exists();
        let max_seen = inputs
            .iter()
            .filter(|input| path_is_under(Path::new(&input.path), &absolute))
            .map(|input| input.mtime_unix)
            .max();
        let mut cursor = cursors.remove(&root_key).unwrap_or(DreamRootCursor {
            root: root_key.clone(),
            root_exists,
            last_seen_max_mtime_unix: None,
            last_scan_completed_at_unix: None,
        });
        cursor.root_exists = root_exists;
        if let Some(max_seen) = max_seen {
            cursor.last_seen_max_mtime_unix =
                Some(cursor.last_seen_max_mtime_unix.unwrap_or(0).max(max_seen));
            cursor.last_scan_completed_at_unix = Some(completed_at);
        }
        cursors.insert(root_key, cursor);
    }
    cursors.into_values().collect()
}

fn normalize_dream_root(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn dream_root_key(path: &Path) -> String {
    normalize_dream_root(path).display().to_string()
}

fn path_is_under(path: &Path, root: &Path) -> bool {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = normalize_dream_root(root);
    path == root || path.starts_with(&root)
}

fn dream_input_kind(path: &Path) -> &'static str {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("artifact.json") => "code_run_artifact",
        Some("run.json") => "bench_run",
        _ => "unknown",
    }
}

fn file_fingerprint(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(format!("sha256:{}", sha256_hex(&bytes)))
}

fn window_input_key(input: &DreamWindowInput) -> String {
    format!("{}\0{}", input.path, input.fingerprint)
}

fn clear_dream_out_dir(out_dir: &Path) -> Result<()> {
    ensure_safe_dream_out_dir(out_dir)?;
    for child in [
        "audit",
        "improve",
        "logs",
        "memory",
        "memory-advance",
        "window",
        "candidates",
    ] {
        let path = out_dir.join(child);
        if path.exists() {
            fs::remove_dir_all(&path).with_context(|| format!("remove old {}", path.display()))?;
        }
    }
    for child in ["dream.json", "dream.md"] {
        let path = out_dir.join(child);
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("remove old {}", path.display()))?;
        }
    }
    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    fs::write(out_dir.join(DREAM_OUTPUT_MARKER), "air dream output\n")
        .with_context(|| format!("write {}", out_dir.join(DREAM_OUTPUT_MARKER).display()))?;
    Ok(())
}

fn ensure_safe_dream_out_dir(out_dir: &Path) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = Path::new("/");
    if out_dir == root || out_dir == cwd {
        bail!(
            "refusing unsafe dream --out-dir {}; choose a dedicated directory such as .air/dream/latest",
            out_dir.display()
        );
    }
    if out_dir.parent().is_none() {
        bail!(
            "refusing unsafe dream --out-dir {}; path has no parent",
            out_dir.display()
        );
    }
    let dream_root = cwd.join(".air").join("dream");
    let inside_default_dream_root = out_dir.starts_with(&dream_root) && out_dir != dream_root;
    let marker = out_dir.join(DREAM_OUTPUT_MARKER);
    let existing_empty = out_dir
        .read_dir()
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true);
    if inside_default_dream_root || marker.exists() || existing_empty {
        return Ok(());
    }
    bail!(
        "refusing to clean non-Dream output directory {}; use a new empty directory under .air/dream/ or a directory containing {}",
        out_dir.display(),
        DREAM_OUTPUT_MARKER
    )
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn read_dream_state(path: &Path) -> Result<Option<DreamState>> {
    if !path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(value))
}

fn write_dream_state(path: &Path, state: &DreamState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(state)?)
        .with_context(|| format!("write {}", path.display()))
}

fn file_mtime_unix(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn dream_run_id(started_at: u64) -> String {
    format!("dream-{started_at}-{}", std::process::id())
}

fn update_latest_dream_link(cwd: &Path, out_dir: &Path) -> Result<()> {
    let latest = cwd.join(".air/dream/latest");
    if let Some(parent) = latest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if latest.exists() || latest.symlink_metadata().is_ok() {
        let metadata = latest
            .symlink_metadata()
            .with_context(|| format!("stat {}", latest.display()))?;
        if !(metadata.file_type().is_symlink() || metadata.is_file()) {
            ensure_safe_dream_out_dir(&latest)?;
            fs::remove_dir_all(&latest).with_context(|| format!("remove {}", latest.display()))?;
        }
    }
    #[cfg(unix)]
    {
        let tmp = latest.with_extension(format!("tmp-{}", std::process::id()));
        if tmp.exists() || tmp.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&tmp);
        }
        std::os::unix::fs::symlink(out_dir, &tmp)
            .with_context(|| format!("symlink {} to {}", tmp.display(), out_dir.display()))?;
        fs::rename(&tmp, &latest)
            .with_context(|| format!("rename {} to {}", tmp.display(), latest.display()))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(latest.with_extension("txt"), out_dir.display().to_string())
            .with_context(|| format!("write {}", latest.with_extension("txt").display()))?;
    }
    Ok(())
}

fn absolutize(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "air-dream-test-{}-{}-{name}",
            std::process::id(),
            unix_now()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn state_cursor_uses_input_mtime_with_overlap() {
        let state = DreamState {
            schema: "air.dream_state.v1".to_string(),
            status: "findings_ready".to_string(),
            state_file: ".air/dream/state.json".to_string(),
            last_started_at_unix: Some(1_000),
            last_completed_at_unix: Some(2_000),
            last_out_dir: None,
            last_roots: Vec::new(),
            last_findings: 0,
            last_top_finding: None,
            last_window: Some(DreamWindowState {
                from_unix: None,
                to_unix: 1_000,
                max_input_mtime_unix: Some(1_500),
                inputs: Vec::new(),
            }),
            root_cursors: Vec::new(),
        };
        assert_eq!(
            state_cursor_unix(&state),
            Some(1_500 - DREAM_CURSOR_OVERLAP_SECONDS)
        );
    }

    #[test]
    fn state_cursor_does_not_advance_empty_window() {
        let state = DreamState {
            schema: "air.dream_state.v1".to_string(),
            status: "no_findings".to_string(),
            state_file: ".air/dream/state.json".to_string(),
            last_started_at_unix: Some(1_000),
            last_completed_at_unix: Some(2_000),
            last_out_dir: None,
            last_roots: Vec::new(),
            last_findings: 0,
            last_top_finding: None,
            last_window: Some(DreamWindowState {
                from_unix: None,
                to_unix: 1_000,
                max_input_mtime_unix: None,
                inputs: Vec::new(),
            }),
            root_cursors: Vec::new(),
        };

        assert_eq!(state_cursor_unix(&state), None);
    }

    #[test]
    fn root_cursor_uses_per_root_high_watermark() {
        let root = temp_root("root-cursor");
        let root_a = root.join("a");
        let root_b = root.join("b");
        let state = DreamState {
            schema: "air.dream_state.v1".to_string(),
            status: "findings_ready".to_string(),
            state_file: ".air/dream/state.json".to_string(),
            last_started_at_unix: Some(1_000),
            last_completed_at_unix: Some(2_000),
            last_out_dir: None,
            last_roots: Vec::new(),
            last_findings: 0,
            last_top_finding: None,
            last_window: None,
            root_cursors: vec![
                DreamRootCursor {
                    root: root_a.display().to_string(),
                    root_exists: true,
                    last_seen_max_mtime_unix: Some(1_500),
                    last_scan_completed_at_unix: Some(1_600),
                },
                DreamRootCursor {
                    root: root_b.display().to_string(),
                    root_exists: true,
                    last_seen_max_mtime_unix: Some(3_000),
                    last_scan_completed_at_unix: Some(3_100),
                },
            ],
        };

        assert_eq!(
            root_cursor_unix(Some(&state), &root_a),
            Some(1_500 - DREAM_CURSOR_OVERLAP_SECONDS)
        );
        assert_eq!(
            state_cursor_unix(&state),
            Some(1_500 - DREAM_CURSOR_OVERLAP_SECONDS)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn window_dedupes_previous_inputs_by_path_and_fingerprint() {
        let root = temp_root("dedupe");
        let artifact_dir = root.join("run-1");
        fs::create_dir_all(&artifact_dir).unwrap();
        let artifact = artifact_dir.join("artifact.json");
        fs::write(&artifact, br#"{"schema":"air.code_run_artifact.v1"}"#).unwrap();
        let previous = DreamWindowState {
            from_unix: None,
            to_unix: unix_now(),
            max_input_mtime_unix: file_mtime_unix(&artifact),
            inputs: vec![DreamWindowInput {
                kind: "code_run_artifact".to_string(),
                path: artifact.display().to_string(),
                fingerprint: file_fingerprint(&artifact).unwrap(),
                mtime_unix: file_mtime_unix(&artifact).unwrap(),
            }],
        };

        let inputs = discover_dream_window_inputs(
            &root,
            std::slice::from_ref(&root),
            None,
            None,
            true,
            Some(&DreamState {
                schema: DREAM_STATE_SCHEMA.to_string(),
                status: "findings_ready".to_string(),
                state_file: String::new(),
                last_started_at_unix: None,
                last_completed_at_unix: None,
                last_out_dir: None,
                last_roots: Vec::new(),
                last_findings: 0,
                last_top_finding: None,
                last_window: Some(previous),
                root_cursors: Vec::new(),
            }),
            false,
        )
        .unwrap();

        assert!(inputs.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn window_applies_limit_after_previous_dedupe() {
        let root = temp_root("dedupe-limit");
        let previous_dir = root.join("a-previous");
        let new_dir = root.join("z-new");
        fs::create_dir_all(&previous_dir).unwrap();
        fs::create_dir_all(&new_dir).unwrap();
        let previous_artifact = previous_dir.join("artifact.json");
        let new_artifact = new_dir.join("artifact.json");
        fs::write(
            &previous_artifact,
            br#"{"schema":"air.code_run_artifact.v1","id":"previous"}"#,
        )
        .unwrap();
        fs::write(
            &new_artifact,
            br#"{"schema":"air.code_run_artifact.v1","id":"new"}"#,
        )
        .unwrap();
        let previous = DreamWindowState {
            from_unix: None,
            to_unix: unix_now(),
            max_input_mtime_unix: file_mtime_unix(&previous_artifact),
            inputs: vec![DreamWindowInput {
                kind: "code_run_artifact".to_string(),
                path: previous_artifact.display().to_string(),
                fingerprint: file_fingerprint(&previous_artifact).unwrap(),
                mtime_unix: file_mtime_unix(&previous_artifact).unwrap(),
            }],
        };

        let inputs = discover_dream_window_inputs(
            &root,
            std::slice::from_ref(&root),
            None,
            Some(1),
            true,
            Some(&DreamState {
                schema: DREAM_STATE_SCHEMA.to_string(),
                status: "findings_ready".to_string(),
                state_file: String::new(),
                last_started_at_unix: None,
                last_completed_at_unix: None,
                last_out_dir: None,
                last_roots: Vec::new(),
                last_findings: 0,
                last_top_finding: None,
                last_window: Some(previous),
                root_cursors: Vec::new(),
            }),
            false,
        )
        .unwrap();

        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].path, new_artifact.display().to_string());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn next_commands_use_promoted_regression_after_manual_promote() {
        let finding = DreamFindingSummary {
            id: "IMP-001".to_string(),
            stable_key: "finding:verification_failed:test".to_string(),
            persistent_id: "FND-TEST".to_string(),
            display_rank: Some(1),
            category: "verification_failed".to_string(),
            summary: "summary".to_string(),
            impact_score: 10,
        };
        let commands = dream_next_commands(
            &[PathBuf::from("target/generated")],
            &[],
            &[],
            Path::new(".air/dream/latest"),
            false,
            Some(&finding),
        );

        assert!(commands
            .iter()
            .any(|command| command
                == "air improve --out-dir .air/dream/latest/improve promote IMP-001"));
        assert!(commands.iter().any(|command| command
            .contains("--regression-file skills/code-agent/benches/regressions/imp-001.json")));
        assert!(!commands
            .iter()
            .any(|command| command.contains("suggested-regressions/imp-001.json")));
    }

    #[test]
    fn next_commands_shell_quote_paths_with_spaces() {
        let finding = DreamFindingSummary {
            id: "IMP 001".to_string(),
            stable_key: "finding:verification_failed:test".to_string(),
            persistent_id: "FND-TEST".to_string(),
            display_rank: None,
            category: "verification_failed".to_string(),
            summary: "summary".to_string(),
            impact_score: 10,
        };
        let commands = dream_next_commands(
            &[PathBuf::from("target/generated with spaces")],
            &[PathBuf::from(
                "target/generated with spaces/run/artifact.json",
            )],
            &[],
            Path::new(".air/dream/latest with spaces"),
            true,
            Some(&finding),
        );

        assert!(commands
            .iter()
            .any(|command| command.contains("'target/generated with spaces/run/artifact.json'")));
        assert!(commands.iter().any(|command| command.contains("'IMP 001'")));
    }

    #[test]
    fn clear_dream_out_dir_rejects_non_dream_non_empty_directory() {
        let root = temp_root("unsafe-clear");
        fs::write(root.join("audit"), b"not a dream output").unwrap();
        let error = clear_dream_out_dir(&root).unwrap_err().to_string();

        assert!(error.contains("refusing to clean non-Dream output directory"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_findings_missing_file_is_empty() {
        let root = temp_root("missing-findings");
        let findings = read_findings(&root.join("findings.json")).unwrap();

        assert!(findings.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stage_schema_validation_rejects_missing_outputs() {
        let root = temp_root("stage-schema");
        let audit_dir = root.join("audit");
        let improve_dir = root.join("improve");
        fs::create_dir_all(&audit_dir).unwrap();
        fs::create_dir_all(&improve_dir).unwrap();
        fs::write(
            audit_dir.join("collection.json"),
            br#"{"schema":"air.audit_collection.v1"}"#,
        )
        .unwrap();

        let error = validate_dream_stage_outputs(&audit_dir, &improve_dir)
            .unwrap_err()
            .to_string();

        assert!(error.contains("dream stage incomplete"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn finding_store_tracks_recurrence_and_reopen() {
        let root = temp_root("finding-store");
        let out_dir = root.join(".air/dream/latest");
        let improve_dir = out_dir.join("improve");
        fs::create_dir_all(&improve_dir).unwrap();
        let findings = vec![serde_json::json!({
            "id": "IMP-001",
            "category": "verification_failed",
            "summary": "missing verification",
            "impact_score": 10
        })];

        update_finding_store(&root, &out_dir, &findings, &improve_dir, false, 1_000).unwrap();
        let findings_path = AirLayout::new(root.to_path_buf()).dream_findings_path();
        let mut records = read_latest_finding_records(&findings_path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "open");
        records[0].status = "resolved".to_string();
        records[0].resolved_at_unix = Some(1_100);
        append_finding_record(&findings_path, &records[0]).unwrap();

        let stage =
            update_finding_store(&root, &out_dir, &findings, &improve_dir, false, 1_200).unwrap();
        let records = read_latest_finding_records(&findings_path).unwrap();
        assert_eq!(stage.recurred, 1);
        assert_eq!(records[0].status, "open");
        assert!(records[0].recurred_after_fix);
        assert_eq!(records[0].recurrence_count, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn finding_store_uses_stable_key_not_display_id() {
        let root = temp_root("finding-stable-key");
        let out_dir = root.join(".air/dream/latest");
        let improve_dir = out_dir.join("improve");
        fs::create_dir_all(&improve_dir).unwrap();
        let first = vec![serde_json::json!({
            "id": "IMP-001",
            "category": "verification_failed",
            "summary": "1 run(s) failed because verification was missing or failed",
            "impact_score": 10,
            "expected_regression": "A verdict fixture fails when no passing verification is present after a patch.",
            "recommended_change_type": "verification_loop_regression"
        })];
        let second = vec![serde_json::json!({
            "id": "IMP-002",
            "category": "verification_failed",
            "summary": "2 run(s) failed because verification was missing or failed",
            "impact_score": 20,
            "expected_regression": "A verdict fixture fails when no passing verification is present after a patch.",
            "recommended_change_type": "verification_loop_regression"
        })];

        update_finding_store(&root, &out_dir, &first, &improve_dir, false, 1_000).unwrap();
        update_finding_store(&root, &out_dir, &second, &improve_dir, false, 1_200).unwrap();
        let findings_path = AirLayout::new(root.to_path_buf()).dream_findings_path();
        let records = read_latest_finding_records(&findings_path).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].display_id, "IMP-002");
        assert_eq!(records[0].recurrence_count, 2);
        assert!(records[0].id.starts_with("FND-"));
        assert!(records[0].aliases.iter().any(|alias| alias == "IMP-001"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn regression_kind_reader_supports_experiment_skip() {
        let root = temp_root("regression-kind");
        let regression = root.join("imp-001.json");
        fs::write(
            &regression,
            br#"{"finding_id":"IMP-001","kind":"skill_route","category":"skill_routing_miss"}"#,
        )
        .unwrap();

        assert_eq!(read_regression_kind(&regression).unwrap(), "skill_route");
        let _ = fs::remove_dir_all(root);
    }
}
