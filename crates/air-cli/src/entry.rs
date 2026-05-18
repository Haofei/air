use crate::project::{
    default_project_file, project_plan, project_run, ProjectPlanOptions, ProjectRunOptions,
};
use crate::skill::{run_skill_auto, SkillAutoRunOptions};
use anyhow::{bail, Result};
use clap::ValueEnum;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum EntryMode {
    Auto,
    Code,
    Project,
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
}

pub(crate) fn run_entry_task(options: EntryTaskOptions) -> Result<()> {
    if options.task.trim().is_empty() {
        bail!("air run task must not be empty");
    }
    let executor = select_entry_executor(&options.task, options.mode);
    match executor {
        EntryExecutor::Code => run_skill_auto(SkillAutoRunOptions {
            task: options.task,
            top_k: options.top_k,
            skills: options.skills,
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
    }
}

fn select_entry_executor(task: &str, mode: EntryMode) -> EntryExecutor {
    match mode {
        EntryMode::Code => EntryExecutor::Code,
        EntryMode::Project => EntryExecutor::Project,
        EntryMode::Auto => {
            if task_looks_project_sized(task) {
                EntryExecutor::Project
            } else {
                EntryExecutor::Code
            }
        }
    }
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
    fn explicit_mode_overrides_auto_routing() {
        assert_eq!(
            select_entry_executor("refactor the whole crate", EntryMode::Code),
            EntryExecutor::Code
        );
        assert_eq!(
            select_entry_executor("small rename", EntryMode::Project),
            EntryExecutor::Project
        );
    }
}
