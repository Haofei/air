use crate::project::{
    default_project_file, project_plan, project_run, ProjectPlanOptions, ProjectRunOptions,
};
use crate::review_agent::{explain_review_agent, run_review_agent, ReviewOptions};
use crate::skill::{
    prepare_skill_composition, route_skills_value_for_task, run_skill_auto, SkillAutoRunOptions,
};
use anyhow::{bail, Result};
use clap::ValueEnum;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum EntryMode {
    Auto,
    Code,
    Project,
    Review,
}

#[derive(Debug)]
pub(crate) struct EntryTaskOptions {
    pub(crate) task: String,
    pub(crate) mode: EntryMode,
    pub(crate) top_k: usize,
    pub(crate) skills: Vec<String>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
    pub(crate) explain: bool,
    pub(crate) tool_config: Option<PathBuf>,
    pub(crate) artifact_out: Option<PathBuf>,
    pub(crate) replay_artifact: Option<PathBuf>,
    pub(crate) replay_from: Option<usize>,
    pub(crate) project_file: Option<PathBuf>,
    pub(crate) plan_only: bool,
    pub(crate) execute: bool,
    pub(crate) planner_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryExecutor {
    Code,
    Project,
    Review,
}

pub(crate) fn run_entry_task(options: EntryTaskOptions) -> Result<()> {
    if options.task.trim().is_empty() {
        bail!("air run task must not be empty");
    }
    let executor = select_entry_executor(&options.task, options.mode);
    if options.explain {
        return explain_entry_task(&options, executor);
    }
    match executor {
        EntryExecutor::Code => run_skill_auto(SkillAutoRunOptions {
            task: options.task,
            top_k: options.top_k,
            skills: options.skills,
            executor_override: Some("code-agent".to_string()),
            profile_override: None,
            model_config: options.model_config,
            trace_out: options.trace_out,
            trace_redact: options.trace_redact,
            trace_raw: options.trace_raw,
            log: options.log,
            tool_config_override: options.tool_config,
            artifact_out: options.artifact_out,
            replay_artifact: options.replay_artifact,
            replay_from: options.replay_from,
        }),
        EntryExecutor::Project => {
            if options.trace_out.is_some()
                || options.tool_config.is_some()
                || options.artifact_out.is_some()
                || options.replay_artifact.is_some()
                || options.replay_from.is_some()
            {
                bail!(
                    "--trace-out/--tool-config/--artifact-out/--replay-artifact are code-agent options; use --mode code or run project subcommands directly"
                );
            }
            let project_file = options
                .project_file
                .unwrap_or_else(|| PathBuf::from(".air/run").join(default_project_file()));
            eprintln!(
                "[air run] selected project-agent; writing project manifest to {}",
                project_file.display()
            );
            project_plan(ProjectPlanOptions {
                goal: options.task,
                output: Some(project_file.clone()),
                model_config: options.model_config,
                planner_model: options.planner_model,
                template: false,
            })?;
            if options.plan_only || !options.execute {
                eprintln!(
                    "[air run] project plan written; review it, then run `air project run --file {} --log` or pass --execute",
                    project_file.display()
                );
                return Ok(());
            }
            project_run(ProjectRunOptions {
                file: project_file,
                task: None,
                log: options.log,
            })
        }
        EntryExecutor::Review => {
            let outputs = run_review_agent(ReviewOptions {
                task: options.task,
                top_k: options.top_k,
                skills: options.skills,
                model_config: options.model_config,
                trace_out: options.trace_out,
                trace_redact: options.trace_redact,
                trace_raw: options.trace_raw,
                log: options.log,
                tool_config: options.tool_config,
                artifact_out: options.artifact_out,
                replay_artifact: options.replay_artifact,
                replay_from: options.replay_from,
            })?;
            println!("{}", serde_json::to_string_pretty(&outputs)?);
            Ok(())
        }
    }
}

fn explain_entry_task(options: &EntryTaskOptions, executor: EntryExecutor) -> Result<()> {
    let explanation = match executor {
        EntryExecutor::Code => explain_code_entry(options)?,
        EntryExecutor::Project => {
            let project_file = options
                .project_file
                .clone()
                .unwrap_or_else(|| PathBuf::from(".air/run").join(default_project_file()));
            json!({
                "task": options.task,
                "executor": "project-agent",
                "mode": "project",
                "project_file": project_file,
                "will_execute": options.execute,
                "plan_only": options.plan_only || !options.execute,
                "planner_model": options.planner_model,
                "model_config": options.model_config,
                "permissions": {
                    "filesystem": "project tasks run through isolated code-agent worktrees before applying patches",
                    "verification": "per-task verification commands and diff constraints come from the generated project manifest",
                },
                "artifacts": {
                    "project_state": ".air/project/state.json",
                    "task_artifacts": ".air/project/tasks/<task-id>/artifact",
                }
            })
        }
        EntryExecutor::Review => explain_review_agent(
            &options.task,
            options.top_k,
            &options.skills,
            &options.model_config,
            &options.trace_out,
            &options.artifact_out,
        )?,
    };
    println!("{}", serde_json::to_string_pretty(&explanation)?);
    Ok(())
}

fn explain_code_entry(options: &EntryTaskOptions) -> Result<Value> {
    let route = route_skills_value_for_task(&options.task, options.top_k.max(1))?;
    let routed = route
        .get("instructions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|card| card.get("host").and_then(Value::as_str) == Some("code-agent"))
        .filter_map(|card| card.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let selected = merge_skill_ids(&routed, &options.skills);
    let prepared = prepare_skill_composition(&selected)?;
    let composition = prepared
        .artifact_extra
        .get("skill_composition")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "task": options.task,
        "executor": "code-agent",
        "mode": "code",
        "route": route,
        "forced_skills": options.skills,
        "selected_instruction_skills": selected,
        "profile": prepared.profile,
        "tool_config": prepared.tool_config,
        "composition": composition,
        "model_config": options.model_config,
        "trace_out": options.trace_out,
        "artifact_out": options.artifact_out,
        "permissions": {
            "file.read": "allowed through bounded read/search tools",
            "file.write": "allowed under the configured code-agent tools",
            "shell.unrestricted": "denied by code-agent skill capability policy",
            "network": "only configured tools such as webfetch or mcp can access network",
        },
        "verification": {
            "requires_patch": true,
            "requires_verification_after_write": true,
            "success_source": "runtime-derived artifact verdict, not model-claimed success",
        }
    }))
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

fn select_entry_executor(task: &str, mode: EntryMode) -> EntryExecutor {
    match mode {
        EntryMode::Code => EntryExecutor::Code,
        EntryMode::Project => EntryExecutor::Project,
        EntryMode::Review => EntryExecutor::Review,
        EntryMode::Auto => {
            if task_looks_like_review(task) {
                EntryExecutor::Review
            } else if task_looks_project_sized(task) {
                EntryExecutor::Project
            } else {
                EntryExecutor::Code
            }
        }
    }
}

fn task_looks_like_review(task: &str) -> bool {
    let task = task.to_ascii_lowercase();
    let review_markers = [
        "code review",
        "review code",
        "review diff",
        "review this diff",
        "review this pr",
        "review this mr",
        "pull request review",
        "merge request review",
        "mr review",
        "pr review",
        "review merge request",
        "review pull request",
        "/merge_requests/",
        "/pull/",
        "审核",
        "审查",
        "代码评审",
        "评审",
    ];
    review_markers.iter().any(|marker| task.contains(marker))
}

fn task_looks_project_sized(task: &str) -> bool {
    let task = task.to_ascii_lowercase();
    let project_markers = [
        "project",
        "end to end",
        "end-to-end",
        "multi-file",
        "multiple files",
        "across crates",
        "across modules",
        "whole crate",
        "entire crate",
        "whole project",
        "large refactor",
        "split crate",
        "split module",
        "refactor crate",
        "refactor project",
        "build an app",
        "build a feature",
        "implement feature",
        "复杂",
        "整个",
        "跨文件",
        "多文件",
        "大项目",
        "拆分项目",
        "重构项目",
        "重构整个",
    ];
    project_markers.iter().any(|marker| task.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::{select_entry_executor, EntryExecutor, EntryMode};

    #[test]
    fn auto_routes_large_tasks_to_project_executor() {
        assert_eq!(
            select_entry_executor(
                "refactor the whole crate into smaller modules",
                EntryMode::Auto
            ),
            EntryExecutor::Project
        );
        assert_eq!(
            select_entry_executor("跨文件重构 tools crate", EntryMode::Auto),
            EntryExecutor::Project
        );
    }

    #[test]
    fn auto_routes_small_tasks_to_code_executor() {
        assert_eq!(
            select_entry_executor("rename this helper and run tests", EntryMode::Auto),
            EntryExecutor::Code
        );
    }

    #[test]
    fn auto_routes_review_tasks_to_review_executor() {
        assert_eq!(
            select_entry_executor("review this MR for regressions", EntryMode::Auto),
            EntryExecutor::Review
        );
        assert_eq!(
            select_entry_executor(
                "review https://gitlab.example.com/a/b/-/merge_requests/123",
                EntryMode::Auto
            ),
            EntryExecutor::Review
        );
    }

    #[test]
    fn explicit_mode_overrides_auto_routing() {
        assert_eq!(
            select_entry_executor("refactor the whole crate", EntryMode::Code),
            EntryExecutor::Code
        );
        assert_eq!(
            select_entry_executor("small rename", EntryMode::Project),
            EntryExecutor::Project
        );
        assert_eq!(
            select_entry_executor("small rename", EntryMode::Review),
            EntryExecutor::Review
        );
    }
}
