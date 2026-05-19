//! Centralized registry of AIR JSON schema identifiers.
//!
//! Every JSON document AIR writes carries a `schema` field naming the document
//! shape (e.g. `air.memory_card.v1`). Producers and consumers should reference
//! these names through the constants in this crate rather than re-typing the
//! string literal, so that:
//!
//! 1. The set of valid schema names is discoverable in one place.
//! 2. Adding a new schema is a single edit (here) instead of a grep-and-pray.
//! 3. CI can call [`is_known`] to flag accidental renames or typos.
//!
//! New schemas should be added here first, then referenced from producer code.

#![deny(missing_docs)]

/// Schema namespace prefix used by every AIR-produced JSON document.
pub const NAMESPACE_PREFIX: &str = "air.";

/// `air run` envelope wrapping every executor output.
pub const RUN: &str = "air.run.v1";

/// `air status` snapshot.
pub const STATUS: &str = "air.status.v1";

// --- Audit ---------------------------------------------------------------

/// Single-run audit report produced by `air audit run`.
pub const AUDIT: &str = "air.audit.v1";
/// Collection report produced by `air audit collect`.
pub const AUDIT_COLLECTION: &str = "air.audit_collection.v1";
/// Request payload sent to the `audit_diagnoser` LLM advisory model.
pub const AUDIT_DIAGNOSIS_REQUEST: &str = "air.audit_diagnosis_request.v1";

// --- Code run artifact ---------------------------------------------------

/// Code-agent run artifact written under `target/generated/code-runs/<run>/`.
pub const CODE_RUN_ARTIFACT: &str = "air.code_run_artifact.v1";
/// Fingerprint payload used to dedupe code-run artifacts.
pub const CODE_RUN_FINGERPRINT: &str = "air.code_run_fingerprint.v1";

// --- Dream ----------------------------------------------------------------

/// Top-level Dream run output.
pub const DREAM: &str = "air.dream.v1";
/// Persistent Dream cursor state stored at `.air/dream/state.json`.
pub const DREAM_STATE: &str = "air.dream_state.v1";
/// Per-run Dream window manifest describing inputs included in the window.
pub const DREAM_WINDOW: &str = "air.dream_window.v1";
/// Persistent Dream finding record appended to `.air/dream/findings.jsonl`.
pub const DREAM_FINDING_RECORD: &str = "air.dream_finding_record.v1";
/// Dream finding listing output.
pub const DREAM_FINDINGS: &str = "air.dream_findings.v1";
/// Single-finding update output (resolve/dismiss/open).
pub const DREAM_FINDING_UPDATE: &str = "air.dream_finding_update.v1";
/// Append-only Dream lifecycle ledger event.
pub const DREAM_LEDGER_EVENT: &str = "air.dream_ledger_event.v1";
/// Deterministic synthesis IR produced from a Dream window.
pub const DREAM_IR: &str = "air.dream_ir.v1";
/// LLM advisory dream synthesis error sidecar.
pub const LLM_DREAM_SYNTHESIS_ERROR: &str = "air.llm_dream_synthesis_error.v1";
/// Provenance written alongside Dream experiment candidates.
pub const DREAM_CANDIDATE_PROVENANCE: &str = "air.dream_candidate_provenance.v1";

// --- Memory ---------------------------------------------------------------

/// Long-term memory card (procedure, concept, hypothesis, policy, failure, routing).
pub const MEMORY_CARD: &str = "air.memory_card.v1";
/// Memory episode summary derived from a code-run artifact or trace.
pub const MEMORY_EPISODE: &str = "air.memory_episode.v1";
/// Memory ledger event recording lifecycle transitions.
pub const MEMORY_LEDGER_EVENT: &str = "air.memory_ledger_event.v1";
/// Memory extraction output (Dream → memory mining).
pub const MEMORY_EXTRACT: &str = "air.memory_extract.v1";
/// Memory advance pass output.
pub const MEMORY_ADVANCE: &str = "air.memory_advance.v1";
/// Memory pack injected into `air run` prompts.
pub const MEMORY_PACK: &str = "air.memory_pack.v1";
/// Frozen renderer version embedded in pack hashes.
pub const MEMORY_CONTEXT_RENDERER: &str = "air.memory_context_renderer.v1";
/// External memory provider hook event.
pub const MEMORY_PROVIDER_EVENT: &str = "air.memory_provider_event.v1";
/// Memory scorecard summary.
pub const MEMORY_SCORECARD: &str = "air.memory_scorecard.v1";
/// Persistent memory scorecard state under `.air/memory/scorecard.json`.
pub const MEMORY_SCORECARD_STATE: &str = "air.memory_scorecard_state.v1";
/// Memory usage event (helped/hurt/used) appended to `usage.jsonl`.
pub const MEMORY_USAGE: &str = "air.memory_usage.v1";
/// Memory list output.
pub const MEMORY_LIST: &str = "air.memory_list.v1";
/// Memory search output.
pub const MEMORY_SEARCH: &str = "air.memory_search.v1";
/// Memory card update output.
pub const MEMORY_UPDATE: &str = "air.memory_update.v1";
/// Memory graph dump output.
pub const MEMORY_GRAPH: &str = "air.memory_graph.v1";
/// Memory causal-eval output.
pub const MEMORY_CAUSAL_EVAL: &str = "air.memory_causal_eval.v1";
/// Memory policy candidate review output.
pub const MEMORY_POLICY_CHECK: &str = "air.memory_policy_check.v1";
/// Memory guard review proposal.
pub const MEMORY_GUARD_REVIEW: &str = "air.memory_guard_review.v1";
/// Memory skill draft generated from a procedure memory.
pub const MEMORY_SKILL_DRAFT: &str = "air.memory_skill_draft.v1";
/// Memory skill draft evaluation gate output.
pub const MEMORY_SKILL_EVALUATE: &str = "air.memory_skill_evaluate.v1";
/// Memory skill advance output (per-card lifecycle decisions).
pub const MEMORY_SKILL_ADVANCE: &str = "air.memory_skill_advance.v1";
/// Experience graph edge between memory cards.
pub const EXPERIENCE_GRAPH_EDGE: &str = "air.experience_graph_edge.v1";

// --- Brain ----------------------------------------------------------------

/// One-page brain summary of memory/skills/findings/policies/candidates.
pub const BRAIN: &str = "air.brain.v1";
/// Brain report rendered as markdown.
pub const BRAIN_REPORT: &str = "air.brain_report.v1";

// --- Improve --------------------------------------------------------------

/// Improve observations file.
pub const IMPROVE_OBSERVATIONS: &str = "air.improve_observations.v1";
/// Improve findings file.
pub const IMPROVE_FINDINGS: &str = "air.improve_findings.v1";
/// Improve suggested regression fixtures.
pub const IMPROVE_SUGGESTED_REGRESSIONS: &str = "air.improve_suggested_regressions.v1";
/// Improve failure classifier (LLM advisory) summary.
pub const IMPROVE_FAILURE_CLASSIFIER: &str = "air.improve_failure_classifier.v1";
/// Improve finding status journal.
pub const IMPROVE_FINDING_STATUS: &str = "air.improve_finding_status.v1";
/// `air improve` top-level run output.
pub const IMPROVE_RUN: &str = "air.improve_run.v1";
/// `air improve next` output.
pub const IMPROVE_NEXT: &str = "air.improve_next.v1";
/// `air improve check` output.
pub const IMPROVE_CHECK: &str = "air.improve_check.v1";
/// `air improve promote` output.
pub const IMPROVE_PROMOTE: &str = "air.improve_promote.v1";
/// `air improve evaluate` output.
pub const IMPROVE_EVALUATE: &str = "air.improve_evaluate.v1";
/// `air improve fix` output.
pub const IMPROVE_FIX: &str = "air.improve_fix.v1";

// --- Eval -----------------------------------------------------------------

/// Eval-corpus manifest pinning file hashes.
pub const EVAL_MANIFEST: &str = "air.eval_manifest.v1";
/// Eval manifest integrity check output.
pub const EVAL_MANIFEST_CHECK: &str = "air.eval_manifest_check.v1";

// --- Regression -----------------------------------------------------------

/// Regression suite run output.
pub const REGRESSION_RUN: &str = "air.regression_run.v1";

// --- Self lab -------------------------------------------------------------

/// `air self prepare` output.
pub const SELF_PREPARE: &str = "air.self_prepare.v1";
/// `air self fix` output (candidate generation).
pub const SELF_FIX: &str = "air.self_fix.v1";
/// `air self capture` output (candidate patch + diff capture).
pub const SELF_CAPTURE: &str = "air.self_capture.v1";
/// `air self compare` output.
pub const SELF_COMPARE: &str = "air.self_compare.v1";
/// Request payload sent to the `candidate_judge` LLM advisory model.
pub const CANDIDATE_JUDGE_REQUEST: &str = "air.candidate_judge_request.v1";

// --- Skill ----------------------------------------------------------------

/// Installed AIR skill manifest.
pub const SKILL: &str = "air.skill.v1";
/// Skill audit output (risk gates).
pub const SKILL_AUDIT: &str = "air.skill_audit.v1";
/// Repo-level lock of imported skill provenance and trust decisions.
pub const SKILLS_LOCK: &str = "air.skills_lock.v1";
/// Request payload sent to the `skill_router` LLM advisory model.
pub const SKILL_ROUTER_REQUEST: &str = "air.skill_router_request.v1";

// --- Project --------------------------------------------------------------

/// Project manifest.
pub const PROJECT: &str = "air.project.v1";
/// Project execution state.
pub const PROJECT_STATE: &str = "air.project_state.v1";

// --- LLM advisory cache --------------------------------------------------

/// Wire format of cached LLM advisory responses.
pub const LLM_ADVISORY_CACHE: &str = "air.llm_advisory_cache.v1";

// --- Trace specialization & JIT cache ------------------------------------

/// Trace specialization output for hot-path optimization.
pub const TRACE_SPECIALIZATION: &str = "air.trace_specialization.v1";
/// Single JIT cache entry.
pub const JIT_CACHE_ENTRY: &str = "air.jit_cache_entry.v1";
/// JIT cache key descriptor.
pub const JIT_CACHE_KEY: &str = "air.jit_cache_key.v1";

// --- Registry helpers ----------------------------------------------------

/// Every registered schema identifier known to this crate.
///
/// The order is stable but not semantically meaningful; consumers should treat
/// it as an unordered set. Adding a new schema must extend this slice so
/// [`is_known`] and [`known_schemas`] reflect it.
pub const ALL: &[&str] = &[
    RUN,
    STATUS,
    AUDIT,
    AUDIT_COLLECTION,
    AUDIT_DIAGNOSIS_REQUEST,
    CODE_RUN_ARTIFACT,
    CODE_RUN_FINGERPRINT,
    DREAM,
    DREAM_STATE,
    DREAM_WINDOW,
    DREAM_FINDING_RECORD,
    DREAM_FINDINGS,
    DREAM_FINDING_UPDATE,
    DREAM_LEDGER_EVENT,
    DREAM_IR,
    LLM_DREAM_SYNTHESIS_ERROR,
    DREAM_CANDIDATE_PROVENANCE,
    MEMORY_CARD,
    MEMORY_EPISODE,
    MEMORY_LEDGER_EVENT,
    MEMORY_EXTRACT,
    MEMORY_ADVANCE,
    MEMORY_PACK,
    MEMORY_CONTEXT_RENDERER,
    MEMORY_PROVIDER_EVENT,
    MEMORY_SCORECARD,
    MEMORY_SCORECARD_STATE,
    MEMORY_USAGE,
    MEMORY_LIST,
    MEMORY_SEARCH,
    MEMORY_UPDATE,
    MEMORY_GRAPH,
    MEMORY_CAUSAL_EVAL,
    MEMORY_POLICY_CHECK,
    MEMORY_GUARD_REVIEW,
    MEMORY_SKILL_DRAFT,
    MEMORY_SKILL_EVALUATE,
    MEMORY_SKILL_ADVANCE,
    EXPERIENCE_GRAPH_EDGE,
    BRAIN,
    BRAIN_REPORT,
    IMPROVE_OBSERVATIONS,
    IMPROVE_FINDINGS,
    IMPROVE_SUGGESTED_REGRESSIONS,
    IMPROVE_FAILURE_CLASSIFIER,
    IMPROVE_FINDING_STATUS,
    IMPROVE_RUN,
    IMPROVE_NEXT,
    IMPROVE_CHECK,
    IMPROVE_PROMOTE,
    IMPROVE_EVALUATE,
    IMPROVE_FIX,
    EVAL_MANIFEST,
    EVAL_MANIFEST_CHECK,
    REGRESSION_RUN,
    SELF_PREPARE,
    SELF_FIX,
    SELF_CAPTURE,
    SELF_COMPARE,
    CANDIDATE_JUDGE_REQUEST,
    SKILL,
    SKILL_AUDIT,
    SKILLS_LOCK,
    SKILL_ROUTER_REQUEST,
    PROJECT,
    PROJECT_STATE,
    LLM_ADVISORY_CACHE,
    TRACE_SPECIALIZATION,
    JIT_CACHE_ENTRY,
    JIT_CACHE_KEY,
];

/// Return every registered AIR schema identifier.
pub fn known_schemas() -> &'static [&'static str] {
    ALL
}

/// Check whether `name` is a registered AIR schema identifier.
pub fn is_known(name: &str) -> bool {
    ALL.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_registered_schema_starts_with_air_prefix() {
        for schema in ALL {
            assert!(
                schema.starts_with(NAMESPACE_PREFIX),
                "{schema} must start with {NAMESPACE_PREFIX}"
            );
        }
    }

    #[test]
    fn every_registered_schema_carries_a_version_suffix() {
        for schema in ALL {
            let last = schema.rsplit('.').next().unwrap();
            assert!(
                last.starts_with('v') && last[1..].chars().all(|c| c.is_ascii_digit()),
                "{schema} must end with `.vN` suffix"
            );
        }
    }

    #[test]
    fn schema_names_are_unique() {
        let set: BTreeSet<_> = ALL.iter().collect();
        assert_eq!(set.len(), ALL.len(), "duplicate schema name in ALL");
    }

    #[test]
    fn is_known_round_trips_registered_constants() {
        assert!(is_known(MEMORY_CARD));
        assert!(is_known(DREAM));
        assert!(is_known(AUDIT));
        assert!(!is_known("air.not_registered.v1"));
        assert!(!is_known("memory_card"));
    }
}
