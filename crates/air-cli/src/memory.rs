use crate::code_artifact::sha256_hex;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const MEMORY_SCHEMA: &str = "air.memory_card.v1";
const EPISODE_SCHEMA: &str = "air.memory_episode.v1";
const LEDGER_SCHEMA: &str = "air.memory_ledger_event.v1";
const MEMORY_EXTRACT_SCHEMA: &str = "air.memory_extract.v1";
const DREAM_IR_SCHEMA: &str = "air.dream_ir.v1";
const GRAPH_EDGE_SCHEMA: &str = "air.experience_graph_edge.v1";
const DEFAULT_MEMORY_DIR: &str = ".air/memory";

pub(crate) struct MemoryExtractOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) out_dir: PathBuf,
    pub(crate) window_manifest: PathBuf,
    pub(crate) observations_file: PathBuf,
    pub(crate) findings_file: PathBuf,
    pub(crate) suggested_regressions_file: Option<PathBuf>,
    pub(crate) mode: String,
}

pub(crate) struct MemoryListOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) kind: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) limit: Option<usize>,
}

pub(crate) struct MemorySearchOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) query: String,
    pub(crate) kind: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) limit: Option<usize>,
}

pub(crate) struct MemoryViewOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) id: String,
    pub(crate) evidence: bool,
}

pub(crate) struct MemorySkillDraftOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) id: String,
    pub(crate) out_dir: Option<PathBuf>,
}

pub(crate) struct MemorySkillEvaluateOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) id: String,
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) bench: bool,
}

pub(crate) struct MemoryPolicyCheckOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) id: String,
}

pub(crate) struct MemoryPromoteOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) id: String,
    pub(crate) status: Option<String>,
}

pub(crate) struct MemoryRetireOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) id: String,
    pub(crate) reason: Option<String>,
}

pub(crate) struct MemoryPackOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) task: String,
    pub(crate) limit: Option<usize>,
    pub(crate) include_evidence: bool,
    pub(crate) record_usage: bool,
}

pub(crate) struct MemoryHintOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) task: String,
    pub(crate) limit: Option<usize>,
}

pub(crate) struct MemoryGraphOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) limit: Option<usize>,
}

pub(crate) struct MemoryScorecardOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(crate) struct MemoryExtractOutput {
    pub(crate) schema: &'static str,
    pub(crate) status: String,
    pub(crate) memory_dir: String,
    pub(crate) report: String,
    pub(crate) episodes_written: usize,
    pub(crate) cards_written: usize,
    pub(crate) failure_candidates: usize,
    pub(crate) procedure_candidates: usize,
    pub(crate) routing_hints: usize,
    pub(crate) concept_candidates: usize,
    pub(crate) hypothesis_candidates: usize,
    pub(crate) policy_candidates: usize,
    pub(crate) graph_edges: usize,
    pub(crate) dream_ir_file: String,
    pub(crate) ledger_events: usize,
    cards: Vec<MemoryCardSummary>,
}

#[derive(Debug, Serialize)]
struct MemoryListOutput {
    schema: &'static str,
    memory_dir: String,
    cards: Vec<MemoryCardSummary>,
}

#[derive(Debug, Serialize)]
struct MemoryGraphOutput {
    schema: &'static str,
    memory_dir: String,
    edges: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct MemoryScorecardOutput {
    schema: &'static str,
    memory_dir: String,
    cards: Vec<MemoryScorecardRow>,
}

#[derive(Debug, Serialize)]
struct MemoryScorecardRow {
    id: String,
    kind: String,
    status: String,
    title: String,
    used: usize,
    helped_candidate: usize,
    hurt_candidate: usize,
    helped: usize,
    hurt: usize,
    neutral: usize,
    help_rate: f64,
    hurt_rate: f64,
}

#[derive(Debug, Serialize)]
struct MemorySkillEvaluateOutput {
    schema: &'static str,
    status: String,
    memory_id: String,
    draft_dir: String,
    validate: StageCheck,
    audit: StageCheck,
    bench: Option<StageCheck>,
    recommendation: String,
    next_commands: Vec<String>,
}

#[derive(Debug, Serialize)]
struct MemoryPolicyCheckOutput {
    schema: &'static str,
    status: String,
    memory_id: String,
    policy: String,
    can_compile_deterministic_guard: bool,
    suggested_runtime_rule: String,
    affected_files: Vec<String>,
    required_tests: Vec<String>,
    next_commands: Vec<String>,
}

#[derive(Debug, Serialize)]
struct StageCheck {
    command: String,
    status: String,
    code: Option<i32>,
    log: String,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct MemoryPackOutput {
    pub(crate) schema: &'static str,
    pub(crate) task: String,
    pub(crate) memory_dir: String,
    pub(crate) cards: Vec<MemoryPackCard>,
    pub(crate) context: String,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct MemoryPackCard {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) content: String,
    pub(crate) triggers: Vec<String>,
    pub(crate) confidence: f64,
    pub(crate) impact: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) evidence: Vec<MemoryEvidence>,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct MemoryRunHint {
    pub(crate) available_promoted: usize,
    pub(crate) reviewable_candidates: usize,
    pub(crate) suggested_memory_ids: Vec<String>,
    pub(crate) candidate_memory_ids: Vec<String>,
    pub(crate) message: String,
    pub(crate) next_commands: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct MemoryCard {
    schema: String,
    id: String,
    kind: String,
    scope: MemoryScope,
    title: String,
    content: String,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    evidence: Vec<MemoryEvidence>,
    confidence: f64,
    impact: f64,
    stability: String,
    status: String,
    created_at: u64,
    updated_at: u64,
    #[serde(default)]
    conflicts: Vec<String>,
    promotion: MemoryPromotion,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct MemoryScope {
    level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    executor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MemoryEvidence {
    pub(crate) kind: String,
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct MemoryPromotion {
    can_prompt_inject: bool,
    can_route: bool,
    can_compile_skill: bool,
    can_regression: bool,
    can_policy: bool,
    requires_human_review: bool,
}

#[derive(Debug, Serialize)]
struct MemoryEpisode {
    schema: &'static str,
    id: String,
    source_kind: String,
    source_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<String>,
    category: String,
    verdict: String,
    #[serde(default)]
    changed_files: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    memory_ids: Vec<String>,
    summary: String,
    created_at: u64,
}

#[derive(Debug, Serialize)]
struct MemoryLedgerEvent {
    schema: &'static str,
    event: String,
    id: String,
    kind: String,
    path: String,
    at_unix: u64,
}

#[derive(Debug, Serialize, Clone)]
struct DreamIrFile {
    schema: &'static str,
    generated_at_unix: u64,
    candidates: Vec<DreamIrCandidate>,
}

#[derive(Debug, Serialize, Clone)]
struct DreamIrCandidate {
    id: String,
    kind: String,
    name: String,
    confidence: f64,
    description: String,
    derived_from: Vec<String>,
    generalizes: Vec<String>,
    suggested_actions: Vec<DreamIrAction>,
}

#[derive(Debug, Serialize, Clone)]
struct DreamIrAction {
    kind: String,
    name: String,
    detail: String,
}

#[derive(Debug, Serialize)]
struct ExperienceGraphEdge {
    schema: &'static str,
    source: String,
    relation: String,
    target: String,
    evidence: Vec<String>,
    at_unix: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct MemoryUsageEvent {
    schema: String,
    memory_id: String,
    task: String,
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_artifact: Option<String>,
    at_unix: u64,
}

#[derive(Debug, Default, Clone, Copy)]
struct MemoryUsageStats {
    used: usize,
    helped_candidate: usize,
    hurt_candidate: usize,
    helped_confirmed: usize,
    hurt_confirmed: usize,
    neutral: usize,
}

struct SkillDraftResult {
    memory_id: String,
    slug: String,
    draft_dir: PathBuf,
    skill_path: PathBuf,
    evidence_path: PathBuf,
}

#[derive(Debug, Serialize, Clone)]
struct MemoryCardSummary {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) title: String,
    pub(crate) confidence: f64,
    pub(crate) impact: f64,
    pub(crate) path: String,
}

pub(crate) fn extract_memory(options: MemoryExtractOptions) -> Result<MemoryExtractOutput> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let now = unix_now();
    let memory_dir = absolutize(
        &cwd,
        options
            .memory_dir
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MEMORY_DIR)),
    );
    let out_dir = absolutize(&cwd, options.out_dir);
    fs::create_dir_all(&memory_dir).with_context(|| format!("create {}", memory_dir.display()))?;
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let observations = read_array_file(&options.observations_file, "observations")?;
    let findings = read_array_file(&options.findings_file, "findings")?;
    let regressions = options
        .suggested_regressions_file
        .as_ref()
        .filter(|path| path.exists())
        .map(|path| read_array_file(path, "regressions"))
        .transpose()?
        .unwrap_or_default();
    let window_inputs = read_array_file(&options.window_manifest, "inputs")?;

    let repo = repo_identifier(&cwd);
    let episodes = merge_episodes(
        build_episodes(&observations, now),
        build_window_episodes(&window_inputs, now),
    );
    let dream_ir = synthesize_dream_ir(
        &findings,
        &observations,
        &episodes,
        now,
        options.mode.as_str(),
    );
    let dream_ir_file = out_dir.join("dream_ir.json");
    fs::write(&dream_ir_file, serde_json::to_vec_pretty(&dream_ir)?)
        .with_context(|| format!("write {}", dream_ir_file.display()))?;
    let graph_edges = build_graph_edges(&findings, &observations, &episodes, &dream_ir, now);
    let mut cards = build_memory_cards(&repo, &findings, &observations, &regressions, now);
    cards.extend(build_synthesis_cards(&repo, &dream_ir, &observations, now));

    let mut ledger_events = 0;
    let mut summaries = Vec::new();
    for episode in &episodes {
        append_jsonl(&memory_dir.join("episodes.jsonl"), episode)?;
        append_ledger(
            &memory_dir,
            "episode_observed",
            &episode.id,
            "episode",
            "",
            now,
        )?;
        ledger_events += 1;
    }
    let usage_updates = append_memory_usage_outcomes(&memory_dir, &episodes, now)?;
    ledger_events += usage_updates;
    let scorecard_updates = reconcile_memory_usage(&memory_dir, now)?;
    ledger_events += scorecard_updates;
    for card in cards {
        let path = write_card(&memory_dir, card, now)?;
        let written = read_card(&path)?;
        append_ledger(
            &memory_dir,
            "memory_candidate_written",
            &written.id,
            &written.kind,
            &path.display().to_string(),
            now,
        )?;
        ledger_events += 1;
        summaries.push(card_summary(&written, &path));
    }
    for edge in &graph_edges {
        append_jsonl(&memory_dir.join("graph.jsonl"), edge)?;
    }

    let failure_candidates = summaries
        .iter()
        .filter(|card| card.kind == "failure")
        .count();
    let procedure_candidates = summaries
        .iter()
        .filter(|card| card.kind == "procedure")
        .count();
    let routing_hints = summaries
        .iter()
        .filter(|card| card.kind == "routing")
        .count();
    let concept_candidates = summaries
        .iter()
        .filter(|card| card.kind == "concept")
        .count();
    let hypothesis_candidates = summaries
        .iter()
        .filter(|card| card.kind == "hypothesis")
        .count();
    let policy_candidates = summaries
        .iter()
        .filter(|card| card.kind == "policy")
        .count();
    let report = out_dir.join("report.md");
    let output = MemoryExtractOutput {
        schema: MEMORY_EXTRACT_SCHEMA,
        status: if summaries.is_empty() && episodes.is_empty() && window_inputs.is_empty() {
            "empty_window".to_string()
        } else {
            "memory_ready".to_string()
        },
        memory_dir: memory_dir.display().to_string(),
        report: report.display().to_string(),
        episodes_written: episodes.len(),
        cards_written: summaries.len(),
        failure_candidates,
        procedure_candidates,
        routing_hints,
        concept_candidates,
        hypothesis_candidates,
        policy_candidates,
        graph_edges: graph_edges.len(),
        dream_ir_file: dream_ir_file.display().to_string(),
        ledger_events,
        cards: summaries,
    };
    fs::write(
        out_dir.join("memory.json"),
        serde_json::to_vec_pretty(&output)?,
    )
    .with_context(|| format!("write {}", out_dir.join("memory.json").display()))?;
    write_memory_report(&report, &output)?;
    Ok(output)
}

pub(crate) fn list_memory(options: MemoryListOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = absolutize(
        &cwd,
        options
            .memory_dir
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MEMORY_DIR)),
    );
    let cards = load_card_summaries(
        &memory_dir,
        options.kind.as_deref(),
        options.status.as_deref(),
    )?;
    let cards = limit_cards(cards, options.limit);
    println!(
        "{}",
        serde_json::to_string_pretty(&MemoryListOutput {
            schema: "air.memory_list.v1",
            memory_dir: memory_dir.display().to_string(),
            cards,
        })?
    );
    Ok(())
}

pub(crate) fn search_memory(options: MemorySearchOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = absolutize(
        &cwd,
        options
            .memory_dir
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MEMORY_DIR)),
    );
    let query = options.query.to_ascii_lowercase();
    let mut cards = Vec::new();
    for (card, path) in load_cards(&memory_dir)? {
        if !matches_filter(&card, options.kind.as_deref(), options.status.as_deref()) {
            continue;
        }
        let haystack = card_search_text(&card);
        if haystack.to_ascii_lowercase().contains(&query) {
            cards.push(card_summary(&card, &path));
        }
    }
    cards.sort_by(|left, right| {
        right
            .impact
            .partial_cmp(&left.impact)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    let cards = limit_cards(cards, options.limit);
    println!(
        "{}",
        serde_json::to_string_pretty(&MemoryListOutput {
            schema: "air.memory_search.v1",
            memory_dir: memory_dir.display().to_string(),
            cards,
        })?
    );
    Ok(())
}

pub(crate) fn view_memory(options: MemoryViewOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = absolutize(
        &cwd,
        options
            .memory_dir
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MEMORY_DIR)),
    );
    let Some((mut card, _path)) = load_cards(&memory_dir)?
        .into_iter()
        .find(|(card, _)| card.id == options.id)
    else {
        anyhow::bail!("memory id not found: {}", options.id);
    };
    if !options.evidence {
        card.evidence.clear();
    }
    println!("{}", serde_json::to_string_pretty(&card)?);
    Ok(())
}

pub(crate) fn draft_skill_from_memory(options: MemorySkillDraftOptions) -> Result<()> {
    let draft = create_skill_draft(options)?;
    let output = json!({
        "schema": "air.memory_skill_draft.v1",
        "status": "drafted",
        "memory_id": draft.memory_id,
        "skill": draft.slug,
        "draft_dir": draft.draft_dir.display().to_string(),
        "skill_file": draft.skill_path.display().to_string(),
        "evidence_file": draft.evidence_path.display().to_string(),
        "next_commands": [
            format!("air memory skill-evaluate {}", draft.memory_id),
            format!("air skill validate {}", draft.draft_dir.display()),
            format!("air skill audit {}", draft.draft_dir.display()),
            format!("air bench skill {} --compare-no-skill", draft.draft_dir.display())
        ]
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) fn evaluate_memory_skill(options: MemorySkillEvaluateOptions) -> Result<()> {
    let bench = options.bench;
    let draft = create_skill_draft(MemorySkillDraftOptions {
        memory_dir: options.memory_dir,
        id: options.id,
        out_dir: options.out_dir,
    })?;
    let log_dir = draft.draft_dir.join(".air-eval");
    fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;
    let validate = run_air_check(
        &[
            "skill",
            "validate",
            draft.draft_dir.to_string_lossy().as_ref(),
        ],
        &log_dir.join("validate.log"),
    )?;
    let audit = run_air_check(
        &["skill", "audit", draft.draft_dir.to_string_lossy().as_ref()],
        &log_dir.join("audit.log"),
    )?;
    let bench_check = if bench {
        Some(run_air_check(
            &[
                "bench",
                "skill",
                draft.draft_dir.to_string_lossy().as_ref(),
                "--compare-no-skill",
                "--limit",
                "1",
            ],
            &log_dir.join("bench.log"),
        )?)
    } else {
        None
    };
    let checks_passed = validate.status == "passed"
        && audit.status == "passed"
        && bench_check
            .as_ref()
            .is_none_or(|check| check.status == "passed");
    let recommendation = if !checks_passed {
        "reject_or_fix_draft"
    } else if bench_check.is_none() {
        "needs_benchmark_before_promotion"
    } else {
        "ready_for_human_review"
    }
    .to_string();
    let output = MemorySkillEvaluateOutput {
        schema: "air.memory_skill_evaluate.v1",
        status: if checks_passed {
            "evaluated".to_string()
        } else {
            "failed".to_string()
        },
        memory_id: draft.memory_id.clone(),
        draft_dir: draft.draft_dir.display().to_string(),
        validate,
        audit,
        bench: bench_check,
        recommendation,
        next_commands: vec![
            format!(
                "air bench skill {} --compare-no-skill",
                draft.draft_dir.display()
            ),
            format!(
                "air skill import {} --out skills/generated/{}",
                draft.draft_dir.display(),
                draft.slug
            ),
        ],
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) fn check_policy_candidate(options: MemoryPolicyCheckOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = absolutize(
        &cwd,
        options
            .memory_dir
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MEMORY_DIR)),
    );
    let Some((card, _path)) = load_cards(&memory_dir)?
        .into_iter()
        .find(|(card, _)| card.id == options.id)
    else {
        anyhow::bail!("memory id not found: {}", options.id);
    };
    if card.kind != "policy" {
        bail!(
            "memory {} is kind `{}`; only policy memory can be checked as runtime policy",
            card.id,
            card.kind
        );
    }
    let can_compile = card.promotion.can_policy
        && !card.evidence.is_empty()
        && card.status != "retired"
        && !card.content.to_ascii_lowercase().contains("disable");
    let affected_files = policy_affected_files(&card);
    let output = MemoryPolicyCheckOutput {
        schema: "air.memory_policy_check.v1",
        status: if can_compile {
            "candidate_checked".to_string()
        } else {
            "needs_review".to_string()
        },
        memory_id: card.id.clone(),
        policy: card.title,
        can_compile_deterministic_guard: can_compile,
        suggested_runtime_rule: card.content,
        affected_files,
        required_tests: vec![
            "cargo test -p air-cli improve::tests".to_string(),
            "cargo test -p air-cli memory::tests".to_string(),
            "air improve evaluate <finding> --candidate <candidate-dir>".to_string(),
        ],
        next_commands: vec![
            format!("air memory view {} --evidence", card.id),
            "Implement as a deterministic guard in a normal reviewed patch; do not auto-apply from memory.".to_string(),
        ],
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn create_skill_draft(options: MemorySkillDraftOptions) -> Result<SkillDraftResult> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = absolutize(
        &cwd,
        options
            .memory_dir
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MEMORY_DIR)),
    );
    let Some((card, _path)) = load_cards(&memory_dir)?
        .into_iter()
        .find(|(card, _)| card.id == options.id)
    else {
        bail!("memory id not found: {}", options.id);
    };
    if card.kind != "procedure" {
        bail!(
            "memory {} is kind `{}`; only procedure memory can draft skills",
            card.id,
            card.kind
        );
    }
    if !card.promotion.can_compile_skill {
        bail!("memory {} is not eligible for skill drafting", card.id);
    }
    let slug = slugify(&card.title);
    let draft_dir = absolutize(
        &cwd,
        options
            .out_dir
            .unwrap_or_else(|| memory_dir.join("drafts").join("skills").join(&slug)),
    );
    fs::create_dir_all(&draft_dir).with_context(|| format!("create {}", draft_dir.display()))?;
    let skill_path = draft_dir.join("SKILL.md");
    let evidence_path = draft_dir.join("evidence.json");
    fs::write(&skill_path, render_skill_draft(&card, &slug))
        .with_context(|| format!("write {}", skill_path.display()))?;
    fs::write(&evidence_path, serde_json::to_vec_pretty(&card.evidence)?)
        .with_context(|| format!("write {}", evidence_path.display()))?;
    Ok(SkillDraftResult {
        memory_id: card.id,
        slug,
        draft_dir,
        skill_path,
        evidence_path,
    })
}

pub(crate) fn promote_memory(options: MemoryPromoteOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let (mut card, path) = find_card(&memory_dir, &options.id)?;
    if card.evidence.is_empty() {
        anyhow::bail!("memory {} has no evidence and cannot be promoted", card.id);
    }
    let status = options.status.unwrap_or_else(|| "promoted".to_string());
    if !matches!(status.as_str(), "validated" | "promoted" | "pinned") {
        anyhow::bail!("unsupported memory promotion status: {status}");
    }
    card.status = status.clone();
    card.updated_at = unix_now();
    card.promotion.requires_human_review = false;
    if matches!(card.kind.as_str(), "failure" | "procedure" | "routing") {
        card.promotion.can_prompt_inject = true;
    }
    if matches!(card.kind.as_str(), "concept" | "hypothesis") {
        card.promotion.can_prompt_inject = true;
        card.promotion.can_route = true;
    }
    fs::write(&path, serde_json::to_vec_pretty(&card)?)
        .with_context(|| format!("write {}", path.display()))?;
    append_ledger(
        &memory_dir,
        "memory_promoted",
        &card.id,
        &card.kind,
        &path.display().to_string(),
        card.updated_at,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "air.memory_update.v1",
            "status": status,
            "id": card.id,
            "kind": card.kind,
            "path": path.display().to_string()
        }))?
    );
    Ok(())
}

pub(crate) fn retire_memory(options: MemoryRetireOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let (mut card, path) = find_card(&memory_dir, &options.id)?;
    card.status = "retired".to_string();
    card.updated_at = unix_now();
    if let Some(reason) = &options.reason {
        card.conflicts.push(format!("retired: {reason}"));
    }
    fs::write(&path, serde_json::to_vec_pretty(&card)?)
        .with_context(|| format!("write {}", path.display()))?;
    append_ledger(
        &memory_dir,
        "memory_retired",
        &card.id,
        &card.kind,
        &path.display().to_string(),
        card.updated_at,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "air.memory_update.v1",
            "status": "retired",
            "id": card.id,
            "kind": card.kind,
            "path": path.display().to_string(),
            "reason": options.reason
        }))?
    );
    Ok(())
}

pub(crate) fn pack_memory_command(options: MemoryPackOptions) -> Result<()> {
    let pack = build_memory_pack(options)?;
    println!("{}", serde_json::to_string_pretty(&pack)?);
    Ok(())
}

pub(crate) fn show_memory_graph(options: MemoryGraphOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let mut edges = read_jsonl_values(&memory_dir.join("graph.jsonl"))?;
    if let Some(limit) = options.limit {
        edges.truncate(limit);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&MemoryGraphOutput {
            schema: "air.memory_graph.v1",
            memory_dir: memory_dir.display().to_string(),
            edges,
        })?
    );
    Ok(())
}

pub(crate) fn show_memory_scorecard(options: MemoryScorecardOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let usage = read_usage_events(&memory_dir.join("usage.jsonl"))?;
    let stats = memory_usage_stats(&usage);
    let mut rows = Vec::new();
    for (card, _path) in load_cards(&memory_dir)? {
        let stats = stats.get(&card.id).copied().unwrap_or_default();
        if stats.used
            + stats.helped_candidate
            + stats.hurt_candidate
            + stats.helped_confirmed
            + stats.hurt_confirmed
            + stats.neutral
            == 0
        {
            continue;
        }
        let evaluated = stats.helped_candidate + stats.hurt_candidate + stats.neutral;
        rows.push(MemoryScorecardRow {
            id: card.id,
            kind: card.kind,
            status: card.status,
            title: card.title,
            used: stats.used,
            helped_candidate: stats.helped_candidate,
            hurt_candidate: stats.hurt_candidate,
            helped: stats.helped_confirmed,
            hurt: stats.hurt_confirmed,
            neutral: stats.neutral,
            help_rate: ratio(stats.helped_candidate, evaluated),
            hurt_rate: ratio(stats.hurt_candidate, evaluated),
        });
    }
    rows.sort_by(|left, right| {
        right
            .help_rate
            .partial_cmp(&left.help_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.used.cmp(&left.used))
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = options.limit {
        rows.truncate(limit);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&MemoryScorecardOutput {
            schema: "air.memory_scorecard.v1",
            memory_dir: memory_dir.display().to_string(),
            cards: rows,
        })?
    );
    Ok(())
}

pub(crate) fn build_memory_pack(options: MemoryPackOptions) -> Result<MemoryPackOutput> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let cards = select_pack_cards(
        &memory_dir,
        &options.task,
        options.limit.unwrap_or(5),
        options.include_evidence,
    )?;
    if options.record_usage {
        record_memory_pack_usage(&memory_dir, &options.task, &cards, unix_now())?;
    }
    let context = render_memory_context(&cards);
    Ok(MemoryPackOutput {
        schema: "air.memory_pack.v1",
        task: options.task,
        memory_dir: memory_dir.display().to_string(),
        cards,
        context,
    })
}

pub(crate) fn build_memory_run_hint(options: MemoryHintOptions) -> Result<MemoryRunHint> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let limit = options.limit.unwrap_or(5);
    let suggested = select_pack_cards(&memory_dir, &options.task, limit, false)?;
    let candidates = select_candidate_hint_cards(&memory_dir, &options.task, limit)?;
    let suggested_memory_ids = suggested
        .iter()
        .map(|card| card.id.clone())
        .collect::<Vec<_>>();
    let candidate_memory_ids = candidates
        .iter()
        .map(|card| card.id.clone())
        .collect::<Vec<_>>();
    let mut next_commands = Vec::new();
    if !suggested_memory_ids.is_empty() {
        next_commands.push(format!(
            "air run {} --memory",
            shell_quote_like(&options.task)
        ));
    }
    if !candidate_memory_ids.is_empty() {
        next_commands.push("air memory list --status candidate".to_string());
        next_commands.push(format!(
            "air memory view {} --evidence",
            candidate_memory_ids[0]
        ));
        next_commands.push(format!("air memory promote {}", candidate_memory_ids[0]));
    }
    let message = match (
        suggested_memory_ids.is_empty(),
        candidate_memory_ids.is_empty(),
    ) {
        (false, false) => {
            "Promoted memories match this task; candidate memories are also waiting for review."
                .to_string()
        }
        (false, true) => {
            "Promoted memories match this task. Re-run with --memory to include them.".to_string()
        }
        (true, false) => {
            "Candidate memories are waiting for review before they can be injected.".to_string()
        }
        (true, true) => "No matching promoted or candidate memory found for this task.".to_string(),
    };
    Ok(MemoryRunHint {
        available_promoted: suggested_memory_ids.len(),
        reviewable_candidates: candidate_memory_ids.len(),
        suggested_memory_ids,
        candidate_memory_ids,
        message,
        next_commands,
    })
}

fn build_episodes(observations: &[Value], now: u64) -> Vec<MemoryEpisode> {
    observations
        .iter()
        .map(|observation| {
            let source_path = string_field(observation, "source_path").unwrap_or_default();
            let category =
                string_field(observation, "category").unwrap_or_else(|| "unknown".to_string());
            let task =
                string_field(observation, "task").or_else(|| string_field(observation, "task_id"));
            let verdict = if observation
                .get("final_success")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "success"
            } else {
                "failure"
            }
            .to_string();
            let summary = format!(
                "{} episode: {}",
                category,
                string_field(observation, "message").unwrap_or_else(|| "run observed".to_string())
            );
            MemoryEpisode {
                schema: EPISODE_SCHEMA,
                id: memory_id("episode", &[&source_path, &category, &summary]),
                source_kind: string_field(observation, "source_kind")
                    .unwrap_or_else(|| "unknown".to_string()),
                source_path,
                artifact_path: string_field(observation, "artifact_path"),
                task,
                category,
                verdict,
                changed_files: string_array_field(observation, "changed_files"),
                skills: string_array_field(observation, "skills"),
                evidence: string_array_field(observation, "evidence"),
                memory_ids: string_array_field(observation, "memory_ids"),
                summary,
                created_at: now,
            }
        })
        .collect()
}

fn build_window_episodes(window_inputs: &[Value], now: u64) -> Vec<MemoryEpisode> {
    let mut episodes = Vec::new();
    for input in window_inputs {
        let path = string_field(input, "path").unwrap_or_default();
        if path.is_empty()
            || Path::new(&path).file_name().and_then(|name| name.to_str()) != Some("artifact.json")
        {
            continue;
        }
        let Ok(value) = read_json_file(Path::new(&path)) else {
            continue;
        };
        let final_success = value
            .pointer("/verdict/final_success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let category = if final_success {
            "success".to_string()
        } else {
            value
                .pointer("/failure_reason/category")
                .or_else(|| value.pointer("/verdict/failure_reason/category"))
                .and_then(Value::as_str)
                .unwrap_or("unknown_failure")
                .to_string()
        };
        let verdict = if final_success { "success" } else { "failure" }.to_string();
        let task = string_field(&value, "task");
        let changed_files = value
            .pointer("/delta/changed_files")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut skills = Vec::new();
        if let Some(skill_id) = value.pointer("/skill/id").and_then(Value::as_str) {
            skills.push(skill_id.to_string());
        }
        if let Some(selected) = value
            .pointer("/extra/skill_route/selected")
            .and_then(Value::as_array)
        {
            for selected in selected {
                if let Some(id) = selected.get("id").and_then(Value::as_str) {
                    skills.push(id.to_string());
                }
            }
        }
        skills.sort();
        skills.dedup();
        let memory_ids = memory_ids_from_artifact(&value);
        let summary = format!(
            "{} code-run episode from {}",
            verdict,
            task.as_deref().unwrap_or("unknown task")
        );
        episodes.push(MemoryEpisode {
            schema: EPISODE_SCHEMA,
            id: memory_id("episode", &[&path, &category, &summary]),
            source_kind: string_field(input, "kind")
                .unwrap_or_else(|| "code_run_artifact".to_string()),
            source_path: path.clone(),
            artifact_path: Some(path),
            task,
            category,
            verdict,
            changed_files,
            skills,
            evidence: Vec::new(),
            memory_ids,
            summary,
            created_at: now,
        });
    }
    episodes
}

fn merge_episodes(
    primary: Vec<MemoryEpisode>,
    secondary: Vec<MemoryEpisode>,
) -> Vec<MemoryEpisode> {
    let mut by_source = BTreeMap::new();
    for episode in primary.into_iter().chain(secondary) {
        let key = if episode.source_path.is_empty() {
            episode.id.clone()
        } else {
            episode.source_path.clone()
        };
        by_source.entry(key).or_insert(episode);
    }
    by_source.into_values().collect()
}

fn build_memory_cards(
    repo: &str,
    findings: &[Value],
    observations: &[Value],
    regressions: &[Value],
    now: u64,
) -> Vec<MemoryCard> {
    let mut cards = Vec::new();
    for finding in findings {
        let id = string_field(finding, "id").unwrap_or_else(|| "unknown".to_string());
        let category = string_field(finding, "category").unwrap_or_else(|| "unknown".to_string());
        let summary = string_field(finding, "summary").unwrap_or_else(|| id.clone());
        let impact_score = finding
            .get("impact_score")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let linked_observations = linked_observations(finding, observations);
        let evidence = finding_evidence(&linked_observations, regressions, &id);
        let scope = MemoryScope {
            level: "repo".to_string(),
            repo: Some(repo.to_string()),
            executor: Some("code-agent".to_string()),
            skill: dominant_skill(&linked_observations),
        };
        cards.push(MemoryCard {
            schema: MEMORY_SCHEMA.to_string(),
            id: memory_id("failure", &[&id, &category, &summary]),
            kind: "failure".to_string(),
            scope: scope.clone(),
            title: format!("{category}: {summary}"),
            content: failure_content(finding),
            triggers: category_triggers(&category),
            evidence: evidence.clone(),
            confidence: confidence_from_count(linked_observations.len()),
            impact: (impact_score / 1000.0).clamp(0.0, 1.0),
            stability: "medium".to_string(),
            status: "candidate".to_string(),
            created_at: now,
            updated_at: now,
            conflicts: Vec::new(),
            promotion: MemoryPromotion {
                can_prompt_inject: false,
                can_route: false,
                can_compile_skill: false,
                can_regression: true,
                can_policy: can_promote_policy(&category),
                requires_human_review: true,
            },
        });
        if let Some(procedure) = procedure_for_category(&category, &summary, &scope, &evidence, now)
        {
            cards.push(procedure);
        }
        if let Some(routing) = routing_for_observations(
            &category,
            &summary,
            &scope,
            &linked_observations,
            &evidence,
            now,
        ) {
            cards.push(routing);
        }
    }
    cards
}

fn synthesize_dream_ir(
    findings: &[Value],
    observations: &[Value],
    episodes: &[MemoryEpisode],
    now: u64,
    mode: &str,
) -> DreamIrFile {
    let mut candidates = Vec::new();
    if mode == "micro" {
        return DreamIrFile {
            schema: DREAM_IR_SCHEMA,
            generated_at_unix: now,
            candidates,
        };
    }
    let categories = findings
        .iter()
        .filter_map(|finding| string_field(finding, "category"))
        .collect::<Vec<_>>();
    let episode_ids = episodes
        .iter()
        .map(|episode| episode.id.clone())
        .collect::<Vec<_>>();
    if categories.iter().any(|category| {
        matches!(
            category.as_str(),
            "no_patch_applied" | "budget_exceeded" | "diff_constraint_failed"
        )
    }) {
        candidates.push(DreamIrCandidate {
            id: memory_id("dream_ir", &["concept", "bounded_exploration"]),
            kind: "concept_candidate".to_string(),
            name: "bounded_exploration".to_string(),
            confidence: confidence_from_count(episode_ids.len()),
            description: "Constrain exploration before mutation in large or ambiguous workspaces; search and map first, read bounded ranges, then edit minimally.".to_string(),
            derived_from: episode_ids.clone(),
            generalizes: observed_domains(observations),
            suggested_actions: vec![
                DreamIrAction {
                    kind: "procedure_candidate".to_string(),
                    name: "search_before_read".to_string(),
                    detail: "Prefer symbol/search probes before broad file reads.".to_string(),
                },
                DreamIrAction {
                    kind: "runtime_policy_candidate".to_string(),
                    name: "warn_on_repeated_broad_reads".to_string(),
                    detail: "Warn or redirect when repeated broad reads happen without an edit hypothesis.".to_string(),
                },
            ],
        });
    }
    if categories
        .iter()
        .any(|category| category == "verification_failed")
    {
        candidates.push(DreamIrCandidate {
            id: memory_id("dream_ir", &["concept", "verification_first"]),
            kind: "concept_candidate".to_string(),
            name: "verification_first_completion".to_string(),
            confidence: confidence_from_count(episode_ids.len()),
            description: "A task is not complete until required verification is present in the trace and passes; model-claimed success is not evidence.".to_string(),
            derived_from: episode_ids.clone(),
            generalizes: vec!["code_agent_completion".to_string(), "runtime_verdicts".to_string()],
            suggested_actions: vec![DreamIrAction {
                kind: "runtime_policy_candidate".to_string(),
                name: "require_trace_verification_before_success".to_string(),
                detail: "Keep final success tied to trace-derived verification, not assistant text.".to_string(),
            }],
        });
    }
    if categories
        .iter()
        .any(|category| category == "diff_constraint_failed")
    {
        candidates.push(DreamIrCandidate {
            id: memory_id("dream_ir", &["concept", "constraint_first_mutation"]),
            kind: "concept_candidate".to_string(),
            name: "constraint_first_mutation".to_string(),
            confidence: confidence_from_count(episode_ids.len()),
            description: "Treat allowed, required, forbidden, and eval-protected files as the mutation boundary before generating a patch.".to_string(),
            derived_from: episode_ids.clone(),
            generalizes: vec!["self_improvement".to_string(), "candidate_patch_review".to_string()],
            suggested_actions: vec![
                DreamIrAction {
                    kind: "procedure_candidate".to_string(),
                    name: "read_constraints_before_edit".to_string(),
                    detail: "Read and restate mutation constraints before editing.".to_string(),
                },
                DreamIrAction {
                    kind: "runtime_policy_candidate".to_string(),
                    name: "block_eval_boundary_mutation".to_string(),
                    detail: "Block candidate changes to eval, regression, trust, and gate files unless explicitly authorized.".to_string(),
                },
            ],
        });
    }
    if mode == "evolution" {
        let domains = observed_episode_domains(episodes, observations);
        let unique_categories = categories.iter().collect::<BTreeSet<_>>();
        if domains.len() >= 2 || unique_categories.len() >= 2 {
            candidates.push(DreamIrCandidate {
                id: memory_id("dream_ir", &["hypothesis", "cross_domain_control_loop"]),
                kind: "hypothesis_candidate".to_string(),
                name: "cross_domain_control_loop".to_string(),
                confidence: (confidence_from_count(episode_ids.len()) * 0.7).clamp(0.0, 1.0),
                description: "Different task domains show the same control-loop shape: sense constraints, narrow scope, act minimally, verify from trace evidence, then adjust routing or policy only after measured outcomes.".to_string(),
                derived_from: episode_ids,
                generalizes: domains,
                suggested_actions: vec![
                    DreamIrAction {
                        kind: "procedure_candidate".to_string(),
                        name: "sense_narrow_act_verify".to_string(),
                        detail: "Draft a cross-domain playbook, but keep it candidate-only until benchmarked across at least two task families.".to_string(),
                    },
                    DreamIrAction {
                        kind: "policy_candidate".to_string(),
                        name: "require_measured_memory_outcomes".to_string(),
                        detail: "Do not promote routing or memory changes without helped/hurt scorecard evidence.".to_string(),
                    },
                ],
            });
        }
    }
    DreamIrFile {
        schema: DREAM_IR_SCHEMA,
        generated_at_unix: now,
        candidates,
    }
}

fn build_synthesis_cards(
    repo: &str,
    dream_ir: &DreamIrFile,
    observations: &[Value],
    now: u64,
) -> Vec<MemoryCard> {
    let mut cards = Vec::new();
    let scope = MemoryScope {
        level: "repo".to_string(),
        repo: Some(repo.to_string()),
        executor: Some("code-agent".to_string()),
        skill: None,
    };
    let evidence = observations
        .iter()
        .take(8)
        .filter_map(|observation| {
            let path = string_field(observation, "source_path")?;
            Some(MemoryEvidence {
                kind: string_field(observation, "source_kind")
                    .unwrap_or_else(|| "observation".to_string()),
                path: path.clone(),
                verdict: Some("failure".to_string()),
                fingerprint: file_fingerprint(Path::new(&path)).ok(),
                note: string_field(observation, "message"),
            })
        })
        .collect::<Vec<_>>();
    for candidate in &dream_ir.candidates {
        cards.push(MemoryCard {
            schema: MEMORY_SCHEMA.to_string(),
            id: memory_id("concept", &[&candidate.name, &candidate.description]),
            kind: "concept".to_string(),
            scope: scope.clone(),
            title: candidate.name.replace('_', " "),
            content: candidate.description.clone(),
            triggers: candidate.generalizes.clone(),
            evidence: evidence.clone(),
            confidence: candidate.confidence,
            impact: 0.55,
            stability: "low".to_string(),
            status: "candidate".to_string(),
            created_at: now,
            updated_at: now,
            conflicts: Vec::new(),
            promotion: MemoryPromotion {
                can_prompt_inject: false,
                can_route: true,
                can_compile_skill: false,
                can_regression: false,
                can_policy: false,
                requires_human_review: true,
            },
        });
        cards.push(MemoryCard {
            schema: MEMORY_SCHEMA.to_string(),
            id: memory_id("hypothesis", &[&candidate.name, &candidate.description]),
            kind: "hypothesis".to_string(),
            scope: scope.clone(),
            title: format!("Hypothesis: {}", candidate.name.replace('_', " ")),
            content: format!(
                "If AIR applies `{}`, similar future tasks may reduce repeated failures. This is a dream synthesis hypothesis and needs benchmark/regression evidence before promotion.",
                candidate.name
            ),
            triggers: candidate.generalizes.clone(),
            evidence: evidence.clone(),
            confidence: (candidate.confidence * 0.8).clamp(0.0, 1.0),
            impact: 0.45,
            stability: "low".to_string(),
            status: "candidate".to_string(),
            created_at: now,
            updated_at: now,
            conflicts: Vec::new(),
            promotion: MemoryPromotion {
                can_prompt_inject: false,
                can_route: false,
                can_compile_skill: false,
                can_regression: false,
                can_policy: false,
                requires_human_review: true,
            },
        });
        for action in candidate.suggested_actions.iter().filter(|action| {
            matches!(
                action.kind.as_str(),
                "runtime_policy_candidate" | "policy_candidate"
            )
        }) {
            cards.push(MemoryCard {
                schema: MEMORY_SCHEMA.to_string(),
                id: memory_id("policy", &[&candidate.name, &action.name]),
                kind: "policy".to_string(),
                scope: scope.clone(),
                title: format!("Policy candidate: {}", action.name.replace('_', " ")),
                content: action.detail.clone(),
                triggers: vec![candidate.name.clone()],
                evidence: evidence.clone(),
                confidence: (candidate.confidence * 0.7).clamp(0.0, 1.0),
                impact: 0.5,
                stability: "low".to_string(),
                status: "candidate".to_string(),
                created_at: now,
                updated_at: now,
                conflicts: Vec::new(),
                promotion: MemoryPromotion {
                    can_prompt_inject: false,
                    can_route: false,
                    can_compile_skill: false,
                    can_regression: false,
                    can_policy: true,
                    requires_human_review: true,
                },
            });
        }
    }
    cards
}

fn build_graph_edges(
    findings: &[Value],
    observations: &[Value],
    episodes: &[MemoryEpisode],
    dream_ir: &DreamIrFile,
    now: u64,
) -> Vec<ExperienceGraphEdge> {
    let mut edges = Vec::new();
    for finding in findings {
        let finding_id = string_field(finding, "id").unwrap_or_else(|| "unknown".to_string());
        for observation in linked_observations(finding, observations) {
            if let Some(source_path) = string_field(observation, "source_path") {
                if let Some(episode) = episodes
                    .iter()
                    .find(|episode| episode.source_path == source_path)
                {
                    edges.push(ExperienceGraphEdge {
                        schema: GRAPH_EDGE_SCHEMA,
                        source: episode.id.clone(),
                        relation: "led_to".to_string(),
                        target: finding_id.clone(),
                        evidence: vec![source_path],
                        at_unix: now,
                    });
                }
            }
        }
    }
    for candidate in &dream_ir.candidates {
        for source in &candidate.derived_from {
            edges.push(ExperienceGraphEdge {
                schema: GRAPH_EDGE_SCHEMA,
                source: source.clone(),
                relation: "generalizes_to".to_string(),
                target: candidate.id.clone(),
                evidence: candidate.generalizes.clone(),
                at_unix: now,
            });
        }
        for action in &candidate.suggested_actions {
            edges.push(ExperienceGraphEdge {
                schema: GRAPH_EDGE_SCHEMA,
                source: candidate.id.clone(),
                relation: "suggests".to_string(),
                target: action.name.clone(),
                evidence: vec![action.detail.clone()],
                at_unix: now,
            });
        }
    }
    edges
}

fn memory_ids_from_artifact(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for pointer in ["/extra/memory_pack/cards", "/memory_pack/cards"] {
        if let Some(cards) = value.pointer(pointer).and_then(Value::as_array) {
            for card in cards {
                if let Some(id) = card.get("id").and_then(Value::as_str) {
                    ids.push(id.to_string());
                }
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn record_memory_pack_usage(
    memory_dir: &Path,
    task: &str,
    cards: &[MemoryPackCard],
    now: u64,
) -> Result<()> {
    for card in cards {
        append_jsonl(
            &memory_dir.join("usage.jsonl"),
            &MemoryUsageEvent {
                schema: "air.memory_usage.v1".to_string(),
                memory_id: card.id.clone(),
                task: task.to_string(),
                outcome: "used".to_string(),
                run_artifact: None,
                at_unix: now,
            },
        )?;
    }
    Ok(())
}

fn run_air_check(args: &[&str], log_path: &Path) -> Result<StageCheck> {
    let exe = std::env::current_exe().context("resolve current air executable")?;
    let output = Command::new(&exe)
        .args(args)
        .output()
        .with_context(|| format!("run air {}", args.join(" ")))?;
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut log = String::new();
    log.push_str("$ air ");
    log.push_str(&args.join(" "));
    log.push_str("\n\n## stdout\n\n");
    log.push_str(&String::from_utf8_lossy(&output.stdout));
    log.push_str("\n\n## stderr\n\n");
    log.push_str(&String::from_utf8_lossy(&output.stderr));
    fs::write(log_path, log).with_context(|| format!("write {}", log_path.display()))?;
    Ok(StageCheck {
        command: format!("air {}", args.join(" ")),
        status: if output.status.success() {
            "passed".to_string()
        } else {
            "failed".to_string()
        },
        code: output.status.code(),
        log: log_path.display().to_string(),
    })
}

fn policy_affected_files(card: &MemoryCard) -> Vec<String> {
    let text = format!("{} {}", card.title, card.content).to_ascii_lowercase();
    let mut files = Vec::new();
    if text.contains("verification") || text.contains("trace") {
        files.push("crates/air-cli/src/code_artifact.rs".to_string());
        files.push("crates/air-cli/src/audit.rs".to_string());
    }
    if text.contains("eval") || text.contains("regression") || text.contains("candidate") {
        files.push("crates/air-cli/src/improve.rs".to_string());
        files.push("crates/air-cli/src/eval_manifest.rs".to_string());
        files.push("crates/air-cli/src/self_lab.rs".to_string());
    }
    if text.contains("memory") || text.contains("routing") {
        files.push("crates/air-cli/src/memory.rs".to_string());
        files.push("crates/air-cli/src/skill.rs".to_string());
    }
    if files.is_empty() {
        files.push("crates/air-cli/src/dream.rs".to_string());
        files.push("crates/air-cli/src/memory.rs".to_string());
    }
    files.sort();
    files.dedup();
    files
}

fn append_memory_usage_outcomes(
    memory_dir: &Path,
    episodes: &[MemoryEpisode],
    now: u64,
) -> Result<usize> {
    let mut written = 0;
    let mut seen = read_usage_events(&memory_dir.join("usage.jsonl"))?
        .into_iter()
        .filter(|event| event.outcome.ends_with("_candidate"))
        .map(|event| {
            format!(
                "{}\0{}\0{}",
                event.memory_id,
                event.run_artifact.unwrap_or_default(),
                event.outcome
            )
        })
        .collect::<BTreeSet<_>>();
    for episode in episodes {
        if episode.memory_ids.is_empty() {
            continue;
        }
        let outcome = if episode.verdict == "success" {
            "helped_candidate"
        } else {
            "hurt_candidate"
        };
        let run_artifact = episode
            .artifact_path
            .clone()
            .unwrap_or_else(|| episode.source_path.clone());
        for memory_id in &episode.memory_ids {
            let key = format!("{}\0{}\0{}", memory_id, run_artifact, outcome);
            if !seen.insert(key) {
                continue;
            }
            append_jsonl(
                &memory_dir.join("usage.jsonl"),
                &MemoryUsageEvent {
                    schema: "air.memory_usage.v1".to_string(),
                    memory_id: memory_id.clone(),
                    task: episode.task.clone().unwrap_or_default(),
                    outcome: outcome.to_string(),
                    run_artifact: Some(run_artifact.clone()),
                    at_unix: now,
                },
            )?;
            written += 1;
        }
    }
    Ok(written)
}

fn reconcile_memory_usage(memory_dir: &Path, now: u64) -> Result<usize> {
    let usage_path = memory_dir.join("usage.jsonl");
    let mut usage = read_usage_events(&usage_path)?;
    let mut stats = memory_usage_stats(&usage);
    let mut events = 0;

    for (memory_id, item) in stats.clone() {
        let evaluated = item.helped_candidate + item.hurt_candidate + item.neutral;
        if item.helped_confirmed == 0
            && item.helped_candidate >= 3
            && evaluated >= 3
            && ratio(item.helped_candidate, evaluated) >= 0.67
        {
            append_jsonl(
                &usage_path,
                &MemoryUsageEvent {
                    schema: "air.memory_usage.v1".to_string(),
                    memory_id: memory_id.clone(),
                    task: String::new(),
                    outcome: "helped".to_string(),
                    run_artifact: None,
                    at_unix: now,
                },
            )?;
            append_ledger(
                memory_dir,
                "memory_help_confirmed",
                &memory_id,
                "memory",
                "",
                now,
            )?;
            usage.push(MemoryUsageEvent {
                schema: "air.memory_usage.v1".to_string(),
                memory_id: memory_id.clone(),
                task: String::new(),
                outcome: "helped".to_string(),
                run_artifact: None,
                at_unix: now,
            });
            events += 2;
        }
        if item.hurt_confirmed == 0
            && item.hurt_candidate >= 2
            && evaluated >= 2
            && ratio(item.hurt_candidate, evaluated) >= 0.6
        {
            append_jsonl(
                &usage_path,
                &MemoryUsageEvent {
                    schema: "air.memory_usage.v1".to_string(),
                    memory_id: memory_id.clone(),
                    task: String::new(),
                    outcome: "hurt".to_string(),
                    run_artifact: None,
                    at_unix: now,
                },
            )?;
            append_ledger(
                memory_dir,
                "memory_hurt_confirmed",
                &memory_id,
                "memory",
                "",
                now,
            )?;
            usage.push(MemoryUsageEvent {
                schema: "air.memory_usage.v1".to_string(),
                memory_id,
                task: String::new(),
                outcome: "hurt".to_string(),
                run_artifact: None,
                at_unix: now,
            });
            events += 2;
        }
    }

    stats = memory_usage_stats(&usage);
    for (mut card, path) in load_cards(memory_dir)? {
        let Some(item) = stats.get(&card.id).copied() else {
            continue;
        };
        if card.status != "retired" && item.hurt_confirmed > 0 {
            card.status = "retired".to_string();
            card.updated_at = now;
            card.conflicts
                .push("auto-retired: confirmed hurt scorecard".to_string());
            write_card_snapshot(&path, &card)?;
            append_ledger(
                memory_dir,
                "memory_auto_retired",
                &card.id,
                &card.kind,
                &path.display().to_string(),
                now,
            )?;
            events += 1;
        } else if card.status == "candidate" && item.helped_confirmed > 0 {
            card.status = "validated".to_string();
            card.updated_at = now;
            card.conflicts
                .push("auto-validated: confirmed helped scorecard".to_string());
            write_card_snapshot(&path, &card)?;
            append_ledger(
                memory_dir,
                "memory_auto_validated",
                &card.id,
                &card.kind,
                &path.display().to_string(),
                now,
            )?;
            events += 1;
        }
    }
    Ok(events)
}

fn memory_usage_stats(events: &[MemoryUsageEvent]) -> BTreeMap<String, MemoryUsageStats> {
    let mut stats = BTreeMap::new();
    for event in events {
        let entry: &mut MemoryUsageStats = stats.entry(event.memory_id.clone()).or_default();
        match event.outcome.as_str() {
            "used" => entry.used += 1,
            "helped_candidate" => entry.helped_candidate += 1,
            "hurt_candidate" => entry.hurt_candidate += 1,
            "helped" => entry.helped_confirmed += 1,
            "hurt" => entry.hurt_confirmed += 1,
            _ => entry.neutral += 1,
        }
    }
    stats
}

fn linked_observations<'a>(finding: &Value, observations: &'a [Value]) -> Vec<&'a Value> {
    let ids = finding
        .get("observation_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    observations
        .iter()
        .filter(|observation| {
            string_field(observation, "id")
                .as_ref()
                .is_some_and(|id| ids.contains(id))
        })
        .collect()
}

fn finding_evidence(
    observations: &[&Value],
    regressions: &[Value],
    finding_id: &str,
) -> Vec<MemoryEvidence> {
    let mut evidence = Vec::new();
    for observation in observations.iter().take(8) {
        let path = string_field(observation, "source_path").unwrap_or_default();
        evidence.push(MemoryEvidence {
            kind: string_field(observation, "source_kind")
                .unwrap_or_else(|| "observation".to_string()),
            fingerprint: file_fingerprint(Path::new(&path)).ok(),
            path,
            verdict: Some(
                if observation
                    .get("final_success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "success"
                } else {
                    "failure"
                }
                .to_string(),
            ),
            note: string_field(observation, "message"),
        });
    }
    for regression in regressions {
        if string_field(regression, "finding_id").as_deref() == Some(finding_id) {
            evidence.push(MemoryEvidence {
                kind: "suggested_regression".to_string(),
                path: finding_id.to_string(),
                verdict: None,
                fingerprint: None,
                note: string_field(regression, "title"),
            });
        }
    }
    evidence
}

fn failure_content(finding: &Value) -> String {
    let category = string_field(finding, "category").unwrap_or_else(|| "unknown".to_string());
    let priority = string_field(finding, "priority_reason").unwrap_or_default();
    let expected = string_field(finding, "expected_regression").unwrap_or_default();
    format!(
        "Repeated runtime evidence produced failure category `{category}`. Priority: {priority}. Regression expectation: {expected}. Keep this as candidate memory until a regression or deterministic policy validates the lesson."
    )
}

fn procedure_for_category(
    category: &str,
    summary: &str,
    scope: &MemoryScope,
    evidence: &[MemoryEvidence],
    now: u64,
) -> Option<MemoryCard> {
    let (title, content, can_compile_skill, can_policy) = match category {
        "verification_failed" => (
            "Verification-first completion procedure",
            "Before reporting success, run the required verification command and confirm the trace records a passing verification event. If verification fails or is absent, continue fixing or report the failure explicitly.",
            true,
            true,
        ),
        "no_patch_applied" => (
            "Hypothesis-to-edit procedure",
            "After bounded exploration, form an edit hypothesis and produce a minimal patch. Avoid ending code-agent work with only reads, searches, or verbal analysis when the task asks for a code change.",
            true,
            true,
        ),
        "budget_exceeded" => (
            "Bounded exploration procedure",
            "Use search first, read bounded ranges, and switch to an edit hypothesis before repeated tool calls consume the budget. Escalate only with concrete missing context.",
            true,
            true,
        ),
        "diff_constraint_failed" => (
            "Constraint-first patch procedure",
            "Read allowed, required, and forbidden file constraints before editing. Keep candidate patches inside the declared file scope and avoid touching eval or trust-state files.",
            true,
            true,
        ),
        "configuration_error" => (
            "Configuration diagnosis procedure",
            "Check local model, tool, skill, and verification configuration before attempting runtime code changes. Treat missing configuration as an operator action, not a candidate patch.",
            false,
            false,
        ),
        _ => return None,
    };
    Some(MemoryCard {
        schema: MEMORY_SCHEMA.to_string(),
        id: memory_id("procedure", &[category, title, summary]),
        kind: "procedure".to_string(),
        scope: scope.clone(),
        title: title.to_string(),
        content: content.to_string(),
        triggers: category_triggers(category),
        evidence: evidence.to_vec(),
        confidence: confidence_from_count(evidence.len()),
        impact: 0.5,
        stability: "medium".to_string(),
        status: "candidate".to_string(),
        created_at: now,
        updated_at: now,
        conflicts: Vec::new(),
        promotion: MemoryPromotion {
            can_prompt_inject: false,
            can_route: true,
            can_compile_skill,
            can_regression: false,
            can_policy,
            requires_human_review: true,
        },
    })
}

fn routing_for_observations(
    category: &str,
    summary: &str,
    scope: &MemoryScope,
    observations: &[&Value],
    evidence: &[MemoryEvidence],
    now: u64,
) -> Option<MemoryCard> {
    let languages = observed_languages(observations);
    if languages.is_empty() && scope.skill.is_none() {
        return None;
    }
    let mut triggers = category_triggers(category);
    triggers.extend(languages.iter().map(|language| format!("{language} task")));
    triggers.sort();
    triggers.dedup();
    let language_text = if languages.is_empty() {
        "unknown language".to_string()
    } else {
        languages.join(", ")
    };
    Some(MemoryCard {
        schema: MEMORY_SCHEMA.to_string(),
        id: memory_id("routing", &[category, summary, &language_text]),
        kind: "routing".to_string(),
        scope: scope.clone(),
        title: format!("Route {language_text} {category} tasks with extra caution"),
        content: format!(
            "When a task matches {language_text} and `{category}`, consider loading a bounded debugging or verification-focused skill. This is a candidate routing hint and must not force skill loading until validated by benchmark evidence."
        ),
        triggers,
        evidence: evidence.to_vec(),
        confidence: confidence_from_count(observations.len()),
        impact: 0.35,
        stability: "low".to_string(),
        status: "candidate".to_string(),
        created_at: now,
        updated_at: now,
        conflicts: Vec::new(),
        promotion: MemoryPromotion {
            can_prompt_inject: false,
            can_route: true,
            can_compile_skill: false,
            can_regression: false,
            can_policy: false,
            requires_human_review: true,
        },
    })
}

fn observed_languages(observations: &[&Value]) -> Vec<String> {
    let mut languages = Vec::new();
    for observation in observations {
        for file in string_array_field(observation, "changed_files") {
            if file.ends_with(".rs") {
                languages.push("rust".to_string());
            } else if file.ends_with(".ts") || file.ends_with(".tsx") || file.ends_with(".js") {
                languages.push("typescript".to_string());
            } else if file.ends_with(".py") {
                languages.push("python".to_string());
            }
        }
    }
    languages.sort();
    languages.dedup();
    languages
}

fn observed_domains(observations: &[Value]) -> Vec<String> {
    let refs = observations.iter().collect::<Vec<_>>();
    let mut domains = observed_languages(&refs);
    for observation in observations {
        if string_field(observation, "source_kind").as_deref() == Some("bench_task") {
            domains.push("benchmark".to_string());
        }
        if string_field(observation, "source_kind").as_deref() == Some("code_run_artifact") {
            domains.push("code_agent".to_string());
        }
    }
    domains.sort();
    domains.dedup();
    domains
}

fn observed_episode_domains(episodes: &[MemoryEpisode], observations: &[Value]) -> Vec<String> {
    let mut domains = observed_domains(observations);
    for episode in episodes {
        for file in &episode.changed_files {
            if file.ends_with(".rs") {
                domains.push("rust".to_string());
            } else if file.ends_with(".ts") || file.ends_with(".tsx") || file.ends_with(".js") {
                domains.push("typescript".to_string());
            } else if file.ends_with(".py") {
                domains.push("python".to_string());
            }
        }
        if episode.source_kind == "bench_run" {
            domains.push("benchmark".to_string());
        }
        if episode.source_kind == "code_run_artifact" {
            domains.push("code_agent".to_string());
        }
    }
    domains.sort();
    domains.dedup();
    domains
}

fn category_triggers(category: &str) -> Vec<String> {
    match category {
        "verification_failed" => vec![
            "verification failure".to_string(),
            "agent claimed success".to_string(),
            "missing test run".to_string(),
        ],
        "no_patch_applied" => vec![
            "no patch applied".to_string(),
            "read loop".to_string(),
            "analysis without edit".to_string(),
        ],
        "budget_exceeded" => vec![
            "budget exceeded".to_string(),
            "repeated tool calls".to_string(),
        ],
        "diff_constraint_failed" => vec![
            "forbidden file".to_string(),
            "required diff missing".to_string(),
        ],
        other => vec![other.replace('_', " ")],
    }
}

fn dominant_skill(observations: &[&Value]) -> Option<String> {
    let mut skills = Vec::new();
    for observation in observations {
        skills.extend(string_array_field(observation, "skills"));
    }
    skills.sort();
    skills.into_iter().next()
}

fn can_promote_policy(category: &str) -> bool {
    matches!(
        category,
        "verification_failed" | "no_patch_applied" | "budget_exceeded" | "diff_constraint_failed"
    )
}

fn confidence_from_count(count: usize) -> f64 {
    match count {
        0 => 0.2,
        1 => 0.45,
        2 => 0.6,
        3..=4 => 0.72,
        _ => 0.82,
    }
}

fn write_card(memory_dir: &Path, mut card: MemoryCard, now: u64) -> Result<PathBuf> {
    let path = memory_dir
        .join("cards")
        .join(&card.kind)
        .join(format!("{}.json", card.id));
    if let Ok(existing) = read_card(&path) {
        card.created_at = existing.created_at;
        if existing.status != "candidate" {
            card.status = existing.status;
        }
    }
    card.updated_at = now;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&card)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn write_card_snapshot(path: &Path, card: &MemoryCard) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(card)?)
        .with_context(|| format!("write {}", path.display()))
}

fn find_card(memory_dir: &Path, id: &str) -> Result<(MemoryCard, PathBuf)> {
    load_cards(memory_dir)?
        .into_iter()
        .find(|(card, _)| card.id == id)
        .ok_or_else(|| anyhow::anyhow!("memory id not found: {id}"))
}

fn read_card(path: &Path) -> Result<MemoryCard> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn select_pack_cards(
    memory_dir: &Path,
    task: &str,
    limit: usize,
    include_evidence: bool,
) -> Result<Vec<MemoryPackCard>> {
    let task_terms = search_terms(task);
    let mut scored = Vec::new();
    for (card, _path) in load_cards(memory_dir)? {
        if !matches!(card.status.as_str(), "promoted" | "pinned") {
            continue;
        }
        if !card.promotion.can_prompt_inject {
            continue;
        }
        let text = card_search_text(&card).to_ascii_lowercase();
        let lexical = task_terms
            .iter()
            .filter(|term| text.contains(term.as_str()))
            .count() as f64;
        let trigger = card
            .triggers
            .iter()
            .filter(|trigger| {
                task.to_ascii_lowercase()
                    .contains(&trigger.to_ascii_lowercase())
            })
            .count() as f64
            * 2.0;
        let score = lexical + trigger + card.impact + card.confidence;
        if score <= 0.0 {
            continue;
        }
        scored.push((score, card));
    }
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .partial_cmp(left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                right
                    .impact
                    .partial_cmp(&left.impact)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    scored.truncate(limit);
    Ok(scored
        .into_iter()
        .map(|(_, card)| MemoryPackCard {
            id: card.id,
            kind: card.kind,
            title: card.title,
            content: card.content,
            triggers: card.triggers,
            confidence: card.confidence,
            impact: card.impact,
            evidence: if include_evidence {
                card.evidence
            } else {
                Vec::new()
            },
        })
        .collect())
}

fn select_candidate_hint_cards(
    memory_dir: &Path,
    task: &str,
    limit: usize,
) -> Result<Vec<MemoryPackCard>> {
    let task_terms = search_terms(task);
    let mut scored = Vec::new();
    for (card, _path) in load_cards(memory_dir)? {
        if card.status != "candidate" || card.kind == "episode" {
            continue;
        }
        let text = card_search_text(&card).to_ascii_lowercase();
        let lexical = task_terms
            .iter()
            .filter(|term| text.contains(term.as_str()))
            .count() as f64;
        let trigger = card
            .triggers
            .iter()
            .filter(|trigger| {
                task.to_ascii_lowercase()
                    .contains(&trigger.to_ascii_lowercase())
            })
            .count() as f64
            * 2.0;
        let score = lexical + trigger + card.impact + card.confidence;
        if score <= 0.0 {
            continue;
        }
        scored.push((score, card));
    }
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .partial_cmp(left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                right
                    .impact
                    .partial_cmp(&left.impact)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    scored.truncate(limit);
    Ok(scored
        .into_iter()
        .map(|(_, card)| MemoryPackCard {
            id: card.id,
            kind: card.kind,
            title: card.title,
            content: card.content,
            triggers: card.triggers,
            confidence: card.confidence,
            impact: card.impact,
            evidence: Vec::new(),
        })
        .collect())
}

fn render_memory_context(cards: &[MemoryPackCard]) -> String {
    if cards.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("AIR promoted memory context:\n");
    out.push_str("- Use these memories as compact guidance, not as proof of task success.\n");
    out.push_str(
        "- Verification, policy, and current repository evidence still override memory.\n",
    );
    for card in cards {
        out.push_str(&format!(
            "\n[{}] {} ({}, confidence {:.2}, impact {:.2})\n{}\n",
            card.id, card.title, card.kind, card.confidence, card.impact, card.content
        ));
        if !card.triggers.is_empty() {
            out.push_str(&format!("Triggers: {}\n", card.triggers.join(", ")));
        }
    }
    out
}

fn shell_quote_like(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

fn load_cards(memory_dir: &Path) -> Result<Vec<(MemoryCard, PathBuf)>> {
    let root = memory_dir.join("cards");
    let mut paths = Vec::new();
    collect_card_paths(&root, &mut paths)?;
    let mut cards = Vec::new();
    for path in paths {
        if let Ok(card) = read_card(&path) {
            cards.push((card, path));
        }
    }
    Ok(cards)
}

fn collect_card_paths(root: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if root.extension().and_then(|ext| ext.to_str()) == Some("json") {
            paths.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry under {}", root.display()))?;
        collect_card_paths(&entry.path(), paths)?;
    }
    Ok(())
}

fn load_card_summaries(
    memory_dir: &Path,
    kind: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<MemoryCardSummary>> {
    let mut cards = load_cards(memory_dir)?
        .into_iter()
        .filter(|(card, _)| matches_filter(card, kind, status))
        .map(|(card, path)| card_summary(&card, &path))
        .collect::<Vec<_>>();
    cards.sort_by(|left, right| {
        right
            .impact
            .partial_cmp(&left.impact)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(cards)
}

fn matches_filter(card: &MemoryCard, kind: Option<&str>, status: Option<&str>) -> bool {
    kind.is_none_or(|kind| card.kind == kind) && status.is_none_or(|status| card.status == status)
}

fn limit_cards(mut cards: Vec<MemoryCardSummary>, limit: Option<usize>) -> Vec<MemoryCardSummary> {
    if let Some(limit) = limit {
        cards.truncate(limit);
    }
    cards
}

fn card_summary(card: &MemoryCard, path: &Path) -> MemoryCardSummary {
    MemoryCardSummary {
        id: card.id.clone(),
        kind: card.kind.clone(),
        status: card.status.clone(),
        title: card.title.clone(),
        confidence: card.confidence,
        impact: card.impact,
        path: path.display().to_string(),
    }
}

fn card_search_text(card: &MemoryCard) -> String {
    let mut text = format!("{} {} {}", card.id, card.title, card.content);
    for trigger in &card.triggers {
        text.push(' ');
        text.push_str(trigger);
    }
    for evidence in &card.evidence {
        text.push(' ');
        text.push_str(&evidence.path);
        if let Some(note) = &evidence.note {
            text.push(' ');
            text.push_str(note);
        }
    }
    text
}

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    writeln!(file, "{}", serde_json::to_string(value)?)
        .with_context(|| format!("write {}", path.display()))
}

fn append_ledger(
    memory_dir: &Path,
    event: &str,
    id: &str,
    kind: &str,
    path: &str,
    now: u64,
) -> Result<()> {
    append_jsonl(
        &memory_dir.join("ledger.jsonl"),
        &MemoryLedgerEvent {
            schema: LEDGER_SCHEMA,
            event: event.to_string(),
            id: id.to_string(),
            kind: kind.to_string(),
            path: path.to_string(),
            at_unix: now,
        },
    )
}

fn write_memory_report(path: &Path, output: &MemoryExtractOutput) -> Result<()> {
    let mut report = String::new();
    report.push_str("# AIR Memory Changes\n\n");
    report.push_str("Dream memory compiles run evidence into candidate long-term memory. Candidate memories are not prompt-injected, routed, compiled into skills, or promoted into runtime policy until later gates validate them.\n\n");
    report.push_str(&format!("- status: `{}`\n", output.status));
    report.push_str(&format!(
        "- episodes_written: `{}`\n",
        output.episodes_written
    ));
    report.push_str(&format!("- cards_written: `{}`\n", output.cards_written));
    report.push_str(&format!(
        "- failure_candidates: `{}`\n",
        output.failure_candidates
    ));
    report.push_str(&format!(
        "- procedure_candidates: `{}`\n",
        output.procedure_candidates
    ));
    report.push_str(&format!("- routing_hints: `{}`\n", output.routing_hints));
    report.push_str(&format!(
        "- concept_candidates: `{}`\n",
        output.concept_candidates
    ));
    report.push_str(&format!(
        "- hypothesis_candidates: `{}`\n",
        output.hypothesis_candidates
    ));
    report.push_str(&format!(
        "- policy_candidates: `{}`\n",
        output.policy_candidates
    ));
    report.push_str(&format!("- graph_edges: `{}`\n", output.graph_edges));
    report.push_str(&format!("- dream_ir: `{}`\n\n", output.dream_ir_file));
    if output.cards.is_empty() {
        report.push_str("No memory cards were created for this window.\n");
    } else {
        report.push_str("## Cards\n\n");
        for card in &output.cards {
            report.push_str(&format!(
                "- `{}` `{}` `{}` impact `{:.2}` confidence `{:.2}`: {}\n",
                card.id, card.kind, card.status, card.impact, card.confidence, card.title
            ));
        }
    }
    fs::write(path, report).with_context(|| format!("write {}", path.display()))
}

fn render_skill_draft(card: &MemoryCard, slug: &str) -> String {
    let triggers = if card.triggers.is_empty() {
        "procedure memory".to_string()
    } else {
        card.triggers.join(", ")
    };
    format!(
        r#"---
name: {slug}
description: {description}
version: 0.1.0
metadata:
  air:
    source_memory_ids:
      - {memory_id}
    evidence_count: {evidence_count}
    status: draft
    requires_review: true
---

# {title}

## When to Use

Use when the task matches: {triggers}.

## Procedure

{content}

## Pitfalls

- Do not treat this draft as trusted until `air skill validate`, `air skill audit`, benchmark comparison, and regression gates pass.
- Do not expand tool permissions based only on memory evidence.
- Do not report success without explicit verification.

## Verification

- Run the smallest relevant verification first.
- Run the required repository or task verification before reporting success.
"#,
        slug = slug,
        description = yaml_scalar(&format!("Draft skill from AIR memory: {}", card.title)),
        memory_id = card.id,
        evidence_count = card.evidence.len(),
        title = card.title,
        triggers = triggers,
        content = card.content
    )
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn yaml_scalar(value: &str) -> String {
    format!("{:?}", value)
}

fn read_array_file(path: &Path, key: &str) -> Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    Ok(value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn read_json_file(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}

fn read_jsonl_values(path: &Path) -> Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect())
}

fn read_usage_events(path: &Path) -> Result<Vec<MemoryUsageEvent>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<MemoryUsageEvent>(line).ok())
        .collect())
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn string_array_field(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn search_terms(value: &str) -> Vec<String> {
    let mut terms = value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(str::trim)
        .filter(|term| term.len() >= 3)
        .map(|term| term.to_ascii_lowercase())
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms
}

fn ratio(value: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64
    }
}

fn memory_id(kind: &str, parts: &[&str]) -> String {
    let mut input = kind.to_string();
    for part in parts {
        input.push('\0');
        input.push_str(part);
    }
    let hash = sha256_hex(input.as_bytes());
    format!("mem_{}_{}", kind, &hash[..12])
}

fn memory_dir(cwd: &Path, memory_dir: Option<PathBuf>) -> PathBuf {
    absolutize(
        cwd,
        memory_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_MEMORY_DIR)),
    )
}

fn file_fingerprint(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(format!("sha256:{}", sha256_hex(&bytes)))
}

fn repo_identifier(cwd: &Path) -> String {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(cwd)
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                return text;
            }
        }
    }
    cwd.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo")
        .to_string()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
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
            "air-memory-test-{}-{}-{name}",
            std::process::id(),
            unix_now()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn procedure_candidates_are_gated_from_prompt_injection() {
        let scope = MemoryScope {
            level: "repo".to_string(),
            repo: Some("repo".to_string()),
            executor: Some("code-agent".to_string()),
            skill: None,
        };
        let card =
            procedure_for_category("verification_failed", "summary", &scope, &[], 1_770_000_000)
                .unwrap();

        assert_eq!(card.kind, "procedure");
        assert_eq!(card.status, "candidate");
        assert!(!card.promotion.can_prompt_inject);
        assert!(card.promotion.can_compile_skill);
        assert!(card.promotion.requires_human_review);
    }

    #[test]
    fn memory_ids_are_stable() {
        assert_eq!(
            memory_id("failure", &["IMP-001", "verification_failed"]),
            memory_id("failure", &["IMP-001", "verification_failed"])
        );
    }

    #[test]
    fn synthesis_turns_diff_failures_into_concepts() {
        let observations = vec![json!({
            "id": "OBS-001",
            "source_kind": "bench_task",
            "source_path": "target/generated/run.json",
            "category": "diff_constraint_failed",
            "message": "required file missing",
            "final_success": false,
            "changed_files": ["src/lib.rs"]
        })];
        let findings = vec![json!({
            "id": "IMP-001",
            "category": "diff_constraint_failed",
            "summary": "constraint failure",
            "observation_ids": ["OBS-001"]
        })];
        let episodes = build_episodes(&observations, 1_770_000_000);
        let ir = synthesize_dream_ir(&findings, &observations, &episodes, 1_770_000_000, "deep");

        assert!(ir
            .candidates
            .iter()
            .any(|candidate| candidate.name == "constraint_first_mutation"));
        let cards = build_synthesis_cards("repo", &ir, &observations, 1_770_000_000);
        assert!(cards.iter().any(|card| card.kind == "concept"));
        assert!(cards.iter().any(|card| card.kind == "policy"));
    }

    #[test]
    fn window_artifacts_create_success_episodes_with_memory_ids() {
        let root = temp_root("window-episode");
        let artifact = root.join("artifact.json");
        fs::write(
            &artifact,
            serde_json::to_vec_pretty(&json!({
                "schema": "air.code_run_artifact.v1",
                "task": "fix parser",
                "skill": { "id": "code-agent" },
                "delta": { "changed_files": ["src/lib.rs"] },
                "verdict": { "final_success": true },
                "extra": {
                    "memory_pack": {
                        "cards": [
                            { "id": "mem_procedure_abc" }
                        ]
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let episodes = build_window_episodes(
            &[json!({
                "kind": "code_run_artifact",
                "path": artifact.display().to_string(),
            })],
            1_770_000_000,
        );

        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].verdict, "success");
        assert_eq!(episodes[0].category, "success");
        assert_eq!(episodes[0].memory_ids, vec!["mem_procedure_abc"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn usage_score_outcomes_start_as_candidates() {
        let root = temp_root("usage");
        let episodes = vec![
            MemoryEpisode {
                schema: EPISODE_SCHEMA,
                id: "ep1".to_string(),
                source_kind: "code_run_artifact".to_string(),
                source_path: "run1/artifact.json".to_string(),
                artifact_path: Some("run1/artifact.json".to_string()),
                task: Some("task".to_string()),
                category: "success".to_string(),
                verdict: "success".to_string(),
                changed_files: Vec::new(),
                skills: Vec::new(),
                evidence: Vec::new(),
                memory_ids: vec!["mem_a".to_string()],
                summary: "success".to_string(),
                created_at: 1,
            },
            MemoryEpisode {
                schema: EPISODE_SCHEMA,
                id: "ep2".to_string(),
                source_kind: "code_run_artifact".to_string(),
                source_path: "run2/artifact.json".to_string(),
                artifact_path: Some("run2/artifact.json".to_string()),
                task: Some("task".to_string()),
                category: "verification_failed".to_string(),
                verdict: "failure".to_string(),
                changed_files: Vec::new(),
                skills: Vec::new(),
                evidence: Vec::new(),
                memory_ids: vec!["mem_a".to_string()],
                summary: "failure".to_string(),
                created_at: 1,
            },
        ];

        append_memory_usage_outcomes(&root, &episodes, 1_770_000_000).unwrap();
        let events = read_usage_events(&root.join("usage.jsonl")).unwrap();
        let outcomes = events
            .iter()
            .map(|event| event.outcome.as_str())
            .collect::<Vec<_>>();
        assert_eq!(outcomes, vec!["helped_candidate", "hurt_candidate"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn usage_reconcile_validates_helpful_and_retires_harmful_memory() {
        let root = temp_root("reconcile");
        let scope = MemoryScope {
            level: "repo".to_string(),
            repo: Some("repo".to_string()),
            executor: Some("code-agent".to_string()),
            skill: None,
        };
        let now = 1_770_000_000;
        for (id, title) in [
            ("mem_helpful", "Helpful memory"),
            ("mem_harmful", "Harmful memory"),
        ] {
            write_card(
                &root,
                MemoryCard {
                    schema: MEMORY_SCHEMA.to_string(),
                    id: id.to_string(),
                    kind: "procedure".to_string(),
                    scope: scope.clone(),
                    title: title.to_string(),
                    content: "memory content".to_string(),
                    triggers: vec!["task".to_string()],
                    evidence: vec![MemoryEvidence {
                        kind: "code_run_artifact".to_string(),
                        path: "artifact.json".to_string(),
                        verdict: Some("success".to_string()),
                        fingerprint: None,
                        note: None,
                    }],
                    confidence: 0.7,
                    impact: 0.7,
                    stability: "medium".to_string(),
                    status: "candidate".to_string(),
                    created_at: now,
                    updated_at: now,
                    conflicts: Vec::new(),
                    promotion: MemoryPromotion {
                        can_prompt_inject: false,
                        can_route: true,
                        can_compile_skill: true,
                        can_regression: false,
                        can_policy: false,
                        requires_human_review: true,
                    },
                },
                now,
            )
            .unwrap();
        }
        for index in 0..3 {
            append_jsonl(
                &root.join("usage.jsonl"),
                &MemoryUsageEvent {
                    schema: "air.memory_usage.v1".to_string(),
                    memory_id: "mem_helpful".to_string(),
                    task: "task".to_string(),
                    outcome: "helped_candidate".to_string(),
                    run_artifact: Some(format!("help-{index}.json")),
                    at_unix: now,
                },
            )
            .unwrap();
        }
        for index in 0..2 {
            append_jsonl(
                &root.join("usage.jsonl"),
                &MemoryUsageEvent {
                    schema: "air.memory_usage.v1".to_string(),
                    memory_id: "mem_harmful".to_string(),
                    task: "task".to_string(),
                    outcome: "hurt_candidate".to_string(),
                    run_artifact: Some(format!("hurt-{index}.json")),
                    at_unix: now,
                },
            )
            .unwrap();
        }

        reconcile_memory_usage(&root, now + 1).unwrap();

        let (helpful, _) = find_card(&root, "mem_helpful").unwrap();
        let (harmful, _) = find_card(&root, "mem_harmful").unwrap();
        assert_eq!(helpful.status, "validated");
        assert_eq!(harmful.status, "retired");
        let usage = read_usage_events(&root.join("usage.jsonl")).unwrap();
        assert!(usage
            .iter()
            .any(|event| event.memory_id == "mem_helpful" && event.outcome == "helped"));
        assert!(usage
            .iter()
            .any(|event| event.memory_id == "mem_harmful" && event.outcome == "hurt"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_hint_reports_promoted_and_candidate_memory() {
        let root = temp_root("hint");
        let scope = MemoryScope {
            level: "repo".to_string(),
            repo: Some("repo".to_string()),
            executor: Some("code-agent".to_string()),
            skill: None,
        };
        let now = 1_770_000_000;
        let promoted = MemoryCard {
            schema: MEMORY_SCHEMA.to_string(),
            id: "mem_promoted_rust".to_string(),
            kind: "procedure".to_string(),
            scope: scope.clone(),
            title: "Rust verification workflow".to_string(),
            content: "Run cargo test after Rust edits.".to_string(),
            triggers: vec!["rust".to_string(), "verification".to_string()],
            evidence: vec![MemoryEvidence {
                kind: "code_run_artifact".to_string(),
                path: "artifact.json".to_string(),
                verdict: Some("success".to_string()),
                fingerprint: None,
                note: None,
            }],
            confidence: 0.9,
            impact: 0.8,
            stability: "medium".to_string(),
            status: "promoted".to_string(),
            created_at: now,
            updated_at: now,
            conflicts: Vec::new(),
            promotion: MemoryPromotion {
                can_prompt_inject: true,
                can_route: true,
                can_compile_skill: true,
                can_regression: false,
                can_policy: false,
                requires_human_review: false,
            },
        };
        let candidate = MemoryCard {
            schema: MEMORY_SCHEMA.to_string(),
            id: "mem_candidate_rust".to_string(),
            kind: "procedure".to_string(),
            scope,
            title: "Candidate Rust bounded debug".to_string(),
            content: "Use rg before broad reads in Rust tasks.".to_string(),
            triggers: vec!["rust".to_string()],
            evidence: Vec::new(),
            confidence: 0.7,
            impact: 0.6,
            stability: "low".to_string(),
            status: "candidate".to_string(),
            created_at: now,
            updated_at: now,
            conflicts: Vec::new(),
            promotion: MemoryPromotion {
                can_prompt_inject: false,
                can_route: false,
                can_compile_skill: true,
                can_regression: false,
                can_policy: false,
                requires_human_review: true,
            },
        };
        write_card(&root, promoted, now).unwrap();
        write_card(&root, candidate, now).unwrap();

        let hint = build_memory_run_hint(MemoryHintOptions {
            memory_dir: Some(root.clone()),
            task: "fix rust verification failure".to_string(),
            limit: Some(5),
        })
        .unwrap();

        assert_eq!(hint.available_promoted, 1);
        assert_eq!(hint.reviewable_candidates, 1);
        assert_eq!(hint.suggested_memory_ids, vec!["mem_promoted_rust"]);
        assert_eq!(hint.candidate_memory_ids, vec!["mem_candidate_rust"]);
        assert!(hint
            .next_commands
            .iter()
            .any(|cmd| cmd.contains("--memory")));
        assert!(hint
            .next_commands
            .iter()
            .any(|cmd| cmd == "air memory promote mem_candidate_rust"));
        let _ = fs::remove_dir_all(root);
    }
}
