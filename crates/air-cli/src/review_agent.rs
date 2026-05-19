use crate::code_agent::{run_code_agent, CodeOptions};
use crate::skill::{prepare_skill_composition_for_executor, route_skills_value_for_task};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

const REVIEW_EXECUTOR_SKILL: &str = "review-agent";
const CODE_REVIEW_SKILL: &str = "code-review";

pub(crate) struct ReviewOptions {
    pub(crate) task: String,
    pub(crate) top_k: usize,
    pub(crate) skills: Vec<String>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
    pub(crate) tool_config: Option<PathBuf>,
    pub(crate) artifact_out: Option<PathBuf>,
    pub(crate) replay_artifact: Option<PathBuf>,
    pub(crate) replay_from: Option<usize>,
}

pub(crate) fn run_review_agent(options: ReviewOptions) -> Result<Value> {
    let route = route_skills_value_for_task(&options.task, options.top_k.max(1))?;
    let selected = selected_review_skills(&route, &options.skills);
    let prepared = prepare_skill_composition_for_executor(REVIEW_EXECUTOR_SKILL, &selected)?;
    let task = review_task_with_skills(&options.task, prepared.task_prefix.as_deref());
    let mut artifact_extra = prepared.artifact_extra.clone();
    artifact_extra.insert(
        "entry_mode".to_string(),
        json!({
            "executor": REVIEW_EXECUTOR_SKILL,
            "kind": "review",
        }),
    );
    artifact_extra.insert(
        "skill_route".to_string(),
        json!({
            "task": options.task,
            "route": route,
            "selected_instruction_skills": selected,
        }),
    );
    run_code_agent(CodeOptions {
        task,
        verification_command: None,
        artifact_task: Some(options.task),
        skill: Some(prepared.metadata.clone()),
        profile: Some(prepared.profile.clone()),
        model_config: options.model_config,
        trace_out: options.trace_out,
        trace_redact: options.trace_redact,
        trace_raw: options.trace_raw,
        log: options.log,
        tool_config: options.tool_config.or_else(|| prepared.tool_config.clone()),
        artifact_out: options.artifact_out,
        artifact_extra,
        verdict_constraints: air_code_artifact::CodeRunVerdictConstraints::review(),
        replay_artifact: options.replay_artifact,
        replay_from: options.replay_from,
    })
}

pub(crate) fn explain_review_agent(
    task: &str,
    top_k: usize,
    forced_skills: &[String],
    model_config: &Option<PathBuf>,
    trace_out: &Option<PathBuf>,
    artifact_out: &Option<PathBuf>,
) -> Result<Value> {
    let route = route_skills_value_for_task(task, top_k.max(1))?;
    let selected = selected_review_skills(&route, forced_skills);
    let prepared = prepare_skill_composition_for_executor(REVIEW_EXECUTOR_SKILL, &selected)?;
    let composition = prepared
        .artifact_extra
        .get("skill_composition")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "task": task,
        "executor": REVIEW_EXECUTOR_SKILL,
        "mode": "review",
        "route": route,
        "forced_skills": forced_skills,
        "selected_instruction_skills": selected,
        "profile": prepared.profile,
        "tool_config": prepared.tool_config,
        "composition": composition,
        "model_config": model_config,
        "trace_out": trace_out,
        "artifact_out": artifact_out,
        "permissions": {
            "file.read": "allowed through bounded read/search tools",
            "file.write": "denied; review-agent has no workspace-writing tools",
            "remote.comment": "denied; review-agent only reads MCP/remote evidence",
            "shell.unrestricted": "denied",
            "network": "only configured read-only tools such as webfetch or gitlab MCP can access network"
        },
        "verdict": {
            "requires_patch": false,
            "requires_verification": false,
            "requires_no_workspace_writes": true,
            "success_source": "runtime-derived artifact verdict plus review output, not model-claimed edit success"
        }
    }))
}

fn selected_review_skills(route: &Value, forced: &[String]) -> Vec<String> {
    let routed = route
        .get("instructions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|card| card.get("host").and_then(Value::as_str) == Some(REVIEW_EXECUTOR_SKILL))
        .filter_map(|card| card.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut merged = merge_skill_ids(&routed, forced);
    if !merged.iter().any(|skill| skill == CODE_REVIEW_SKILL) {
        merged.insert(0, CODE_REVIEW_SKILL.to_string());
    }
    merged
}

fn merge_skill_ids(primary: &[String], additional: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    primary
        .iter()
        .map(String::as_str)
        .chain(additional.iter().flat_map(|value| value.split(',')))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(|value| {
            if seen.insert(value.to_string()) {
                Some(value.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn review_task_with_skills(task: &str, prefix: Option<&str>) -> String {
    let mode = "AIR review mode: inspect evidence and produce findings only. Do not edit files, do not run mutating commands, and do not post remote comments unless explicitly instructed by a future write-capable workflow.";
    match prefix {
        Some(prefix) => format!("{prefix}\n\n{mode}\n\nReview task: {task}"),
        None => format!("{mode}\n\nReview task: {task}"),
    }
}
