use crate::code_agent::{run_code_agent, CodeOptions};
use air_code_artifact::{CodeRunSkill, CodeRunVerdictConstraints};
use air_skill::{
    explain_skill_value, import_skill_value, list_skills_value,
    prepare_skill_composition as library_prepare_skill_composition,
    prepare_skill_composition_for_executor as library_prepare_skill_composition_for_executor,
    resolve_skill_run_metadata as library_resolve_skill_run_metadata, route_skill_value,
    upgrade_skill as library_upgrade_skill, PreparedSkillRun,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub(crate) struct SkillAutoRunOptions {
    pub(crate) task: String,
    pub(crate) top_k: usize,
    pub(crate) skills: Vec<String>,
    pub(crate) executor_override: Option<String>,
    pub(crate) profile_override: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
    pub(crate) tool_config_override: Option<PathBuf>,
    pub(crate) artifact_out: Option<PathBuf>,
    pub(crate) replay_artifact: Option<PathBuf>,
    pub(crate) replay_from: Option<usize>,
    pub(crate) verification_command: Option<String>,
    pub(crate) memory_pack: Option<Value>,
}

pub(crate) fn list_skills() -> Result<()> {
    print_json(&list_skills_value()?)
}

pub(crate) fn route_skill(options: SkillRouteOptions) -> Result<()> {
    print_json(&route_skill_value(options)?)
}

pub(crate) fn validate_skill(reference: &str) -> Result<()> {
    print_json(&validate_skill_value(reference)?)
}

pub(crate) fn explain_skill(reference: &str, profile_override: Option<PathBuf>) -> Result<()> {
    print_json(&explain_skill_value(reference, profile_override)?)
}

pub(crate) fn audit_skill(reference: &str, write: bool) -> Result<()> {
    print_json(&audit_skill_value(reference, write)?)
}

pub(crate) fn upgrade_skill(options: SkillUpgradeOptions) -> Result<()> {
    library_upgrade_skill(options)
}

pub(crate) fn import_skill(source: &str, out: Option<&std::path::Path>) -> Result<()> {
    print_json(&import_skill_value(source, out)?)
}

pub(crate) fn run_skill_auto_capture(options: SkillAutoRunOptions) -> Result<Value> {
    let route = route_skills_value_for_task(&options.task, options.top_k)?;
    let executor_id = options
        .executor_override
        .clone()
        .or_else(|| {
            route
                .get("executor")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "code-agent".to_string());
    let mut selected = route
        .get("instructions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|card| {
            card.get("host")
                .and_then(Value::as_str)
                .is_some_and(|host| host == executor_id)
        })
        .filter_map(|card| card.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    selected = merge_skill_ids(&selected, &options.skills);
    let prepared = prepare_skill_composition_for_executor(&executor_id, &selected)?;
    let task = if let Some(prefix) = &prepared.task_prefix {
        format!("{prefix}\n\nTask: {}", options.task)
    } else {
        options.task.clone()
    };
    let mut artifact_extra = prepared.artifact_extra.clone();
    if let Some(memory_pack) = options.memory_pack {
        artifact_extra.insert("memory_pack".to_string(), memory_pack);
    }
    artifact_extra.insert(
        "skill_route".to_string(),
        json!({
            "task": options.task,
            "executor": route.get("executor").cloned().unwrap_or(Value::Null),
            "effective_executor": executor_id,
            "instructions": route.get("instructions").cloned().unwrap_or_else(|| json!([])),
            "forced_instructions": options.skills,
            "selected": route.get("selected").cloned().unwrap_or_else(|| json!([])),
            "rejected": route.get("rejected").cloned().unwrap_or_else(|| json!([])),
        }),
    );
    run_code_agent(CodeOptions {
        task,
        verification_command: options.verification_command,
        artifact_task: Some(options.task.clone()),
        skill: Some(prepared.metadata.clone()),
        profile: Some(
            options
                .profile_override
                .unwrap_or_else(|| prepared.profile.clone()),
        ),
        model_config: options.model_config,
        trace_out: options.trace_out,
        trace_redact: options.trace_redact,
        trace_raw: options.trace_raw,
        log: options.log,
        tool_config: options
            .tool_config_override
            .or_else(|| prepared.tool_config.clone()),
        artifact_out: options.artifact_out,
        artifact_extra,
        verdict_constraints: verdict_constraints_for_executor(&prepared.metadata.id),
        replay_artifact: options.replay_artifact,
        replay_from: options.replay_from,
    })
}

pub(crate) fn verdict_constraints_for_executor(executor_id: &str) -> CodeRunVerdictConstraints {
    if matches!(executor_id, "review-agent" | "bench-agent") {
        CodeRunVerdictConstraints::review()
    } else {
        CodeRunVerdictConstraints::code_edit()
    }
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

pub(crate) fn prepare_skill_composition(
    instruction_references: &[String],
) -> Result<PreparedSkillRun> {
    library_prepare_skill_composition(instruction_references)
}

pub(crate) fn prepare_skill_composition_for_executor(
    executor_id: &str,
    instruction_references: &[String],
) -> Result<PreparedSkillRun> {
    library_prepare_skill_composition_for_executor(executor_id, instruction_references)
}

pub(crate) fn resolve_skill_run_metadata(reference: &str) -> Result<CodeRunSkill> {
    library_resolve_skill_run_metadata(reference)
}

fn print_json(value: &Value) -> Result<()> {
    serde_json::to_writer_pretty(std::io::stdout(), value)?;
    println!();
    Ok(())
}
pub(crate) use air_skill::{
    audit_skill_value, route_skills_value_for_task, validate_skill_value, SkillRouteOptions,
    SkillUpgradeOptions,
};
