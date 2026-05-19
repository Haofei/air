use crate::code_artifact::sha256_hex;
use crate::ops::{
    append_jsonl_locked, run_current_air_stage, with_jsonl_lock, write_json_atomic, AirLayout,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MEMORY_SCHEMA: &str = "air.memory_card.v1";
const EPISODE_SCHEMA: &str = "air.memory_episode.v1";
const LEDGER_SCHEMA: &str = "air.memory_ledger_event.v1";
const MEMORY_EXTRACT_SCHEMA: &str = "air.memory_extract.v1";
const MEMORY_ADVANCE_SCHEMA: &str = "air.memory_advance.v1";
const DREAM_IR_SCHEMA: &str = "air.dream_ir.v1";
const GRAPH_EDGE_SCHEMA: &str = "air.experience_graph_edge.v1";
const MEMORY_SCORECARD_SCHEMA: &str = "air.memory_scorecard_state.v1";
const DEFAULT_MEMORY_DIR: &str = ".air/memory";
const MEMORY_USAGE_COMPACT_BYTES: u64 = 1_048_576;

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

pub(crate) struct MemoryCausalEvalOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) id: String,
    pub(crate) outcome: String,
    pub(crate) evidence: PathBuf,
    pub(crate) note: Option<String>,
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

pub(crate) struct MemoryAdvanceOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) skill_bench: bool,
    pub(crate) limit: Option<usize>,
    pub(crate) timeout_seconds: Option<u64>,
}

pub(crate) struct BrainOptions {
    pub(crate) memory_dir: Option<PathBuf>,
    pub(crate) dream_dir: Option<PathBuf>,
    pub(crate) section: BrainSection,
    pub(crate) limit: Option<usize>,
    pub(crate) out: Option<PathBuf>,
    pub(crate) json: bool,
}

pub(crate) enum BrainSection {
    Summary,
    Memory,
    Skills,
    Policies,
    Findings,
    Candidates,
    View(String),
    Report,
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
    causal_helped: usize,
    causal_hurt: usize,
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
pub(crate) struct MemoryAdvanceOutput {
    pub(crate) schema: &'static str,
    pub(crate) status: String,
    pub(crate) memory_dir: String,
    pub(crate) out_dir: String,
    pub(crate) promoted_memories: Vec<String>,
    pub(crate) validated_memories: Vec<String>,
    pub(crate) retired_memories: Vec<String>,
    pub(crate) validated_skills: Vec<MemoryAdvanceSkill>,
    pub(crate) routing_measurements: Vec<MemoryRoutingMeasurement>,
    pub(crate) reviewed_guards: Vec<MemoryAdvanceGuard>,
    pub(crate) next_commands: Vec<String>,
}

impl MemoryAdvanceOutput {
    pub(crate) fn skipped(out_dir: &Path, reason: &str) -> Result<Self> {
        fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
        let output = Self {
            schema: MEMORY_ADVANCE_SCHEMA,
            status: reason.to_string(),
            memory_dir: DEFAULT_MEMORY_DIR.to_string(),
            out_dir: out_dir.display().to_string(),
            promoted_memories: Vec::new(),
            validated_memories: Vec::new(),
            retired_memories: Vec::new(),
            validated_skills: Vec::new(),
            routing_measurements: Vec::new(),
            reviewed_guards: Vec::new(),
            next_commands: Vec::new(),
        };
        fs::write(
            out_dir.join("advance.json"),
            serde_json::to_vec_pretty(&output)?,
        )
        .with_context(|| format!("write {}", out_dir.join("advance.json").display()))?;
        write_advance_report(&out_dir.join("advance.md"), &output)?;
        Ok(output)
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MemoryAdvanceSkill {
    pub(crate) memory_id: String,
    pub(crate) draft_dir: String,
    pub(crate) status: String,
    pub(crate) recommendation: String,
    pub(crate) validate: String,
    pub(crate) audit: String,
    pub(crate) bench: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct MemoryRoutingMeasurement {
    pub(crate) memory_id: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) used: usize,
    pub(crate) helped_candidate: usize,
    pub(crate) hurt_candidate: usize,
    pub(crate) helped: usize,
    pub(crate) hurt: usize,
    pub(crate) causal_helped: usize,
    pub(crate) causal_hurt: usize,
    pub(crate) help_rate: f64,
    pub(crate) hurt_rate: f64,
    pub(crate) decision: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct MemoryAdvanceGuard {
    pub(crate) memory_id: String,
    pub(crate) status: String,
    pub(crate) proposal_file: String,
    pub(crate) can_compile_deterministic_guard: bool,
}

#[derive(Debug, Serialize)]
struct BrainOutput {
    schema: &'static str,
    memory_dir: String,
    dream_dir: String,
    summary: BrainSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory: Option<BrainMemorySection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skills: Option<BrainSkillSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policies: Option<BrainPolicySection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    findings: Option<BrainFindingSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates: Option<BrainCandidateSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    view: Option<Value>,
    next_commands: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
struct BrainSummary {
    memory: BTreeMap<String, usize>,
    skills: BTreeMap<String, usize>,
    policies: BTreeMap<String, usize>,
    findings: BTreeMap<String, usize>,
    candidates: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct BrainMemorySection {
    by_status: BTreeMap<String, Vec<BrainMemoryItem>>,
}

#[derive(Debug, Serialize)]
struct BrainMemoryItem {
    id: String,
    kind: String,
    status: String,
    title: String,
    confidence: f64,
    impact: f64,
    used: usize,
    helped_candidate: usize,
    hurt_candidate: usize,
    helped: usize,
    hurt: usize,
    causal_helped: usize,
    causal_hurt: usize,
    can_prompt_inject: bool,
    can_route: bool,
    can_compile_skill: bool,
    can_policy: bool,
    path: String,
    next_action: String,
}

#[derive(Debug, Serialize)]
struct BrainSkillSection {
    installed: Vec<BrainSkillItem>,
    drafts: Vec<BrainSkillDraftItem>,
}

#[derive(Debug, Serialize)]
struct BrainSkillItem {
    id: String,
    mode: String,
    version: Option<String>,
    description: Option<String>,
    path: String,
    source_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct BrainSkillDraftItem {
    id: String,
    memory_id: Option<String>,
    status: String,
    recommendation: Option<String>,
    validate: Option<String>,
    audit: Option<String>,
    bench: Option<String>,
    path: String,
    next_action: String,
}

#[derive(Debug, Serialize)]
struct BrainPolicySection {
    memory_policies: Vec<BrainPolicyItem>,
    guard_proposals: Vec<BrainGuardProposalItem>,
}

#[derive(Debug, Serialize)]
struct BrainPolicyItem {
    id: String,
    status: String,
    title: String,
    confidence: f64,
    impact: f64,
    can_policy: bool,
    path: String,
    next_action: String,
}

#[derive(Debug, Serialize)]
struct BrainGuardProposalItem {
    memory_id: String,
    status: String,
    can_compile_deterministic_guard: bool,
    suggested_runtime_rule: String,
    affected_files: Vec<String>,
    path: String,
}

#[derive(Debug, Serialize)]
struct BrainFindingSection {
    by_status: BTreeMap<String, Vec<BrainFindingItem>>,
}

#[derive(Debug, Serialize)]
struct BrainFindingItem {
    id: String,
    display_id: String,
    category: String,
    status: String,
    summary: String,
    impact_score: i64,
    recurrence_count: u64,
    linked_regressions: Vec<String>,
    linked_candidates: Vec<String>,
    recurred_after_fix: bool,
    next_action: String,
}

#[derive(Debug, Serialize)]
struct BrainCandidateSection {
    items: Vec<BrainCandidateItem>,
}

#[derive(Debug, Serialize)]
struct BrainCandidateItem {
    id: String,
    finding: String,
    status: String,
    decision: Option<String>,
    score: Option<f64>,
    path: String,
    eval: Option<String>,
    report: Option<String>,
    next_action: String,
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

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
struct MemoryUsageStats {
    used: usize,
    helped_candidate: usize,
    hurt_candidate: usize,
    helped_confirmed: usize,
    hurt_confirmed: usize,
    causal_helped: usize,
    causal_hurt: usize,
    neutral: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct MemoryUsageScorecardFile {
    schema: String,
    compacted_at_unix: u64,
    stats: BTreeMap<String, MemoryUsageStats>,
}

struct SkillDraftResult {
    memory_id: String,
    slug: String,
    draft_dir: PathBuf,
    manifest_path: PathBuf,
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
        "manifest_file": draft.manifest_path.display().to_string(),
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
        None,
    )?;
    let audit = run_air_check(
        &["skill", "audit", draft.draft_dir.to_string_lossy().as_ref()],
        &log_dir.join("audit.log"),
        None,
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
            None,
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
    let manifest_path = draft_dir.join("air-skill.yaml");
    let skill_path = draft_dir.join("SKILL.md");
    let evidence_path = draft_dir.join("evidence.json");
    fs::write(&manifest_path, render_skill_manifest_draft(&card, &slug))
        .with_context(|| format!("write {}", manifest_path.display()))?;
    fs::write(&skill_path, render_skill_draft(&card, &slug))
        .with_context(|| format!("write {}", skill_path.display()))?;
    fs::write(&evidence_path, serde_json::to_vec_pretty(&card.evidence)?)
        .with_context(|| format!("write {}", evidence_path.display()))?;
    Ok(SkillDraftResult {
        memory_id: card.id,
        slug,
        draft_dir,
        manifest_path,
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
    if matches!(status.as_str(), "promoted" | "pinned")
        && !has_causal_promotion_evidence(&memory_dir, &card.id)?
    {
        anyhow::bail!(
            "memory {} cannot be promoted to {status} without causal eval evidence; run `air memory causal-eval {} --outcome helped --evidence <compare-no-memory.json>` first, or use --status validated",
            card.id,
            card.id
        );
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
    write_card_snapshot(&path, &card)?;
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

pub(crate) fn causal_eval_memory(options: MemoryCausalEvalOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let (card, path) = find_card(&memory_dir, &options.id)?;
    if !matches!(options.outcome.as_str(), "helped" | "hurt" | "neutral") {
        bail!("unsupported causal memory outcome: {}", options.outcome);
    }
    let evidence = absolutize(&cwd, options.evidence);
    if !evidence.exists() {
        bail!(
            "causal eval evidence does not exist: {}",
            evidence.display()
        );
    }
    let outcome = match options.outcome.as_str() {
        "helped" => "causal_helped",
        "hurt" => "causal_hurt",
        _ => "causal_neutral",
    };
    let now = unix_now();
    append_jsonl(
        &memory_dir.join("usage.jsonl"),
        &MemoryUsageEvent {
            schema: "air.memory_usage.v1".to_string(),
            memory_id: card.id.clone(),
            task: options.note.unwrap_or_default(),
            outcome: outcome.to_string(),
            run_artifact: Some(evidence.display().to_string()),
            at_unix: now,
        },
    )?;
    append_ledger(
        &memory_dir,
        "memory_causal_eval_recorded",
        &card.id,
        &card.kind,
        &path.display().to_string(),
        now,
    )?;
    compact_usage_log_if_needed(&memory_dir, now)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "air.memory_causal_eval.v1",
            "status": "recorded",
            "id": card.id,
            "kind": card.kind,
            "outcome": outcome,
            "evidence": evidence.display().to_string()
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
    write_card_snapshot(&path, &card)?;
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
    let stats = memory_usage_stats_for_dir(&memory_dir)?;
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
            causal_helped: stats.causal_helped,
            causal_hurt: stats.causal_hurt,
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

pub(crate) fn show_brain(options: BrainOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let layout = AirLayout::new(cwd.clone());
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let dream_dir = absolutize(
        &cwd,
        options.dream_dir.unwrap_or_else(|| layout.dream_dir()),
    );
    let section = options.section;
    let effective_limit = if matches!(section, BrainSection::Summary) {
        options.limit.or(Some(5))
    } else {
        options.limit
    };
    let brain = build_brain_index(&cwd, &memory_dir, &dream_dir, effective_limit)?;
    match section {
        BrainSection::Summary => {
            print_brain_output(
                BrainOutput {
                    schema: "air.brain.v1",
                    memory_dir: memory_dir.display().to_string(),
                    dream_dir: dream_dir.display().to_string(),
                    summary: brain.summary,
                    memory: Some(brain.memory),
                    skills: Some(brain.skills),
                    policies: Some(brain.policies),
                    findings: Some(brain.findings),
                    candidates: Some(brain.candidates),
                    view: None,
                    next_commands: brain_next_commands(),
                },
                options.json,
            )?;
        }
        BrainSection::Memory => {
            print_brain_output(
                BrainOutput {
                    schema: "air.brain.v1",
                    memory_dir: memory_dir.display().to_string(),
                    dream_dir: dream_dir.display().to_string(),
                    summary: brain.summary,
                    memory: Some(brain.memory),
                    skills: None,
                    policies: None,
                    findings: None,
                    candidates: None,
                    view: None,
                    next_commands: brain_next_commands(),
                },
                options.json,
            )?;
        }
        BrainSection::Skills => {
            print_brain_output(
                BrainOutput {
                    schema: "air.brain.v1",
                    memory_dir: memory_dir.display().to_string(),
                    dream_dir: dream_dir.display().to_string(),
                    summary: brain.summary,
                    memory: None,
                    skills: Some(brain.skills),
                    policies: None,
                    findings: None,
                    candidates: None,
                    view: None,
                    next_commands: brain_next_commands(),
                },
                options.json,
            )?;
        }
        BrainSection::Policies => {
            print_brain_output(
                BrainOutput {
                    schema: "air.brain.v1",
                    memory_dir: memory_dir.display().to_string(),
                    dream_dir: dream_dir.display().to_string(),
                    summary: brain.summary,
                    memory: None,
                    skills: None,
                    policies: Some(brain.policies),
                    findings: None,
                    candidates: None,
                    view: None,
                    next_commands: brain_next_commands(),
                },
                options.json,
            )?;
        }
        BrainSection::Findings => {
            print_brain_output(
                BrainOutput {
                    schema: "air.brain.v1",
                    memory_dir: memory_dir.display().to_string(),
                    dream_dir: dream_dir.display().to_string(),
                    summary: brain.summary,
                    memory: None,
                    skills: None,
                    policies: None,
                    findings: Some(brain.findings),
                    candidates: None,
                    view: None,
                    next_commands: brain_next_commands(),
                },
                options.json,
            )?;
        }
        BrainSection::Candidates => {
            print_brain_output(
                BrainOutput {
                    schema: "air.brain.v1",
                    memory_dir: memory_dir.display().to_string(),
                    dream_dir: dream_dir.display().to_string(),
                    summary: brain.summary,
                    memory: None,
                    skills: None,
                    policies: None,
                    findings: None,
                    candidates: Some(brain.candidates),
                    view: None,
                    next_commands: brain_next_commands(),
                },
                options.json,
            )?;
        }
        BrainSection::View(id) => {
            let view = brain_view(&brain, &id)?;
            print_brain_output(
                BrainOutput {
                    schema: "air.brain.v1",
                    memory_dir: memory_dir.display().to_string(),
                    dream_dir: dream_dir.display().to_string(),
                    summary: brain.summary,
                    memory: None,
                    skills: None,
                    policies: None,
                    findings: None,
                    candidates: None,
                    view: Some(view),
                    next_commands: brain_next_commands(),
                },
                options.json,
            )?;
        }
        BrainSection::Report => {
            let out = absolutize(
                &cwd,
                options
                    .out
                    .unwrap_or_else(|| PathBuf::from(".air/brain/report.md")),
            );
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            fs::write(&out, render_brain_report(&brain))
                .with_context(|| format!("write {}", out.display()))?;
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                    "schema": "air.brain_report.v1",
                    "report": out.display().to_string(),
                    "summary": brain.summary,
                    "next_commands": [
                        "air brain",
                        "air brain memory",
                        "air brain skills",
                        "air brain policies",
                        "air brain findings",
                        "air brain candidates"
                    ]
                    }))?
                );
            } else {
                println!("AIR brain report written to {}", out.display());
            }
        }
    }
    Ok(())
}

fn print_brain_output(output: BrainOutput, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", render_brain_text(&output));
    }
    Ok(())
}

fn render_brain_text(output: &BrainOutput) -> String {
    let mut out = String::new();
    out.push_str("AIR Agent Brain\n\n");
    out.push_str("A readable digest of what AIR has learned, what it can use now, and what still needs review.\n\n");
    out.push_str("At a glance\n");
    out.push_str(&format!(
        "- Memory: {}\n- Skills: {}\n- Policies: {}\n- Findings: {}\n- Candidates: {}\n\n",
        friendly_counts(&output.summary.memory),
        friendly_counts(&output.summary.skills),
        friendly_counts(&output.summary.policies),
        friendly_counts(&output.summary.findings),
        friendly_counts(&output.summary.candidates)
    ));
    if let Some(memory) = &output.memory {
        render_brain_memory_text(&mut out, memory);
    }
    if let Some(skills) = &output.skills {
        render_brain_skills_text(&mut out, skills);
    }
    if let Some(policies) = &output.policies {
        render_brain_policies_text(&mut out, policies);
    }
    if let Some(findings) = &output.findings {
        render_brain_findings_text(&mut out, findings);
    }
    if let Some(candidates) = &output.candidates {
        render_brain_candidates_text(&mut out, candidates);
    }
    if let Some(view) = &output.view {
        render_brain_view_text(&mut out, view);
    }
    out.push_str("Useful commands\n");
    for command in &output.next_commands {
        out.push_str(&format!("- {command}\n"));
    }
    out.push_str("\nUse --json for machine-readable output.\n");
    out
}

fn render_brain_candidates_text(out: &mut String, candidates: &BrainCandidateSection) {
    out.push_str("Dream experiment candidates\n");
    if candidates.items.is_empty() {
        out.push_str("- No Dream experiment candidates found yet.\n\n");
        return;
    }
    for item in &candidates.items {
        out.push_str(&format!(
            "- {} for {}: {}\n  decision: {}; score: {}; eval: {}\n  next: {}\n",
            item.id,
            item.finding,
            item.status,
            item.decision.as_deref().unwrap_or("unknown"),
            item.score
                .map(|score| format!("{score:.2}"))
                .unwrap_or_else(|| "n/a".to_string()),
            item.eval.as_deref().unwrap_or("not written"),
            item.next_action
        ));
    }
    out.push('\n');
}

fn render_brain_memory_text(out: &mut String, memory: &BrainMemorySection) {
    out.push_str("What AIR remembers\n");
    for status in ["pinned", "promoted", "validated", "candidate", "retired"] {
        let Some(items) = memory.by_status.get(status) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{}\n", memory_group_label(status)));
        for item in items {
            out.push_str(&format!(
                "- {}: {}\n  type: {}; confidence {:.0}%, impact {:.0}%\n  evidence so far: {} helpful run(s), {} proven helpful, {} harmful run(s)\n  next: {}\n",
                item.id,
                item.title,
                item.kind,
                item.confidence * 100.0,
                item.impact * 100.0,
                item.helped_candidate,
                item.causal_helped,
                item.hurt_candidate,
                user_action_text(&item.next_action)
            ));
        }
    }
    out.push('\n');
}

fn render_brain_skills_text(out: &mut String, skills: &BrainSkillSection) {
    out.push_str("Skills AIR can load or is learning\n\n");
    if skills.installed.is_empty() {
        out.push_str("Installed skills: none found\n");
    } else {
        out.push_str("Installed skills already available\n");
        for skill in &skills.installed {
            out.push_str(&format!(
                "- {} ({}){}\n",
                skill.id,
                skill_mode_label(&skill.mode),
                skill
                    .description
                    .as_ref()
                    .map(|description| format!(": {}", truncate_text(description, 140)))
                    .unwrap_or_default()
            ));
        }
    }
    if skills.drafts.is_empty() {
        out.push_str("\nDream-compiled skill drafts: none\n\n");
    } else {
        out.push_str("\nDream-compiled skill drafts\n");
        for draft in &skills.drafts {
            out.push_str(&format!(
                "- {}: {}\n  checks: validate {}, audit {}, benchmark {}\n  next: {}\n",
                draft.id,
                skill_draft_status_label(&draft.status),
                check_label(draft.validate.as_deref()),
                check_label(draft.audit.as_deref()),
                check_label(draft.bench.as_deref()),
                user_action_text(&draft.next_action)
            ));
        }
        out.push('\n');
    }
}

fn render_brain_policies_text(out: &mut String, policies: &BrainPolicySection) {
    out.push_str("Safety rules AIR is considering\n\n");
    if policies.memory_policies.is_empty() {
        out.push_str("Policy memories: none\n");
    } else {
        out.push_str("Policy ideas from Dream\n");
        for policy in &policies.memory_policies {
            out.push_str(&format!(
                "- {}: {}\n  status: {}; next: {}\n",
                policy.id,
                policy.title,
                status_label(&policy.status),
                user_action_text(&policy.next_action)
            ));
        }
    }
    if policies.guard_proposals.is_empty() {
        out.push_str("\nReviewed guard proposals: none\n\n");
    } else {
        out.push_str("\nReviewed guard proposals\n");
        for guard in &policies.guard_proposals {
            out.push_str(&format!(
                "- {}: {}\n  would touch: {}\n",
                guard.memory_id,
                guard.suggested_runtime_rule,
                if guard.affected_files.is_empty() {
                    "unknown".to_string()
                } else {
                    guard.affected_files.join(", ")
                }
            ));
        }
        out.push('\n');
    }
}

fn render_brain_findings_text(out: &mut String, findings: &BrainFindingSection) {
    out.push_str("Problems AIR saw recently\n");
    for status in ["open", "resolved", "dismissed"] {
        let Some(items) = findings.by_status.get(status) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{}\n", finding_group_label(status)));
        for finding in items {
            out.push_str(&format!(
                "- {}: {}\n  impact score {}, seen {} time(s)\n  {}\n  next: {}\n",
                finding.id,
                finding_category_label(&finding.category),
                finding.impact_score,
                finding.recurrence_count,
                finding.summary,
                user_action_text(&finding.next_action)
            ));
        }
    }
    out.push('\n');
}

fn render_brain_view_text(out: &mut String, view: &Value) {
    out.push_str("Detail\n\n");
    let kind = view.get("kind").and_then(Value::as_str).unwrap_or("object");
    out.push_str(&format!("Type: {kind}\n"));
    match kind {
        "memory" => {
            out.push_str(&format!(
                "ID: {}\nStatus: {}\nKind: {}\nTitle: {}\n\n{}\n\n",
                view.get("id").and_then(Value::as_str).unwrap_or("unknown"),
                view.get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("memory_kind")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("content").and_then(Value::as_str).unwrap_or("")
            ));
            if let Some(evidence) = view.get("evidence").and_then(Value::as_array) {
                out.push_str("Evidence\n");
                for item in evidence {
                    out.push_str(&format!(
                        "- {}: {}{}\n",
                        item.get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("evidence"),
                        item.get("path")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown"),
                        item.get("verdict")
                            .and_then(Value::as_str)
                            .map(|verdict| format!(" ({verdict})"))
                            .unwrap_or_default()
                    ));
                }
            }
        }
        "skill_draft" => {
            out.push_str(&format!(
                "Skill: {}\nStatus: {}\nValidate: {}\nAudit: {}\nBench: {}\nNext: {}\nPath: {}\n",
                view.get("id").and_then(Value::as_str).unwrap_or("unknown"),
                view.get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("validate")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("audit")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("bench")
                    .and_then(Value::as_str)
                    .unwrap_or("pending"),
                view.get("next_action")
                    .and_then(Value::as_str)
                    .unwrap_or("review"),
                view.get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ));
        }
        "guard_proposal" => {
            out.push_str(&format!(
                "Memory: {}\nStatus: {}\nRule: {}\nPath: {}\n",
                view.get("memory_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("suggested_runtime_rule")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                view.get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ));
        }
        "finding" => {
            if let Some(record) = view.get("record") {
                out.push_str(&format!(
                    "Finding: {}\nStatus: {}\nCategory: {}\nScore: {}\nSummary: {}\n",
                    record
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown"),
                    record
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown"),
                    record
                        .get("category")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown"),
                    record
                        .get("impact_score")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                    record.get("summary").and_then(Value::as_str).unwrap_or("")
                ));
            }
        }
        _ => out.push_str(&serde_json::to_string_pretty(view).unwrap_or_default()),
    }
    out.push('\n');
}

fn friendly_counts(counts: &BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return "none".to_string();
    }
    counts
        .iter()
        .map(|(key, value)| format!("{} {}", value, count_label(key, *value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn count_label(key: &str, count: usize) -> String {
    match key {
        "candidate" => plural(count, "new idea to review", "new ideas to review").to_string(),
        "promoted" => plural(count, "ready-to-use memory", "ready-to-use memories").to_string(),
        "validated" => plural(count, "validated memory", "validated memories").to_string(),
        "pinned" => plural(count, "pinned memory", "pinned memories").to_string(),
        "retired" => plural(count, "retired memory", "retired memories").to_string(),
        "validated_skill" => {
            plural(count, "validated skill draft", "validated skill drafts").to_string()
        }
        "installed" => plural(count, "installed skill", "installed skills").to_string(),
        "guard_proposals" => plural(count, "guard proposal", "guard proposals").to_string(),
        "memory_policy_cards" => plural(count, "policy idea", "policy ideas").to_string(),
        "open" => plural(count, "open problem", "open problems").to_string(),
        "resolved" => plural(count, "resolved problem", "resolved problems").to_string(),
        "dismissed" => plural(count, "dismissed problem", "dismissed problems").to_string(),
        _ => key.replace('_', " "),
    }
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn truncate_text(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(limit.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn memory_group_label(status: &str) -> &'static str {
    match status {
        "pinned" => "Pinned memory: always included",
        "promoted" => "Ready-to-use memory: included in future runs",
        "validated" => "Validated memory: needs causal proof before automatic use",
        "candidate" => "New ideas from Dream: review before use",
        "retired" => "Retired memory: no longer used",
        _ => "Other memory",
    }
}

fn finding_group_label(status: &str) -> &'static str {
    match status {
        "open" => "Open problems",
        "resolved" => "Resolved problems",
        "dismissed" => "Dismissed problems",
        _ => "Other problems",
    }
}

fn status_label(status: &str) -> &'static str {
    match status {
        "pinned" => "pinned and active",
        "promoted" => "ready to use",
        "validated" => "validated, needs causal proof",
        "candidate" => "new idea, needs review",
        "retired" => "retired",
        "open" => "open",
        "resolved" => "resolved",
        "dismissed" => "dismissed",
        _ => "unknown",
    }
}

fn skill_mode_label(mode: &str) -> &'static str {
    match mode {
        "executor" => "agent runtime",
        "instruction" => "workflow guide",
        _ => "skill",
    }
}

fn skill_draft_status_label(status: &str) -> &'static str {
    match status {
        "validated_skill" => "static checks passed; needs benchmark",
        "bench_passed" => "benchmark passed; ready for human import review",
        "skill_needs_review" => "failed checks; needs fixing",
        "draft" => "draft; not validated yet",
        _ => "unknown status",
    }
}

fn check_label(status: Option<&str>) -> &'static str {
    match status {
        Some("passed") => "passed",
        Some("failed") => "failed",
        Some("skipped") => "skipped",
        Some(_) => "unknown",
        None => "pending",
    }
}

fn finding_category_label(category: &str) -> &'static str {
    match category {
        "budget_exceeded" => "ran out of budget while working",
        "diff_constraint_failed" => "changed the wrong files or missed constraints",
        "no_patch_applied" => "did not make a code change",
        "verification_failed" => "verification was missing or failed",
        "skill_routing_miss" => "picked the wrong skill",
        "mcp_tool_failure" => "external tool failed",
        _ => "unknown problem",
    }
}

fn user_action_text(action: &str) -> String {
    if action.starts_with("air memory view ") {
        "review the evidence before using it".to_string()
    } else if action.starts_with("air memory skill-evaluate ") {
        "evaluate this as a possible reusable skill".to_string()
    } else if action.starts_with("air memory causal-eval ") {
        "run a compare-without-memory check before promotion".to_string()
    } else if action.starts_with("air memory policy-check ") {
        "review as a possible deterministic runtime guard".to_string()
    } else if action.starts_with("air improve promote ") {
        "turn this repeated problem into a regression candidate".to_string()
    } else {
        action.to_string()
    }
}

pub(crate) fn advance_memory(options: MemoryAdvanceOptions) -> Result<MemoryAdvanceOutput> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let memory_dir = memory_dir(&cwd, options.memory_dir);
    let out_dir = absolutize(
        &cwd,
        options
            .out_dir
            .unwrap_or_else(|| memory_dir.join("advance")),
    );
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    let now = unix_now();
    let _ = reconcile_memory_usage(&memory_dir, now)?;
    let stats = memory_usage_stats_for_dir(&memory_dir)?;

    let mut promoted_memories = Vec::new();
    let mut validated_memories = Vec::new();
    let mut retired_memories = Vec::new();
    let mut validated_skills = Vec::new();
    let mut routing_measurements = Vec::new();
    let mut reviewed_guards = Vec::new();
    let mut next_commands = Vec::new();
    let started = Instant::now();
    let timeout = options.timeout_seconds.map(Duration::from_secs);
    let mut cards = load_cards(&memory_dir)?;
    cards.sort_by(|(left, _), (right, _)| {
        advance_card_priority(right)
            .cmp(&advance_card_priority(left))
            .then_with(|| {
                right
                    .impact
                    .partial_cmp(&left.impact)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut touched = 0usize;

    for (mut card, path) in cards {
        if options.limit.is_some_and(|limit| touched >= limit) {
            break;
        }
        if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
            next_commands
                .push("memory advance stopped because --timeout-seconds was exhausted".to_string());
            break;
        }
        let item = stats.get(&card.id).copied().unwrap_or_default();
        let mut did_work = false;

        if card.status != "retired" && should_retire_from_advance(item) {
            card.status = "retired".to_string();
            card.updated_at = now;
            write_card_snapshot(&path, &card)?;
            append_ledger(
                &memory_dir,
                "memory_advance_retired",
                &card.id,
                &card.kind,
                &path.display().to_string(),
                now,
            )?;
            retired_memories.push(card.id.clone());
            touched += 1;
            continue;
        }

        if card.status == "candidate" && item.helped_confirmed > 0 {
            card.status = "validated".to_string();
            card.updated_at = now;
            write_card_snapshot(&path, &card)?;
            append_ledger(
                &memory_dir,
                "memory_advance_validated",
                &card.id,
                &card.kind,
                &path.display().to_string(),
                now,
            )?;
            validated_memories.push(card.id.clone());
            did_work = true;
        }

        if matches!(card.status.as_str(), "candidate" | "validated")
            && item.causal_helped > 0
            && card.promotion.can_prompt_inject
        {
            card.status = "promoted".to_string();
            card.updated_at = now;
            card.promotion.requires_human_review = false;
            if matches!(
                card.kind.as_str(),
                "routing" | "procedure" | "concept" | "hypothesis"
            ) {
                card.promotion.can_route = true;
            }
            write_card_snapshot(&path, &card)?;
            append_ledger(
                &memory_dir,
                "memory_advance_promoted",
                &card.id,
                &card.kind,
                &path.display().to_string(),
                now,
            )?;
            promoted_memories.push(card.id.clone());
            did_work = true;
        }

        if has_routing_measurement_signal(item)
            && (card.promotion.can_route || matches!(card.kind.as_str(), "routing" | "procedure"))
        {
            let evaluated = item.helped_candidate + item.hurt_candidate + item.neutral;
            let decision = if item.causal_hurt > 0 || item.hurt_confirmed > 0 {
                "avoid_or_retire"
            } else if item.causal_helped > 0 {
                "measured_improvement"
            } else if item.helped_confirmed > 0 {
                "validated_correlation"
            } else {
                "needs_causal_eval"
            };
            routing_measurements.push(MemoryRoutingMeasurement {
                memory_id: card.id.clone(),
                kind: card.kind.clone(),
                status: card.status.clone(),
                used: item.used,
                helped_candidate: item.helped_candidate,
                hurt_candidate: item.hurt_candidate,
                helped: item.helped_confirmed,
                hurt: item.hurt_confirmed,
                causal_helped: item.causal_helped,
                causal_hurt: item.causal_hurt,
                help_rate: ratio(item.helped_candidate, evaluated),
                hurt_rate: ratio(item.hurt_candidate, evaluated),
                decision: decision.to_string(),
            });
            did_work = true;
        }

        if matches!(card.status.as_str(), "validated" | "promoted")
            && card.kind == "procedure"
            && card.promotion.can_compile_skill
        {
            let remaining = timeout.map(|timeout| timeout.saturating_sub(started.elapsed()));
            let skill = advance_skill_candidate(
                &memory_dir,
                &card,
                &out_dir,
                options.skill_bench,
                remaining,
            )?;
            if skill.status == "validated_skill" {
                validated_skills.push(skill);
            } else {
                next_commands.push(format!("air memory skill-evaluate {}", card.id));
                validated_skills.push(skill);
            }
            did_work = true;
        }

        if card.kind == "policy"
            && matches!(card.status.as_str(), "candidate" | "validated" | "promoted")
        {
            let guard = write_policy_review(&memory_dir, &card, &out_dir, now)?;
            if guard.can_compile_deterministic_guard {
                next_commands.push(format!("air memory policy-check {}", card.id));
            }
            reviewed_guards.push(guard);
            did_work = true;
        }

        if did_work {
            touched += 1;
        }
    }

    if !routing_measurements.is_empty() {
        fs::write(
            out_dir.join("routing-scorecard.json"),
            serde_json::to_vec_pretty(&routing_measurements)?,
        )
        .with_context(|| format!("write {}", out_dir.join("routing-scorecard.json").display()))?;
    }

    for id in &promoted_memories {
        next_commands.push(format!(
            "air memory pack <task> # now includes {id} when relevant"
        ));
    }
    for skill in &validated_skills {
        if skill.status == "validated_skill" {
            next_commands.push(format!(
                "air skill import {} --out skills/generated/{}",
                skill.draft_dir,
                slugify(&skill.memory_id)
            ));
        }
    }
    if !reviewed_guards.is_empty() {
        next_commands.push(
            "review guard proposals and implement accepted ones as deterministic runtime patches"
                .to_string(),
        );
    }

    let output = MemoryAdvanceOutput {
        schema: MEMORY_ADVANCE_SCHEMA,
        status: "advanced".to_string(),
        memory_dir: memory_dir.display().to_string(),
        out_dir: out_dir.display().to_string(),
        promoted_memories,
        validated_memories,
        retired_memories,
        validated_skills,
        routing_measurements,
        reviewed_guards,
        next_commands,
    };
    fs::write(
        out_dir.join("advance.json"),
        serde_json::to_vec_pretty(&output)?,
    )
    .with_context(|| format!("write {}", out_dir.join("advance.json").display()))?;
    write_advance_report(&out_dir.join("advance.md"), &output)?;
    Ok(output)
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
        next_commands.push(format!("air run {}", shell_quote_like(&options.task)));
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
            "Promoted memories match this task and will be included by default.".to_string()
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
    let mut by_category: BTreeMap<String, Vec<&MemoryEpisode>> = BTreeMap::new();
    for episode in episodes {
        by_category
            .entry(episode.category.clone())
            .or_default()
            .push(episode);
    }
    for (category, category_episodes) in &by_category {
        let successes = category_episodes
            .iter()
            .filter(|episode| episode.verdict == "success")
            .count();
        let failures = category_episodes
            .iter()
            .filter(|episode| episode.verdict != "success")
            .count();
        if successes > 0 && failures > 0 && mode != "micro" {
            let derived_from = category_episodes
                .iter()
                .take(12)
                .map(|episode| episode.id.clone())
                .collect::<Vec<_>>();
            candidates.push(DreamIrCandidate {
                id: memory_id("dream_ir", &["procedure_delta", category]),
                kind: "procedure_candidate".to_string(),
                name: format!("{}_success_failure_delta", category.replace('-', "_")),
                confidence: confidence_from_count(successes + failures),
                description: format!(
                    "Dream compared successful and failed `{category}` episodes and found enough mixed evidence to draft a procedure delta. This candidate should be turned into a skill only after compare-no-memory or replay evidence."
                ),
                derived_from,
                generalizes: vec![category.clone()],
                suggested_actions: vec![
                    DreamIrAction {
                        kind: "skill_draft_candidate".to_string(),
                        name: format!("{}_procedure_delta", category.replace('-', "_")),
                        detail: "Draft a skill from the successful trace shape, then run skill-evaluate with benchmark/replay evidence before promotion.".to_string(),
                    },
                    DreamIrAction {
                        kind: "causal_eval_required".to_string(),
                        name: "compare_no_memory_before_promotion".to_string(),
                        detail: "Record causal eval evidence with `air memory causal-eval` before promoting this memory.".to_string(),
                    },
                ],
            });
        }
    }
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
    compact_usage_log_if_needed(memory_dir, now)?;
    Ok(())
}

fn run_air_check(args: &[&str], log_path: &Path, timeout: Option<Duration>) -> Result<StageCheck> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    let output = run_current_air_stage(&cwd, &args, log_path, timeout)?;
    Ok(StageCheck {
        command: format!("air {}", args.join(" ")),
        status: if output.success {
            "passed".to_string()
        } else if output.status == "timeout" || output.status == "timeout_before_start" {
            output.status
        } else {
            "failed".to_string()
        },
        code: output.code,
        log: log_path.display().to_string(),
    })
}

fn policy_affected_files(card: &MemoryCard) -> Vec<String> {
    let _ = card;
    Vec::new()
}

fn advance_card_priority(card: &MemoryCard) -> u8 {
    match (card.status.as_str(), card.kind.as_str()) {
        ("promoted", "policy") | ("validated", "policy") | ("candidate", "policy") => 7,
        ("validated", "procedure") | ("promoted", "procedure") => 6,
        ("validated", _) | ("promoted", _) => 5,
        ("candidate", "routing") | ("candidate", "procedure") => 4,
        ("candidate", _) => 3,
        ("retired", _) => 0,
        _ => 1,
    }
}

fn should_retire_from_advance(item: MemoryUsageStats) -> bool {
    let confirmed_help = item.helped_confirmed + item.causal_helped;
    let confirmed_hurt = item.hurt_confirmed + item.causal_hurt;
    let confirmed_total = confirmed_help + confirmed_hurt;
    if confirmed_hurt > confirmed_help && confirmed_total >= 2 {
        return true;
    }
    let evaluated = item.helped_candidate + item.hurt_candidate + item.neutral;
    evaluated >= 10 && ratio(item.hurt_candidate, evaluated) >= 0.3
}

fn has_routing_measurement_signal(item: MemoryUsageStats) -> bool {
    item.used
        + item.helped_candidate
        + item.hurt_candidate
        + item.helped_confirmed
        + item.hurt_confirmed
        + item.causal_helped
        + item.causal_hurt
        + item.neutral
        > 0
}

fn advance_skill_candidate(
    memory_dir: &Path,
    card: &MemoryCard,
    out_dir: &Path,
    bench: bool,
    timeout: Option<Duration>,
) -> Result<MemoryAdvanceSkill> {
    let draft_dir = out_dir.join("skills").join(slugify(&card.title));
    let draft = create_skill_draft(MemorySkillDraftOptions {
        memory_dir: Some(memory_dir.to_path_buf()),
        id: card.id.clone(),
        out_dir: Some(draft_dir),
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
        timeout,
    )?;
    let audit = run_air_check(
        &["skill", "audit", draft.draft_dir.to_string_lossy().as_ref()],
        &log_dir.join("audit.log"),
        timeout,
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
            timeout,
        )?)
    } else {
        None
    };
    let passed = validate.status == "passed"
        && audit.status == "passed"
        && bench_check
            .as_ref()
            .is_none_or(|check| check.status == "passed");
    let recommendation = if !passed {
        "reject_or_fix_draft"
    } else if bench_check.is_none() {
        "validated_by_static_gates_needs_benchmark"
    } else {
        "ready_for_human_import_review"
    };
    let output = serde_json::json!({
        "schema": "air.memory_skill_advance.v1",
        "memory_id": card.id,
        "draft_dir": draft.draft_dir.display().to_string(),
        "validate": validate,
        "audit": audit,
        "bench": bench_check,
        "recommendation": recommendation
    });
    fs::write(
        draft.draft_dir.join("skill-advance.json"),
        serde_json::to_vec_pretty(&output)?,
    )
    .with_context(|| {
        format!(
            "write {}",
            draft.draft_dir.join("skill-advance.json").display()
        )
    })?;
    let validate_status = output
        .get("validate")
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let audit_status = output
        .get("audit")
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let bench_status = output
        .get("bench")
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(MemoryAdvanceSkill {
        memory_id: card.id.clone(),
        draft_dir: draft.draft_dir.display().to_string(),
        status: if passed {
            "validated_skill".to_string()
        } else {
            "skill_needs_review".to_string()
        },
        recommendation: recommendation.to_string(),
        validate: validate_status,
        audit: audit_status,
        bench: bench_status,
    })
}

fn write_policy_review(
    memory_dir: &Path,
    card: &MemoryCard,
    out_dir: &Path,
    now: u64,
) -> Result<MemoryAdvanceGuard> {
    let can_compile = card.promotion.can_policy
        && !card.evidence.is_empty()
        && card.status != "retired"
        && !card.content.to_ascii_lowercase().contains("disable");
    let affected_files = policy_affected_files(card);
    let proposal = serde_json::json!({
        "schema": "air.memory_guard_review.v1",
        "status": if can_compile { "reviewed_guard_proposal" } else { "needs_review" },
        "memory_id": card.id,
        "policy": card.title,
        "can_compile_deterministic_guard": can_compile,
        "suggested_runtime_rule": card.content,
        "affected_files": affected_files,
        "required_tests": [
            "cargo test -p air-cli improve::tests",
            "cargo test -p air-cli memory::tests",
            "air improve evaluate <finding> --candidate <candidate-dir>"
        ],
        "human_gate": "Implement accepted policy as a normal reviewed patch; AIR does not auto-edit runtime guard code from memory.",
        "created_at": now
    });
    let guard_dir = out_dir.join("guards");
    fs::create_dir_all(&guard_dir).with_context(|| format!("create {}", guard_dir.display()))?;
    let proposal_file = guard_dir.join(format!("{}.json", card.id));
    fs::write(&proposal_file, serde_json::to_vec_pretty(&proposal)?)
        .with_context(|| format!("write {}", proposal_file.display()))?;
    append_ledger(
        memory_dir,
        "memory_guard_reviewed",
        &card.id,
        &card.kind,
        &proposal_file.display().to_string(),
        now,
    )?;
    Ok(MemoryAdvanceGuard {
        memory_id: card.id.clone(),
        status: if can_compile {
            "reviewed_guard_proposal".to_string()
        } else {
            "needs_review".to_string()
        },
        proposal_file: proposal_file.display().to_string(),
        can_compile_deterministic_guard: can_compile,
    })
}

fn write_advance_report(path: &Path, output: &MemoryAdvanceOutput) -> Result<()> {
    let mut report = String::new();
    report.push_str("# AIR Memory Advance\n\n");
    report.push_str("This pass advances only evidence-gated memory. It may validate or promote memory with scorecard evidence, validate skill drafts, and write runtime guard proposals, but it does not import skills, pin memory, edit runtime code, commit, or open PRs.\n\n");
    report.push_str(&format!("- status: `{}`\n", output.status));
    report.push_str(&format!(
        "- promoted_memories: `{}`\n",
        output.promoted_memories.len()
    ));
    report.push_str(&format!(
        "- validated_memories: `{}`\n",
        output.validated_memories.len()
    ));
    report.push_str(&format!(
        "- retired_memories: `{}`\n",
        output.retired_memories.len()
    ));
    report.push_str(&format!(
        "- validated_skills: `{}`\n",
        output.validated_skills.len()
    ));
    report.push_str(&format!(
        "- routing_measurements: `{}`\n",
        output.routing_measurements.len()
    ));
    report.push_str(&format!(
        "- reviewed_guards: `{}`\n\n",
        output.reviewed_guards.len()
    ));
    if !output.promoted_memories.is_empty() {
        report.push_str("## Promoted Memory\n\n");
        for id in &output.promoted_memories {
            report.push_str(&format!("- `{id}`\n"));
        }
    }
    if !output.validated_skills.is_empty() {
        report.push_str("\n## Skill Drafts\n\n");
        for skill in &output.validated_skills {
            report.push_str(&format!(
                "- `{}` `{}` validate `{}` audit `{}` recommendation `{}`\n",
                skill.memory_id, skill.status, skill.validate, skill.audit, skill.recommendation
            ));
        }
    }
    if !output.reviewed_guards.is_empty() {
        report.push_str("\n## Runtime Guard Proposals\n\n");
        for guard in &output.reviewed_guards {
            report.push_str(&format!(
                "- `{}` `{}` proposal `{}`\n",
                guard.memory_id, guard.status, guard.proposal_file
            ));
        }
    }
    fs::write(path, report).with_context(|| format!("write {}", path.display()))
}

struct BrainIndex {
    summary: BrainSummary,
    memory: BrainMemorySection,
    skills: BrainSkillSection,
    policies: BrainPolicySection,
    findings: BrainFindingSection,
    candidates: BrainCandidateSection,
    cards: Vec<(MemoryCard, PathBuf)>,
    finding_records: Vec<Value>,
}

fn build_brain_index(
    cwd: &Path,
    memory_dir: &Path,
    dream_dir: &Path,
    limit: Option<usize>,
) -> Result<BrainIndex> {
    let cards = load_cards(memory_dir)?;
    let stats = memory_usage_stats_for_dir(memory_dir)?;
    let full_skills = brain_skill_section(cwd, memory_dir, dream_dir, None)?;
    let full_policies = brain_policy_section(&cards, dream_dir, None)?;
    let (full_findings, finding_records) = brain_finding_section(dream_dir, None)?;
    let full_candidates = brain_candidate_section(dream_dir, None)?;
    let summary = brain_summary(
        &cards,
        &full_skills,
        &full_policies,
        &full_findings,
        &full_candidates,
    );
    let memory = brain_memory_section(&cards, &stats, limit);
    let skills = brain_skill_section(cwd, memory_dir, dream_dir, limit)?;
    let policies = brain_policy_section(&cards, dream_dir, limit)?;
    let (findings, _) = brain_finding_section(dream_dir, limit)?;
    let candidates = brain_candidate_section(dream_dir, limit)?;
    Ok(BrainIndex {
        summary,
        memory,
        skills,
        policies,
        findings,
        candidates,
        cards,
        finding_records,
    })
}

fn brain_summary(
    cards: &[(MemoryCard, PathBuf)],
    skills: &BrainSkillSection,
    policies: &BrainPolicySection,
    findings: &BrainFindingSection,
    candidates: &BrainCandidateSection,
) -> BrainSummary {
    let mut summary = BrainSummary::default();
    for (card, _) in cards {
        *summary.memory.entry(card.status.clone()).or_default() += 1;
    }
    summary
        .skills
        .insert("installed".to_string(), skills.installed.len());
    for draft in &skills.drafts {
        *summary.skills.entry(draft.status.clone()).or_default() += 1;
    }
    summary.policies.insert(
        "memory_policy_cards".to_string(),
        policies.memory_policies.len(),
    );
    summary.policies.insert(
        "guard_proposals".to_string(),
        policies.guard_proposals.len(),
    );
    for records in findings.by_status.values() {
        for finding in records {
            *summary.findings.entry(finding.status.clone()).or_default() += 1;
        }
    }
    for candidate in &candidates.items {
        *summary
            .candidates
            .entry(candidate.status.clone())
            .or_default() += 1;
    }
    summary
}

fn brain_memory_section(
    cards: &[(MemoryCard, PathBuf)],
    stats: &BTreeMap<String, MemoryUsageStats>,
    limit: Option<usize>,
) -> BrainMemorySection {
    let mut items = cards
        .iter()
        .map(|(card, path)| {
            let item = stats.get(&card.id).copied().unwrap_or_default();
            BrainMemoryItem {
                id: card.id.clone(),
                kind: card.kind.clone(),
                status: card.status.clone(),
                title: card.title.clone(),
                confidence: card.confidence,
                impact: card.impact,
                used: item.used,
                helped_candidate: item.helped_candidate,
                hurt_candidate: item.hurt_candidate,
                helped: item.helped_confirmed,
                hurt: item.hurt_confirmed,
                causal_helped: item.causal_helped,
                causal_hurt: item.causal_hurt,
                can_prompt_inject: card.promotion.can_prompt_inject,
                can_route: card.promotion.can_route,
                can_compile_skill: card.promotion.can_compile_skill,
                can_policy: card.promotion.can_policy,
                path: path.display().to_string(),
                next_action: memory_next_action(card, item),
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        brain_status_rank(&right.status)
            .cmp(&brain_status_rank(&left.status))
            .then_with(|| {
                right
                    .impact
                    .partial_cmp(&left.impact)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = limit {
        items.truncate(limit);
    }
    let mut by_status = BTreeMap::<String, Vec<BrainMemoryItem>>::new();
    for item in items {
        by_status.entry(item.status.clone()).or_default().push(item);
    }
    BrainMemorySection { by_status }
}

fn brain_skill_section(
    cwd: &Path,
    memory_dir: &Path,
    dream_dir: &Path,
    limit: Option<usize>,
) -> Result<BrainSkillSection> {
    let dream_latest = brain_latest_run_dir(dream_dir);
    let mut installed = Vec::new();
    for manifest in collect_named_files(&cwd.join("skills"), "air-skill.yaml")? {
        if let Some(item) = read_brain_skill_item(&manifest)? {
            installed.push(item);
        }
    }
    installed.sort_by(|left, right| left.id.cmp(&right.id));
    if let Some(limit) = limit {
        installed.truncate(limit);
    }

    let mut draft_paths = Vec::new();
    draft_paths.extend(collect_named_files(
        &memory_dir.join("drafts").join("skills"),
        "air-skill.yaml",
    )?);
    draft_paths.extend(collect_named_files(
        &dream_latest.join("memory-advance").join("skills"),
        "air-skill.yaml",
    )?);
    let mut seen = BTreeSet::new();
    let mut drafts = Vec::new();
    for manifest in draft_paths {
        let key = manifest.display().to_string();
        if seen.insert(key) {
            if let Some(item) = read_brain_skill_draft_item(&manifest)? {
                drafts.push(item);
            }
        }
    }
    drafts.sort_by(|left, right| {
        brain_skill_status_rank(&right.status)
            .cmp(&brain_skill_status_rank(&left.status))
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = limit {
        drafts.truncate(limit);
    }
    Ok(BrainSkillSection { installed, drafts })
}

fn brain_policy_section(
    cards: &[(MemoryCard, PathBuf)],
    dream_dir: &Path,
    limit: Option<usize>,
) -> Result<BrainPolicySection> {
    let mut memory_policies = cards
        .iter()
        .filter(|(card, _)| card.kind == "policy")
        .map(|(card, path)| BrainPolicyItem {
            id: card.id.clone(),
            status: card.status.clone(),
            title: card.title.clone(),
            confidence: card.confidence,
            impact: card.impact,
            can_policy: card.promotion.can_policy,
            path: path.display().to_string(),
            next_action: if card.promotion.can_policy && card.status != "retired" {
                format!("air memory policy-check {}", card.id)
            } else {
                "no runtime guard action".to_string()
            },
        })
        .collect::<Vec<_>>();
    memory_policies.sort_by(|left, right| {
        brain_status_rank(&right.status)
            .cmp(&brain_status_rank(&left.status))
            .then_with(|| {
                right
                    .impact
                    .partial_cmp(&left.impact)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = limit {
        memory_policies.truncate(limit);
    }

    let guard_dir = brain_latest_run_dir(dream_dir)
        .join("memory-advance")
        .join("guards");
    let mut guard_proposals = collect_json_files(&guard_dir)?
        .into_iter()
        .filter_map(|path| read_brain_guard_proposal(&path).transpose())
        .collect::<Result<Vec<_>>>()?;
    guard_proposals.sort_by(|left, right| left.memory_id.cmp(&right.memory_id));
    if let Some(limit) = limit {
        guard_proposals.truncate(limit);
    }
    Ok(BrainPolicySection {
        memory_policies,
        guard_proposals,
    })
}

fn brain_latest_run_dir(dream_dir: &Path) -> PathBuf {
    let latest = dream_dir.join("latest");
    if latest.exists() {
        latest
    } else {
        dream_dir.to_path_buf()
    }
}

fn brain_findings_path(dream_dir: &Path) -> PathBuf {
    let direct = dream_dir.join("findings.jsonl");
    if direct.exists() {
        return direct;
    }
    dream_dir
        .parent()
        .map(|parent| parent.join("findings.jsonl"))
        .filter(|path| path.exists())
        .unwrap_or(direct)
}

fn brain_candidate_section(
    dream_dir: &Path,
    limit: Option<usize>,
) -> Result<BrainCandidateSection> {
    let mut items = Vec::new();
    let latest = brain_latest_run_dir(dream_dir);
    collect_brain_candidates_from_dir(&latest.join("candidates"), &mut items)?;
    let runs_dir = dream_dir.join("runs");
    if runs_dir.exists() {
        for entry in
            fs::read_dir(&runs_dir).with_context(|| format!("read {}", runs_dir.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                collect_brain_candidates_from_dir(&entry.path().join("candidates"), &mut items)?;
            }
        }
    }
    items.sort_by(|left, right| {
        candidate_status_rank(&right.status)
            .cmp(&candidate_status_rank(&left.status))
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    items.dedup_by(|left, right| left.path == right.path);
    if let Some(limit) = limit {
        items.truncate(limit);
    }
    Ok(BrainCandidateSection { items })
}

fn collect_brain_candidates_from_dir(
    dir: &Path,
    items: &mut Vec<BrainCandidateItem>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for finding_entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let finding_entry = finding_entry?;
        if !finding_entry.file_type()?.is_dir() {
            continue;
        }
        let finding = finding_entry.file_name().to_string_lossy().to_string();
        for candidate_entry in fs::read_dir(finding_entry.path())
            .with_context(|| format!("read {}", finding_entry.path().display()))?
        {
            let candidate_entry = candidate_entry?;
            if !candidate_entry.file_type()?.is_dir() {
                continue;
            }
            let path = candidate_entry.path();
            let id = candidate_entry.file_name().to_string_lossy().to_string();
            let eval_path = path.join("eval.json");
            let eval = read_json_file(&eval_path).ok();
            let decision = eval
                .as_ref()
                .and_then(|value| value.get("decision").or_else(|| value.get("status")))
                .and_then(Value::as_str)
                .map(str::to_string);
            let score = eval
                .as_ref()
                .and_then(|value| value.get("score"))
                .and_then(Value::as_f64);
            let status = decision.clone().unwrap_or_else(|| {
                if eval_path.exists() {
                    "evaluated".to_string()
                } else {
                    "generated".to_string()
                }
            });
            let report = [path.join("report.md"), path.join("eval.md")]
                .into_iter()
                .find(|path| path.exists())
                .map(|path| path.display().to_string());
            items.push(BrainCandidateItem {
                id,
                finding: finding.clone(),
                status,
                decision,
                score,
                path: path.display().to_string(),
                eval: eval_path.exists().then(|| eval_path.display().to_string()),
                report,
                next_action: "review candidate diff/eval before promotion".to_string(),
            });
        }
    }
    Ok(())
}

fn candidate_status_rank(status: &str) -> u8 {
    match status {
        "accept_candidate" | "accepted" => 4,
        "needs_review" | "evaluated" => 3,
        "generated" => 2,
        "reject_candidate" | "rejected" => 1,
        _ => 0,
    }
}

fn brain_finding_section(
    dream_dir: &Path,
    limit: Option<usize>,
) -> Result<(BrainFindingSection, Vec<Value>)> {
    let records = latest_jsonl_records(&brain_findings_path(dream_dir))?;
    let mut items = records.iter().map(brain_finding_item).collect::<Vec<_>>();
    items.sort_by(|left, right| {
        brain_finding_status_rank(&right.status)
            .cmp(&brain_finding_status_rank(&left.status))
            .then_with(|| right.impact_score.cmp(&left.impact_score))
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = limit {
        items.truncate(limit);
    }
    let mut by_status = BTreeMap::<String, Vec<BrainFindingItem>>::new();
    for item in items {
        by_status.entry(item.status.clone()).or_default().push(item);
    }
    Ok((BrainFindingSection { by_status }, records))
}

fn brain_finding_item(value: &Value) -> BrainFindingItem {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    BrainFindingItem {
        id: id.clone(),
        display_id: value
            .get("display_id")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_string(),
        category: value
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        status: status.clone(),
        summary: value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        impact_score: value
            .get("impact_score")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        recurrence_count: value
            .get("recurrence_count")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        linked_regressions: string_array(value.get("linked_regressions")),
        linked_candidates: string_array(value.get("linked_candidates")),
        recurred_after_fix: value
            .get("recurred_after_fix")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        next_action: finding_next_action(&id, &status),
    }
}

fn read_brain_skill_item(manifest: &Path) -> Result<Option<BrainSkillItem>> {
    let value = read_yaml_value(manifest)?;
    Ok(Some(BrainSkillItem {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        mode: value
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("executor")
            .to_string(),
        version: value
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        path: manifest.display().to_string(),
        source_type: value
            .get("source")
            .and_then(|source| source.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

fn read_brain_skill_draft_item(manifest: &Path) -> Result<Option<BrainSkillDraftItem>> {
    let value = read_yaml_value(manifest)?;
    let advance = manifest
        .parent()
        .map(|parent| parent.join("skill-advance.json"))
        .and_then(|path| read_json_file(&path).ok());
    let validate = advance
        .as_ref()
        .and_then(|value| value.pointer("/validate/status"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let audit = advance
        .as_ref()
        .and_then(|value| value.pointer("/audit/status"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let bench = advance
        .as_ref()
        .and_then(|value| value.pointer("/bench/status"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let status = advance
        .as_ref()
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(
            || match (validate.as_deref(), audit.as_deref(), bench.as_deref()) {
                (Some("passed"), Some("passed"), Some("passed")) => {
                    Some("bench_passed".to_string())
                }
                (Some("passed"), Some("passed"), _) => Some("validated_skill".to_string()),
                (Some("failed"), _, _) | (_, Some("failed"), _) => {
                    Some("skill_needs_review".to_string())
                }
                _ => None,
            },
        )
        .unwrap_or_else(|| "draft".to_string());
    Ok(Some(BrainSkillDraftItem {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        memory_id: value
            .pointer("/source/memory_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        status: status.clone(),
        recommendation: advance
            .as_ref()
            .and_then(|value| value.get("recommendation"))
            .and_then(Value::as_str)
            .map(str::to_string),
        validate,
        audit,
        bench,
        path: manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .display()
            .to_string(),
        next_action: skill_draft_next_action(&status),
    }))
}

fn read_brain_guard_proposal(path: &Path) -> Result<Option<BrainGuardProposalItem>> {
    let value = read_json_file(path)?;
    Ok(Some(BrainGuardProposalItem {
        memory_id: value
            .get("memory_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        can_compile_deterministic_guard: value
            .get("can_compile_deterministic_guard")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        suggested_runtime_rule: value
            .get("suggested_runtime_rule")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        affected_files: string_array(value.get("affected_files")),
        path: path.display().to_string(),
    }))
}

fn brain_view(brain: &BrainIndex, id: &str) -> Result<Value> {
    let mut matches = Vec::new();
    if let Some((card, path)) = brain.cards.iter().find(|(card, _)| card.id == id) {
        matches.push(json!({
            "kind": "memory",
            "id": card.id,
            "status": card.status,
            "memory_kind": card.kind,
            "title": card.title,
            "content": card.content,
            "triggers": card.triggers,
            "evidence": card.evidence,
            "promotion": card.promotion,
            "path": path.display().to_string(),
        }));
    }
    for draft in &brain.skills.drafts {
        if draft.id == id || draft.memory_id.as_deref() == Some(id) {
            matches.push(json!({
                "kind": "skill_draft",
                "id": draft.id,
                "memory_id": draft.memory_id,
                "status": draft.status,
                "recommendation": draft.recommendation,
                "validate": draft.validate,
                "audit": draft.audit,
                "bench": draft.bench,
                "path": draft.path,
                "next_action": draft.next_action,
            }));
        }
    }
    for guard in &brain.policies.guard_proposals {
        if guard.memory_id == id {
            matches.push(json!({
                "kind": "guard_proposal",
                "memory_id": guard.memory_id,
                "status": guard.status,
                "can_compile_deterministic_guard": guard.can_compile_deterministic_guard,
                "suggested_runtime_rule": guard.suggested_runtime_rule,
                "affected_files": guard.affected_files,
                "path": guard.path,
            }));
        }
    }
    for candidate in &brain.candidates.items {
        if candidate.id == id || candidate.path.ends_with(id) {
            matches.push(json!({
                "kind": "experiment_candidate",
                "id": candidate.id,
                "finding": candidate.finding,
                "status": candidate.status,
                "decision": candidate.decision,
                "score": candidate.score,
                "path": candidate.path,
                "eval": candidate.eval,
                "report": candidate.report,
                "next_action": candidate.next_action,
            }));
        }
    }
    matches.extend(
        brain
            .finding_records
            .iter()
            .filter(|record| {
                record.get("id").and_then(Value::as_str) == Some(id)
                    || record.get("display_id").and_then(Value::as_str) == Some(id)
                    || record
                        .get("aliases")
                        .and_then(Value::as_array)
                        .is_some_and(|aliases| {
                            aliases.iter().any(|alias| alias.as_str() == Some(id))
                        })
            })
            .cloned()
            .map(|record| json!({"kind": "finding", "record": record})),
    );
    match matches.len() {
        0 => bail!("brain object not found: {id}"),
        1 => Ok(matches.remove(0)),
        count => bail!(
            "brain object id is ambiguous: {id} matched {count} objects; use a more specific id"
        ),
    }
}

fn render_brain_report(brain: &BrainIndex) -> String {
    let mut out = String::new();
    out.push_str("# AIR Agent Brain\n\n");
    out.push_str("Organized view of Dream memory, compiled skill drafts, policy proposals, and persistent findings.\n\n");
    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- Memory: {}\n- Skills: {}\n- Policies: {}\n- Findings: {}\n- Candidates: {}\n\n",
        compact_counts(&brain.summary.memory),
        compact_counts(&brain.summary.skills),
        compact_counts(&brain.summary.policies),
        compact_counts(&brain.summary.findings),
        compact_counts(&brain.summary.candidates)
    ));
    out.push_str("## Promoted / Validated Memory\n\n");
    for status in ["pinned", "promoted", "validated"] {
        if let Some(items) = brain.memory.by_status.get(status) {
            for item in items.iter().take(20) {
                out.push_str(&format!(
                    "- `{}` `{}` `{}` helped_candidate `{}` causal_helped `{}`: {}\n",
                    item.id,
                    item.kind,
                    item.status,
                    item.helped_candidate,
                    item.causal_helped,
                    item.title
                ));
            }
        }
    }
    out.push_str("\n## Skill Drafts\n\n");
    for draft in &brain.skills.drafts {
        out.push_str(&format!(
            "- `{}` `{}` validate `{}` audit `{}` bench `{}`: {}\n",
            draft.id,
            draft.status,
            draft.validate.as_deref().unwrap_or("unknown"),
            draft.audit.as_deref().unwrap_or("unknown"),
            draft.bench.as_deref().unwrap_or("pending"),
            draft.next_action
        ));
    }
    out.push_str("\n## Runtime Guard Proposals\n\n");
    for guard in &brain.policies.guard_proposals {
        out.push_str(&format!(
            "- `{}` `{}` compile `{}`: {}\n",
            guard.memory_id,
            guard.status,
            guard.can_compile_deterministic_guard,
            guard.suggested_runtime_rule
        ));
    }
    out.push_str("\n## Open Findings\n\n");
    if let Some(open) = brain.findings.by_status.get("open") {
        for finding in open.iter().take(20) {
            out.push_str(&format!(
                "- `{}` `{}` score `{}` recurrences `{}`: {}\n",
                finding.id,
                finding.category,
                finding.impact_score,
                finding.recurrence_count,
                finding.summary
            ));
        }
    }
    out.push_str("\n## Dream Experiment Candidates\n\n");
    for candidate in brain.candidates.items.iter().take(20) {
        out.push_str(&format!(
            "- `{}` finding `{}` status `{}` decision `{}` eval `{}`\n",
            candidate.id,
            candidate.finding,
            candidate.status,
            candidate.decision.as_deref().unwrap_or("unknown"),
            candidate.eval.as_deref().unwrap_or("not written")
        ));
    }
    out
}

fn memory_next_action(card: &MemoryCard, stats: MemoryUsageStats) -> String {
    match card.status.as_str() {
        "candidate" if stats.helped_confirmed > 0 => {
            format!("air memory promote {} --status validated", card.id)
        }
        "candidate" => format!("air memory view {} --evidence", card.id),
        "validated" if stats.causal_helped == 0 => format!(
            "air memory causal-eval {} --outcome helped --evidence <compare-no-memory.json>",
            card.id
        ),
        "validated" | "promoted" if card.promotion.can_compile_skill => {
            format!("air memory skill-evaluate {}", card.id)
        }
        "promoted" | "pinned" => "included in safe memory packs".to_string(),
        "retired" => "retired; no runtime use".to_string(),
        _ => format!("air memory view {} --evidence", card.id),
    }
}

fn finding_next_action(id: &str, status: &str) -> String {
    match status {
        "open" => format!("air improve promote {id}"),
        "resolved" => "watch for recurrence in future Dream runs".to_string(),
        "dismissed" => format!("air dream findings open {id} --reason <reason>"),
        _ => "review finding record".to_string(),
    }
}

fn skill_draft_next_action(status: &str) -> String {
    match status {
        "validated_skill" => "run benchmark before import/trust".to_string(),
        "bench_passed" => "review and import as a trusted skill if appropriate".to_string(),
        "skill_needs_review" => "fix or reject draft before benchmarking".to_string(),
        _ => "run air memory skill-evaluate <memory-id>".to_string(),
    }
}

fn brain_next_commands() -> Vec<String> {
    vec![
        "air brain memory".to_string(),
        "air brain skills".to_string(),
        "air brain policies".to_string(),
        "air brain findings".to_string(),
        "air brain candidates".to_string(),
        "air brain report".to_string(),
    ]
}

fn collect_named_files(root: &Path, name: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_named_files_inner(root, name, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_named_files_inner(root: &Path, name: &str, files: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if root.file_name().and_then(|value| value.to_str()) == Some(name) {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry under {}", root.display()))?;
        collect_named_files_inner(&entry.path(), name, files)?;
    }
    Ok(())
}

fn collect_json_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_json_files_inner(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_json_files_inner(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if root.extension().and_then(|value| value.to_str()) == Some("json") {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry under {}", root.display()))?;
        collect_json_files_inner(&entry.path(), files)?;
    }
    Ok(())
}

fn latest_jsonl_records(path: &Path) -> Result<Vec<Value>> {
    let mut latest = BTreeMap::<String, Value>::new();
    for value in read_jsonl_values(path)? {
        let key = value
            .get("stable_key")
            .and_then(Value::as_str)
            .or_else(|| value.get("id").and_then(Value::as_str))
            .unwrap_or("unknown")
            .to_string();
        latest.insert(key, value);
    }
    Ok(latest.into_values().collect())
}

fn read_yaml_value(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn compact_counts(counts: &BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return "none".to_string();
    }
    counts
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn brain_status_rank(status: &str) -> u8 {
    match status {
        "pinned" => 5,
        "promoted" => 4,
        "validated" => 3,
        "candidate" => 2,
        "observed" => 1,
        "retired" => 0,
        _ => 1,
    }
}

fn brain_skill_status_rank(status: &str) -> u8 {
    match status {
        "bench_passed" => 4,
        "validated_skill" => 3,
        "draft" => 2,
        "skill_needs_review" => 1,
        _ => 0,
    }
}

fn brain_finding_status_rank(status: &str) -> u8 {
    match status {
        "open" => 3,
        "resolved" => 2,
        "dismissed" => 1,
        _ => 0,
    }
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
    compact_usage_log_if_needed(memory_dir, now)?;
    Ok(written)
}

fn reconcile_memory_usage(memory_dir: &Path, now: u64) -> Result<usize> {
    let usage_path = memory_dir.join("usage.jsonl");
    let stats = memory_usage_stats_for_dir(memory_dir)?;
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
            events += 2;
        }
    }

    let stats = memory_usage_stats_for_dir(memory_dir)?;
    for (mut card, path) in load_cards(memory_dir)? {
        let Some(item) = stats.get(&card.id).copied() else {
            continue;
        };
        if card.status != "retired" && should_retire_from_advance(item) {
            card.status = "retired".to_string();
            card.updated_at = now;
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
    compact_usage_log_if_needed(memory_dir, now)?;
    Ok(events)
}

fn has_causal_promotion_evidence(memory_dir: &Path, memory_id: &str) -> Result<bool> {
    Ok(memory_usage_stats_for_dir(memory_dir)?
        .get(memory_id)
        .is_some_and(|stats| stats.causal_helped > 0))
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
            "causal_helped" => entry.causal_helped += 1,
            "causal_hurt" => entry.causal_hurt += 1,
            _ => entry.neutral += 1,
        }
    }
    stats
}

fn merge_usage_stats(
    target: &mut BTreeMap<String, MemoryUsageStats>,
    source: BTreeMap<String, MemoryUsageStats>,
) {
    for (memory_id, item) in source {
        let entry = target.entry(memory_id).or_default();
        entry.used += item.used;
        entry.helped_candidate += item.helped_candidate;
        entry.hurt_candidate += item.hurt_candidate;
        entry.helped_confirmed += item.helped_confirmed;
        entry.hurt_confirmed += item.hurt_confirmed;
        entry.causal_helped += item.causal_helped;
        entry.causal_hurt += item.causal_hurt;
        entry.neutral += item.neutral;
    }
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
    write_json_atomic(path, &serde_json::to_vec_pretty(card)?)
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
    append_jsonl_locked(path, value)
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

fn render_skill_manifest_draft(card: &MemoryCard, slug: &str) -> String {
    let triggers = if card.triggers.is_empty() {
        "    - procedure memory\n".to_string()
    } else {
        card.triggers
            .iter()
            .map(|trigger| format!("    - {}\n", yaml_scalar(trigger)))
            .collect::<String>()
    };
    format!(
        r#"schema: air.skill.v1
id: {slug}
mode: instruction
version: 0.1.0
description: {description}
source:
  type: dream_memory
  memory_id: {memory_id}
instructions:
  files:
    - SKILL.md
capabilities: {{}}
host:
  skill: code-agent
routing:
  summary: {summary}
  triggers:
{triggers}  task_types:
    - code
verification:
  mode: instruction_only
  scripts: disabled
"#,
        slug = slug,
        description = yaml_scalar(&format!("Draft skill from AIR memory: {}", card.title)),
        memory_id = yaml_scalar(&card.id),
        summary = yaml_scalar(&format!(
            "Use this untrusted draft when the task matches procedure memory: {}",
            card.title
        )),
        triggers = triggers
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

fn memory_usage_stats_for_dir(memory_dir: &Path) -> Result<BTreeMap<String, MemoryUsageStats>> {
    let mut stats = read_usage_scorecard(memory_dir)?;
    merge_usage_stats(
        &mut stats,
        memory_usage_stats(&read_usage_events(&memory_dir.join("usage.jsonl"))?),
    );
    Ok(stats)
}

fn read_usage_scorecard(memory_dir: &Path) -> Result<BTreeMap<String, MemoryUsageStats>> {
    let path = memory_dir.join("scorecard.json");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let file: MemoryUsageScorecardFile = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    Ok(file.stats)
}

fn persist_usage_scorecard(
    memory_dir: &Path,
    stats: &BTreeMap<String, MemoryUsageStats>,
    now: u64,
) -> Result<()> {
    write_json_atomic(
        &memory_dir.join("scorecard.json"),
        &serde_json::to_vec_pretty(&MemoryUsageScorecardFile {
            schema: MEMORY_SCORECARD_SCHEMA.to_string(),
            compacted_at_unix: now,
            stats: stats.clone(),
        })?,
    )
}

fn compact_usage_log_if_needed(memory_dir: &Path, now: u64) -> Result<()> {
    let usage_path = memory_dir.join("usage.jsonl");
    if !usage_path.exists()
        || fs::metadata(&usage_path)
            .map(|metadata| metadata.len() < MEMORY_USAGE_COMPACT_BYTES)
            .unwrap_or(true)
    {
        return Ok(());
    }
    with_jsonl_lock(&usage_path, || {
        let stats = memory_usage_stats_for_dir(memory_dir)?;
        persist_usage_scorecard(memory_dir, &stats, now)?;
        write_json_atomic(&usage_path, b"")
    })
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
        memory_dir.unwrap_or_else(|| AirLayout::new(cwd.to_path_buf()).memory_dir()),
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
    fn synthesis_compares_success_and_failure_episodes() {
        let episodes = vec![
            MemoryEpisode {
                schema: EPISODE_SCHEMA,
                id: "ep-success".to_string(),
                source_kind: "code_run_artifact".to_string(),
                source_path: "success/artifact.json".to_string(),
                artifact_path: Some("success/artifact.json".to_string()),
                task: Some("fix parser".to_string()),
                category: "rust_debugging".to_string(),
                verdict: "success".to_string(),
                changed_files: vec!["src/lib.rs".to_string()],
                skills: Vec::new(),
                evidence: Vec::new(),
                memory_ids: Vec::new(),
                summary: "success".to_string(),
                created_at: 1,
            },
            MemoryEpisode {
                schema: EPISODE_SCHEMA,
                id: "ep-failure".to_string(),
                source_kind: "code_run_artifact".to_string(),
                source_path: "failure/artifact.json".to_string(),
                artifact_path: Some("failure/artifact.json".to_string()),
                task: Some("fix parser".to_string()),
                category: "rust_debugging".to_string(),
                verdict: "failure".to_string(),
                changed_files: Vec::new(),
                skills: Vec::new(),
                evidence: Vec::new(),
                memory_ids: Vec::new(),
                summary: "failure".to_string(),
                created_at: 1,
            },
        ];
        let ir = synthesize_dream_ir(&[], &[], &episodes, 1_770_000_000, "deep");

        assert!(ir
            .candidates
            .iter()
            .any(|candidate| candidate.name == "rust_debugging_success_failure_delta"));
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
        for index in 0..10 {
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
    fn promote_requires_causal_evidence_for_prompt_memory() {
        let root = temp_root("causal-promote");
        let now = 1_770_000_000;
        write_card(
            &root,
            MemoryCard {
                schema: MEMORY_SCHEMA.to_string(),
                id: "mem_causal".to_string(),
                kind: "procedure".to_string(),
                scope: MemoryScope {
                    level: "repo".to_string(),
                    repo: Some("repo".to_string()),
                    executor: Some("code-agent".to_string()),
                    skill: None,
                },
                title: "Causal memory".to_string(),
                content: "Use this only after causal eval.".to_string(),
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
                status: "validated".to_string(),
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

        assert!(!has_causal_promotion_evidence(&root, "mem_causal").unwrap());
        append_jsonl(
            &root.join("usage.jsonl"),
            &MemoryUsageEvent {
                schema: "air.memory_usage.v1".to_string(),
                memory_id: "mem_causal".to_string(),
                task: "compare-no-memory".to_string(),
                outcome: "causal_helped".to_string(),
                run_artifact: Some("compare.json".to_string()),
                at_unix: now,
            },
        )
        .unwrap();
        assert!(has_causal_promotion_evidence(&root, "mem_causal").unwrap());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn advance_promotes_causal_memory_and_writes_guard_review() {
        let root = temp_root("advance");
        let now = 1_770_000_000;
        let scope = MemoryScope {
            level: "repo".to_string(),
            repo: Some("repo".to_string()),
            executor: Some("code-agent".to_string()),
            skill: None,
        };
        write_card(
            &root,
            MemoryCard {
                schema: MEMORY_SCHEMA.to_string(),
                id: "mem_routing_causal".to_string(),
                kind: "routing".to_string(),
                scope: scope.clone(),
                title: "Causal routing memory".to_string(),
                content: "Use verification memory for verification tasks.".to_string(),
                triggers: vec!["verification".to_string()],
                evidence: vec![MemoryEvidence {
                    kind: "compare_no_memory".to_string(),
                    path: "compare.json".to_string(),
                    verdict: Some("helped".to_string()),
                    fingerprint: None,
                    note: None,
                }],
                confidence: 0.8,
                impact: 0.8,
                stability: "medium".to_string(),
                status: "validated".to_string(),
                created_at: now,
                updated_at: now,
                conflicts: Vec::new(),
                promotion: MemoryPromotion {
                    can_prompt_inject: true,
                    can_route: true,
                    can_compile_skill: false,
                    can_regression: false,
                    can_policy: false,
                    requires_human_review: true,
                },
            },
            now,
        )
        .unwrap();
        write_card(
            &root,
            MemoryCard {
                schema: MEMORY_SCHEMA.to_string(),
                id: "mem_policy_guard".to_string(),
                kind: "policy".to_string(),
                scope,
                title: "Policy candidate: require trace verification".to_string(),
                content: "Require trace verification before success.".to_string(),
                triggers: vec!["verification".to_string()],
                evidence: vec![MemoryEvidence {
                    kind: "code_run_artifact".to_string(),
                    path: "artifact.json".to_string(),
                    verdict: Some("failure".to_string()),
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
                    can_route: false,
                    can_compile_skill: false,
                    can_regression: false,
                    can_policy: true,
                    requires_human_review: true,
                },
            },
            now,
        )
        .unwrap();
        append_jsonl(
            &root.join("usage.jsonl"),
            &MemoryUsageEvent {
                schema: "air.memory_usage.v1".to_string(),
                memory_id: "mem_routing_causal".to_string(),
                task: "compare-no-memory".to_string(),
                outcome: "causal_helped".to_string(),
                run_artifact: Some("compare.json".to_string()),
                at_unix: now,
            },
        )
        .unwrap();

        let output = advance_memory(MemoryAdvanceOptions {
            memory_dir: Some(root.clone()),
            out_dir: Some(root.join("advance")),
            skill_bench: false,
            limit: None,
            timeout_seconds: None,
        })
        .unwrap();

        assert_eq!(output.promoted_memories, vec!["mem_routing_causal"]);
        assert_eq!(output.reviewed_guards.len(), 1);
        assert!(root.join("advance/guards/mem_policy_guard.json").exists());
        let (card, _) = find_card(&root, "mem_routing_causal").unwrap();
        assert_eq!(card.status, "promoted");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn advance_does_not_retire_on_single_hurt() {
        let root = temp_root("advance-single-hurt");
        let now = 1_770_000_000;
        write_card(
            &root,
            MemoryCard {
                schema: MEMORY_SCHEMA.to_string(),
                id: "mem_single_hurt".to_string(),
                kind: "procedure".to_string(),
                scope: MemoryScope {
                    level: "repo".to_string(),
                    repo: Some("repo".to_string()),
                    executor: Some("code-agent".to_string()),
                    skill: None,
                },
                title: "Mostly helpful memory".to_string(),
                content: "Use verification.".to_string(),
                triggers: vec!["verification".to_string()],
                evidence: vec![MemoryEvidence {
                    kind: "code_run_artifact".to_string(),
                    path: "artifact.json".to_string(),
                    verdict: Some("success".to_string()),
                    fingerprint: None,
                    note: None,
                }],
                confidence: 0.8,
                impact: 0.8,
                stability: "medium".to_string(),
                status: "promoted".to_string(),
                created_at: now,
                updated_at: now,
                conflicts: Vec::new(),
                promotion: MemoryPromotion {
                    can_prompt_inject: true,
                    can_route: true,
                    can_compile_skill: false,
                    can_regression: false,
                    can_policy: false,
                    requires_human_review: false,
                },
            },
            now,
        )
        .unwrap();
        for index in 0..9 {
            append_jsonl(
                &root.join("usage.jsonl"),
                &MemoryUsageEvent {
                    schema: "air.memory_usage.v1".to_string(),
                    memory_id: "mem_single_hurt".to_string(),
                    task: "task".to_string(),
                    outcome: "helped_candidate".to_string(),
                    run_artifact: Some(format!("help-{index}.json")),
                    at_unix: now,
                },
            )
            .unwrap();
        }
        append_jsonl(
            &root.join("usage.jsonl"),
            &MemoryUsageEvent {
                schema: "air.memory_usage.v1".to_string(),
                memory_id: "mem_single_hurt".to_string(),
                task: "task".to_string(),
                outcome: "hurt_candidate".to_string(),
                run_artifact: Some("hurt.json".to_string()),
                at_unix: now,
            },
        )
        .unwrap();

        let output = advance_memory(MemoryAdvanceOptions {
            memory_dir: Some(root.clone()),
            out_dir: Some(root.join("advance")),
            skill_bench: false,
            limit: None,
            timeout_seconds: None,
        })
        .unwrap();

        assert!(output.retired_memories.is_empty());
        let (card, _) = find_card(&root, "mem_single_hurt").unwrap();
        assert_eq!(card.status, "promoted");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn advance_retire_balances_confirmed_and_causal_evidence() {
        let mut stats = MemoryUsageStats {
            hurt_confirmed: 2,
            causal_helped: 5,
            ..Default::default()
        };
        assert!(!should_retire_from_advance(stats));

        stats.causal_hurt = 4;
        assert!(should_retire_from_advance(stats));
    }

    #[test]
    fn advance_limit_ignores_unmeasured_routing_cards() {
        let root = temp_root("advance-limit-unmeasured-routing");
        let now = 1_770_000_000;
        let scope = MemoryScope {
            level: "repo".to_string(),
            repo: Some("repo".to_string()),
            executor: Some("code-agent".to_string()),
            skill: None,
        };
        for index in 0..5 {
            write_card(
                &root,
                MemoryCard {
                    schema: MEMORY_SCHEMA.to_string(),
                    id: format!("mem_unmeasured_route_{index}"),
                    kind: "routing".to_string(),
                    scope: scope.clone(),
                    title: format!("Unmeasured routing {index}"),
                    content: "Candidate routing hint without usage.".to_string(),
                    triggers: vec!["routing".to_string()],
                    evidence: vec![MemoryEvidence {
                        kind: "dream_ir".to_string(),
                        path: "dream_ir.json".to_string(),
                        verdict: None,
                        fingerprint: None,
                        note: None,
                    }],
                    confidence: 0.4,
                    impact: 0.4,
                    stability: "medium".to_string(),
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
                },
                now,
            )
            .unwrap();
        }
        write_card(
            &root,
            MemoryCard {
                schema: MEMORY_SCHEMA.to_string(),
                id: "mem_measured_route".to_string(),
                kind: "routing".to_string(),
                scope,
                title: "Measured routing".to_string(),
                content: "Candidate routing hint with usage.".to_string(),
                triggers: vec!["routing".to_string()],
                evidence: vec![MemoryEvidence {
                    kind: "code_run_artifact".to_string(),
                    path: "artifact.json".to_string(),
                    verdict: Some("success".to_string()),
                    fingerprint: None,
                    note: None,
                }],
                confidence: 0.3,
                impact: 0.1,
                stability: "medium".to_string(),
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
            },
            now,
        )
        .unwrap();
        append_jsonl(
            &root.join("usage.jsonl"),
            &MemoryUsageEvent {
                schema: "air.memory_usage.v1".to_string(),
                memory_id: "mem_measured_route".to_string(),
                task: "task".to_string(),
                outcome: "used".to_string(),
                run_artifact: Some("artifact.json".to_string()),
                at_unix: now,
            },
        )
        .unwrap();

        let output = advance_memory(MemoryAdvanceOptions {
            memory_dir: Some(root.clone()),
            out_dir: Some(root.join("advance")),
            skill_bench: false,
            limit: Some(1),
            timeout_seconds: None,
        })
        .unwrap();

        assert_eq!(output.routing_measurements.len(), 1);
        assert_eq!(
            output.routing_measurements[0].memory_id,
            "mem_measured_route"
        );
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
            .any(|cmd| cmd == "air run 'fix rust verification failure'"));
        assert!(hint
            .next_commands
            .iter()
            .any(|cmd| cmd == "air memory promote mem_candidate_rust"));
        let _ = fs::remove_dir_all(root);
    }
}
