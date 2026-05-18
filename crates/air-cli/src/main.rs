use air_runtime::{system_return_event, Vm};
mod bench;
mod code_agent;
mod code_artifact;
mod diagnostics;
mod entry;
mod explain;
mod models;
mod planner;
mod profile;
mod project;
mod run_plan;
mod skill;
mod tools;
use crate::bench::{bench_code_agent, bench_skill, BenchCodeAgentOptions, BenchSkillOptions};
use crate::diagnostics::emit_diagnostics;
use crate::entry::{run_entry_task, EntryMode, EntryTaskOptions};
use crate::explain::{build_plan_explanation, format_plan_explanation};
use crate::models::ModelProviderChoice;
use crate::planner::{
    module_base_dir_for_modules, module_base_dir_for_store_path, plan_task, PlanOptions,
    ValidatePlanOptions,
};
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::project::{
    bench_project, default_project_file, project_plan, project_run, project_status, project_verify,
    BenchProjectOptions, ProjectPlanOptions, ProjectRunOptions, ProjectStatusOptions,
    ProjectVerifyOptions,
};
use crate::run_plan::{
    observe_event_with_trace_file, replay, resume_plan, run_plan, write_partial_trace, write_trace,
    ReplayOptions, ResumePlanOptions, RunPlanOptions,
};
use crate::skill::{
    audit_skill, explain_skill, import_skill, list_skills, route_skill, validate_skill,
    SkillRouteOptions,
};
use crate::tools::ToolProviderChoice;
use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn load_dotenv() {
    let _ = dotenvy::from_filename(".env");
    apply_model_profile_env();
}

fn apply_model_profile_env() {
    let Ok(profile) = std::env::var("AIR_MODEL_PROFILE") else {
        return;
    };
    let profile = normalize_model_profile_key(&profile);
    if profile.is_empty() {
        return;
    }
    for (target, suffix) in [
        ("OPENAI_API_KEY", "API_KEY"),
        ("OPENAI_BASE_URL", "BASE_URL"),
        ("OPENAI_MODEL", "MODEL"),
    ] {
        let source = format!("AIR_MODEL_{profile}_{suffix}");
        if let Ok(value) = std::env::var(source) {
            if !value.trim().is_empty() {
                std::env::set_var(target, value);
            }
        }
    }
}

fn normalize_model_profile_key(profile: &str) -> String {
    profile
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[derive(Debug, Parser)]
#[command(name = "air")]
#[command(about = "AIR skill runtime CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Run or inspect installed AIR skills.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Run AIR benchmark suites.
    Bench {
        #[command(subcommand)]
        command: BenchCommand,
    },
    /// Advanced AIR IR/runtime tools.
    Dev {
        #[command(subcommand)]
        command: DevCommand,
    },
    /// Run bounded multi-task coding projects.
    #[command(hide = true)]
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Run a user task through AIR's entry agent.
    Run {
        /// Natural-language task.
        target: String,

        /// Entry routing mode for natural-language tasks.
        #[arg(long, value_enum, default_value_t = EntryMode::Auto)]
        mode: EntryMode,

        /// Number of instruction skills to route into the host agent.
        #[arg(long, default_value_t = 3, hide = true)]
        top_k: usize,

        /// Additional instruction skills to preload. Accepts repeated flags or comma-separated ids.
        #[arg(long = "skills", value_delimiter = ',')]
        skills: Vec<String>,

        /// Project manifest path when --mode project is selected.
        #[arg(long, hide = true)]
        project_file: Option<PathBuf>,

        /// For project mode, stop after writing the project manifest.
        #[arg(long, hide = true, conflicts_with = "execute")]
        plan_only: bool,

        /// For project mode, run the generated project manifest immediately.
        #[arg(long)]
        execute: bool,

        /// Project planner model alias from --model-config.
        #[arg(long, default_value = "project_planner", hide = true)]
        planner_model: String,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long, hide = true)]
        trace_out: Option<PathBuf>,

        /// Redact sensitive fields and cap trace event size before writing --trace-out (default).
        #[arg(long, hide = true)]
        trace_redact: bool,

        /// Write raw trace events without redaction.
        #[arg(long, conflicts_with = "trace_redact", hide = true)]
        trace_raw: bool,

        /// Print human-readable execution logs to stderr.
        #[arg(long)]
        log: bool,

        /// Optional tool provider config JSON.
        #[arg(long, hide = true)]
        tool_config: Option<PathBuf>,

        /// Optional directory for an auditable code-run artifact.
        #[arg(long, hide = true)]
        artifact_out: Option<PathBuf>,

        /// Replay a previous code-run artifact, optionally switching to live execution with --replay-from.
        #[arg(long, hide = true)]
        replay_artifact: Option<PathBuf>,

        /// 1-based trace event line where replay should switch to the live provider.
        #[arg(long, requires = "replay_artifact", hide = true)]
        replay_from: Option<usize>,
    },
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    /// List local AIR skills.
    List,
    /// Validate an AIR skill manifest.
    Validate {
        /// Skill id, manifest path, or skill directory.
        skill: String,
    },
    /// Audit an AIR skill package without executing it.
    Audit {
        /// Skill id, manifest path, or skill directory.
        skill: String,

        /// Write audit.json next to the skill manifest.
        #[arg(long)]
        write: bool,
    },
    /// Import a local folder, git repository, or zip as AIR skill package(s).
    Import {
        /// Local directory/path, git URL, or zip URL/path.
        source: String,

        /// Destination skill directory for one skill, or destination root for a skill collection.
        /// Defaults to skills/vendor for collections and skills/vendor/<skill-id> for single skills.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Route a task to the most relevant local AIR skills.
    Route {
        /// Natural-language task.
        task: String,

        /// Number of selected skill cards to return.
        #[arg(long, default_value_t = 3)]
        top_k: usize,

        /// Include deterministic routing score details.
        #[arg(long)]
        explain: bool,
    },
    /// Explain what a skill is allowed to do without running it.
    Explain {
        /// Skill id, manifest path, or skill directory.
        skill: String,

        /// Skill run profile override.
        #[arg(long)]
        profile: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    /// Run the code-agent benchmark suite.
    Code {
        /// Benchmark suite JSON file.
        #[arg(long)]
        suite: Option<PathBuf>,

        /// Output directory for run.json, traces, and workdirs.
        #[arg(long)]
        out_dir: Option<PathBuf>,

        /// Coding-agent run profile.
        #[arg(long)]
        profile: Option<PathBuf>,

        /// OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Run only one task id.
        #[arg(long)]
        task: Option<String>,

        /// Run at most N selected tasks.
        #[arg(long)]
        limit: Option<usize>,

        /// Print AIR execution logs while the benchmark runs.
        #[arg(long)]
        log: bool,

        /// Keep successful task workdirs. Failed task workdirs are always kept.
        #[arg(long)]
        keep_workdirs: bool,

        /// Ignore cached code-run artifacts and spend model calls again.
        #[arg(long)]
        refresh: bool,

        /// Optional Markdown benchmark report output path.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Run a code-agent suite with a skill preloaded, optionally comparing no-skill.
    Skill {
        /// Skill id, manifest path, skill directory, or comma-separated instruction skill ids.
        skill: String,

        /// Additional instruction skills to preload.
        #[arg(long, value_delimiter = ',')]
        skills: Vec<String>,

        /// Benchmark suite JSON file.
        #[arg(long)]
        suite: Option<PathBuf>,

        /// Output directory for run.json, traces, and workdirs.
        #[arg(long)]
        out_dir: Option<PathBuf>,

        /// OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Run only one task id.
        #[arg(long)]
        task: Option<String>,

        /// Run at most N selected tasks.
        #[arg(long)]
        limit: Option<usize>,

        /// Compare against a no-skill baseline in the same benchmark run.
        #[arg(long)]
        compare_no_skill: bool,

        /// Print AIR execution logs while the benchmark runs.
        #[arg(long)]
        log: bool,

        /// Keep successful task workdirs. Failed task workdirs are always kept.
        #[arg(long)]
        keep_workdirs: bool,

        /// Ignore cached code-run artifacts and spend model calls again.
        #[arg(long)]
        refresh: bool,

        /// Optional Markdown benchmark report output path.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Run deterministic project-orchestrator benchmark cases.
    #[command(hide = true)]
    Project {
        /// Project benchmark suite JSON file.
        #[arg(long)]
        suite: Option<PathBuf>,

        /// Output directory for run.json.
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum DevCommand {
    /// Parse and statically verify an AIR module.
    ValidateModule {
        /// Path to a .air.yaml, .air.yml, or .air.json file.
        file: PathBuf,
    },
    /// Parse and statically verify an AIR system.
    ValidateSystem {
        /// Path to a .air-system.yaml file.
        file: PathBuf,
    },
    /// Parse and statically verify a dynamic AIR run plan against a module store.
    ValidatePlan {
        /// Path to a .air-plan.yaml file.
        plan: Option<PathBuf>,

        /// Optional run profile containing plan and store paths.
        #[arg(long)]
        profile: Option<PathBuf>,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: Option<PathBuf>,

        /// Print a governance and compatibility summary for the validated run plan.
        #[arg(long)]
        explain: bool,
    },
    /// Ask a planner model to generate a dynamic AIR run plan from a task.
    MakePlan {
        /// Natural-language task to plan.
        #[arg(long, conflicts_with = "task_file")]
        task: Option<String>,

        /// File containing the natural-language task to plan.
        #[arg(long, conflicts_with = "task")]
        task_file: Option<PathBuf>,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: PathBuf,

        /// OpenAI-compatible model config JSON containing the planner model alias.
        #[arg(long, required_unless_present = "explain")]
        model_config: Option<PathBuf>,

        /// Planner model alias from --model-config.
        #[arg(long, default_value = "planner")]
        planner_model: String,

        /// Include internal primitive modules in the planner catalog.
        #[arg(long)]
        allow_internal: bool,

        /// Print local component-selection reasoning without calling the planner model.
        #[arg(long)]
        explain: bool,

        /// Optional output path. Prints YAML to stdout when omitted.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Run a state-machine AIR module directly.
    RunModule {
        /// Path to a .air.yaml, .air.yml, or .air.json file.
        file: PathBuf,

        /// Path to a JSON object containing module inputs.
        #[arg(long)]
        input: PathBuf,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Redact sensitive fields and cap trace event size before writing --trace-out (default).
        #[arg(long)]
        trace_redact: bool,

        /// Write raw trace events without redaction.
        #[arg(long, conflicts_with = "trace_redact")]
        trace_raw: bool,

        /// Print human-readable execution logs to stderr.
        #[arg(long)]
        log: bool,

        /// Use built-in example tools such as docs.search.
        #[arg(long)]
        example_tools: bool,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Run an AIR system DAG with built-in mock providers.
    RunSystem {
        /// Path to a .air-system.yaml file.
        file: PathBuf,

        /// Path to a JSON object containing system inputs.
        #[arg(long)]
        input: PathBuf,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Redact sensitive fields and cap trace event size before writing --trace-out (default).
        #[arg(long)]
        trace_redact: bool,

        /// Write raw trace events without redaction.
        #[arg(long, conflicts_with = "trace_redact")]
        trace_raw: bool,

        /// Print human-readable execution logs to stderr.
        #[arg(long)]
        log: bool,

        /// Use built-in example tools such as docs.search.
        #[arg(long)]
        example_tools: bool,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Run a dynamic AIR run plan against a module store.
    RunPlan {
        /// Path to a .air-plan.yaml file.
        plan: Option<PathBuf>,

        /// Optional run profile containing plan, store, input, and provider config.
        #[arg(long)]
        profile: Option<PathBuf>,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: Option<PathBuf>,

        /// Path to a JSON object containing plan inputs.
        #[arg(long)]
        input: Option<PathBuf>,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Redact sensitive fields and cap trace event size before writing --trace-out (default).
        #[arg(long)]
        trace_redact: bool,

        /// Write raw trace events without redaction.
        #[arg(long, conflicts_with = "trace_redact")]
        trace_raw: bool,

        /// Optional JSON state output path for AIR resume.
        #[arg(long)]
        state_out: Option<PathBuf>,

        /// Optional JSON state checkpoint path updated after each completed module.
        #[arg(long)]
        checkpoint_out: Option<PathBuf>,

        /// Optional JIT cache directory for trace-specialized hot-path RunPlans.
        #[arg(long)]
        jit_cache: Option<PathBuf>,

        /// Execute eligible schedule groups concurrently with independent provider instances.
        #[arg(long)]
        parallel: bool,

        /// Print human-readable execution logs to stderr.
        #[arg(long)]
        log: bool,

        /// Use built-in example tools such as docs.search.
        #[arg(long)]
        example_tools: bool,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Resume a dynamic AIR run plan from a saved AIR state.
    ResumePlan {
        /// Path to a .air-plan.yaml file.
        plan: Option<PathBuf>,

        /// Optional run profile containing plan, store, input, and provider config.
        #[arg(long)]
        profile: Option<PathBuf>,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: Option<PathBuf>,

        /// Path to a JSON object containing plan inputs.
        #[arg(long)]
        input: Option<PathBuf>,

        /// Path to a JSON state file produced by run-plan --state-out.
        #[arg(long)]
        state: PathBuf,

        /// Override an output endpoint before resuming, as endpoint=json.
        #[arg(long = "override")]
        overrides: Vec<String>,

        /// Optional OpenAI-compatible model config JSON.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Optional JSONL trace output path.
        #[arg(long)]
        trace_out: Option<PathBuf>,

        /// Redact sensitive fields and cap trace event size before writing --trace-out (default).
        #[arg(long)]
        trace_redact: bool,

        /// Write raw trace events without redaction.
        #[arg(long, conflicts_with = "trace_redact")]
        trace_raw: bool,

        /// Optional JSON state output path for AIR resume.
        #[arg(long)]
        state_out: Option<PathBuf>,

        /// Optional JSON state checkpoint path updated after each completed module.
        #[arg(long)]
        checkpoint_out: Option<PathBuf>,

        /// Print human-readable execution logs to stderr.
        #[arg(long)]
        log: bool,

        /// Use built-in example tools such as docs.search.
        #[arg(long)]
        example_tools: bool,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Replay final output from a JSONL trace.
    Replay {
        /// Path to a trace JSONL file.
        trace: PathBuf,

        /// Specialize the trace back into a validated AIR run plan.
        #[arg(long)]
        specialize_run_plan: bool,

        /// Path to a .air-store.yaml file used for trace specialization validation.
        #[arg(long)]
        store: Option<PathBuf>,

        /// Optional specialized .air-plan.yaml output path. Prints YAML when omitted.
        #[arg(long)]
        output: Option<PathBuf>,

        /// Optional cache identity JSON output path for specialized traces.
        #[arg(long)]
        identity_out: Option<PathBuf>,

        /// Print trace statistics and final output as JSON.
        #[arg(long)]
        stats: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Create an editable AIR project manifest draft.
    Plan {
        /// Project goal to turn into an editable task manifest.
        goal: String,

        /// Output project manifest path. Prints YAML to stdout when omitted.
        #[arg(long)]
        output: Option<PathBuf>,

        /// OpenAI-compatible model config JSON for project task decomposition.
        #[arg(long)]
        model_config: Option<PathBuf>,

        /// Planner model alias from --model-config.
        #[arg(long, default_value = "project_planner")]
        planner_model: String,

        /// Only write a one-task manifest template without calling a model.
        #[arg(long)]
        template: bool,
    },
    /// Run pending project tasks in dependency order, or one selected task.
    Run {
        /// Project manifest path.
        #[arg(long, default_value_os_t = default_project_file())]
        file: PathBuf,

        /// Run one task id instead of all pending tasks.
        #[arg(long)]
        task: Option<String>,

        /// Print AIR execution logs while tasks run.
        #[arg(long)]
        log: bool,
    },
    /// Show project task state without calling a model.
    Status {
        /// Project manifest path.
        #[arg(long, default_value_os_t = default_project_file())]
        file: PathBuf,
    },
    /// Re-run deterministic verification and diff constraints without calling a model.
    Verify {
        /// Project manifest path.
        #[arg(long, default_value_os_t = default_project_file())]
        file: PathBuf,

        /// Verify one task id instead of all tasks.
        #[arg(long)]
        task: Option<String>,
    },
}

fn main() -> Result<()> {
    load_dotenv();
    let cli = Cli::parse();

    match cli.command {
        Command::Skill { command } => match command {
            SkillCommand::List => list_skills(),
            SkillCommand::Validate { skill } => validate_skill(&skill),
            SkillCommand::Audit { skill, write } => audit_skill(&skill, write),
            SkillCommand::Import { source, out } => import_skill(&source, out.as_deref()),
            SkillCommand::Route {
                task,
                top_k,
                explain,
            } => route_skill(SkillRouteOptions {
                task,
                top_k,
                explain,
            }),
            SkillCommand::Explain { skill, profile } => explain_skill(&skill, profile),
        },
        Command::Bench { command } => match command {
            BenchCommand::Code {
                suite,
                out_dir,
                profile,
                model_config,
                task,
                limit,
                log,
                keep_workdirs,
                refresh,
                report,
            } => bench_code_agent(BenchCodeAgentOptions {
                suite,
                out_dir,
                profile,
                model_config,
                task,
                limit,
                log,
                keep_workdirs,
                refresh,
                report,
            }),
            BenchCommand::Skill {
                skill,
                skills,
                suite,
                out_dir,
                model_config,
                task,
                limit,
                compare_no_skill,
                log,
                keep_workdirs,
                refresh,
                report,
            } => bench_skill(BenchSkillOptions {
                skill,
                skills,
                suite,
                out_dir,
                model_config,
                task,
                limit,
                compare_no_skill,
                log,
                keep_workdirs,
                refresh,
                report,
            }),
            BenchCommand::Project { suite, out_dir } => {
                bench_project(BenchProjectOptions { suite, out_dir })
            }
        },
        Command::Dev { command } => run_dev_command(command),
        Command::Project { command } => match command {
            ProjectCommand::Plan {
                goal,
                output,
                model_config,
                planner_model,
                template,
            } => project_plan(ProjectPlanOptions {
                goal,
                output,
                model_config,
                planner_model,
                template,
            }),
            ProjectCommand::Run { file, task, log } => {
                project_run(ProjectRunOptions { file, task, log })
            }
            ProjectCommand::Status { file } => project_status(ProjectStatusOptions { file }),
            ProjectCommand::Verify { file, task } => {
                project_verify(ProjectVerifyOptions { file, task })
            }
        },
        Command::Run {
            target,
            mode,
            top_k,
            skills,
            project_file,
            plan_only,
            execute,
            planner_model,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            log,
            tool_config,
            artifact_out,
            replay_artifact,
            replay_from,
        } => run_entry_task(EntryTaskOptions {
            task: target,
            mode,
            top_k,
            skills,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            log,
            tool_config,
            artifact_out,
            replay_artifact,
            replay_from,
            project_file,
            plan_only,
            execute,
            planner_model,
        }),
    }
}

fn run_dev_command(command: DevCommand) -> Result<()> {
    match command {
        DevCommand::ValidateModule { file } => validate(file),
        DevCommand::ValidateSystem { file } => validate_system(file),
        DevCommand::ValidatePlan {
            plan,
            profile,
            store,
            explain,
        } => validate_plan(ValidatePlanOptions {
            plan,
            profile,
            store,
            explain,
        }),
        DevCommand::MakePlan {
            task,
            task_file,
            store,
            model_config,
            planner_model,
            allow_internal,
            explain,
            output,
        } => plan_task(PlanOptions {
            task,
            task_file,
            store,
            model_config,
            planner_model,
            allow_internal,
            explain,
            output,
        }),
        DevCommand::RunModule {
            file,
            input,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            log,
            example_tools,
            tool_config,
        } => run(
            file,
            input,
            model_config,
            trace_out,
            trace_redact || !trace_raw,
            log,
            example_tools,
            tool_config,
        ),
        DevCommand::RunSystem {
            file,
            input,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            log,
            example_tools,
            tool_config,
        } => run_system(
            file,
            input,
            model_config,
            trace_out,
            trace_redact || !trace_raw,
            log,
            example_tools,
            tool_config,
        ),
        DevCommand::RunPlan {
            plan,
            profile,
            store,
            input,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            state_out,
            checkpoint_out,
            jit_cache,
            parallel,
            log,
            example_tools,
            tool_config,
        } => run_plan(RunPlanOptions {
            plan,
            profile,
            store,
            input,
            input_values: None,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            state_out,
            checkpoint_out,
            jit_cache,
            parallel,
            log,
            example_tools,
            tool_config,
            model_replay: None,
        }),
        DevCommand::ResumePlan {
            plan,
            profile,
            store,
            input,
            state,
            overrides,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            state_out,
            checkpoint_out,
            log,
            example_tools,
            tool_config,
        } => resume_plan(ResumePlanOptions {
            plan,
            profile,
            store,
            input,
            state,
            overrides,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            state_out,
            checkpoint_out,
            log,
            example_tools,
            tool_config,
        }),
        DevCommand::Replay {
            trace,
            specialize_run_plan,
            store,
            output,
            identity_out,
            stats,
        } => replay(ReplayOptions {
            trace,
            specialize_run_plan,
            store,
            output,
            identity_out,
            stats,
        }),
    }
}

fn validate_system(file: PathBuf) -> Result<()> {
    let system = air_linker::parse_system_file(&file)?;
    let base_dir = module_base_dir_for_modules(&system.modules)?;
    let report = air_linker::validate_system(&system, base_dir);

    if report.diagnostics.is_empty() {
        println!("ok: {} {}", system.system.name, system.system.version);
        return Ok(());
    }

    emit_diagnostics(&report.diagnostics);

    if report.is_success() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn validate_plan(options: ValidatePlanOptions) -> Result<()> {
    let ValidatePlanOptions {
        plan,
        profile,
        store,
        explain,
    } = options;

    let profile = match profile {
        Some(path) => Some((path.clone(), read_run_plan_profile(&path)?)),
        None => None,
    };
    let plan = plan
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.plan))
        })
        .ok_or_else(|| anyhow::anyhow!("validate-plan requires a plan path or --profile"))?;
    let store = store
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.store))
        })
        .ok_or_else(|| anyhow::anyhow!("validate-plan requires --store or --profile"))?;

    let plan = air_linker::parse_run_plan_file(plan)?;
    let store_path = store;
    let store = air_linker::parse_module_store_file(&store_path)?;
    let base_dir = module_base_dir_for_store_path(&store, &store_path);
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);

    emit_diagnostics(&report.diagnostics);

    if report.is_success() {
        if explain {
            let explanation = build_plan_explanation(&plan, &store, &base_dir)?;
            print!("{}", format_plan_explanation(&explanation));
        } else if report.diagnostics.is_empty() {
            println!("ok: {} {}", plan.plan.name, plan.plan.version);
        }
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn validate(file: PathBuf) -> Result<()> {
    let module = air_parser::parse_air_file(&file)?;
    let report = air_verify::verify(&module);

    if report.diagnostics.is_empty() {
        println!("ok: {} {}", module.agent.name, module.agent.version);
        return Ok(());
    }

    emit_diagnostics(&report.diagnostics);

    if report.is_success() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    file: PathBuf,
    input: PathBuf,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    log: bool,
    example_tools: bool,
    tool_config: Option<PathBuf>,
) -> Result<()> {
    let module = air_parser::parse_air_file(&file)?;
    let report = air_verify::verify(&module);
    if !report.is_success() {
        emit_diagnostics(&report.diagnostics);
        std::process::exit(1);
    }

    let input = fs::read_to_string(input)?;
    let Value::Object(inputs) = serde_json::from_str::<Value>(&input)? else {
        anyhow::bail!("--input must be a JSON object");
    };

    let observe = log || trace_out.is_some();
    let mut observed_trace = Vec::new();
    let tools = ToolProviderChoice::from_config(tool_config, example_tools)?;
    let result = if let Some(model_config) = model_config {
        let models = ModelProviderChoice::from_config_file_with_provider_io(model_config, log)?;
        let mut vm = Vm { tools, models };
        if observe {
            let result = vm.run_with_observer(&module, inputs, |event| {
                observe_event_with_trace_file(
                    event,
                    log,
                    &mut observed_trace,
                    trace_out.as_ref(),
                    trace_redact,
                )
            });
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            vm.run(&module, inputs)?
        }
    } else {
        let mut vm = Vm {
            tools,
            models: ModelProviderChoice::echo(),
        };
        if observe {
            let result = vm.run_with_observer(&module, inputs, |event| {
                observe_event_with_trace_file(
                    event,
                    log,
                    &mut observed_trace,
                    trace_out.as_ref(),
                    trace_redact,
                )
            });
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            vm.run(&module, inputs)?
        }
    };
    if let Some(trace_out) = trace_out {
        let mut trace = result.trace.clone();
        trace.push(system_return_event(result.outputs.clone()));
        write_trace(trace_out, &trace, trace_redact)?;
    }
    println!("{}", serde_json::to_string_pretty(&result.outputs)?);

    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn run_system(
    file: PathBuf,
    input: PathBuf,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    log: bool,
    example_tools: bool,
    tool_config: Option<PathBuf>,
) -> Result<()> {
    let system = air_linker::parse_system_file(&file)?;
    let base_dir = module_base_dir_for_modules(&system.modules)?;
    let report = air_linker::validate_system(&system, &base_dir);
    if !report.is_success() {
        emit_diagnostics(&report.diagnostics);
        std::process::exit(1);
    }

    let input = fs::read_to_string(input)?;
    let Value::Object(inputs) = serde_json::from_str::<Value>(&input)? else {
        anyhow::bail!("--input must be a JSON object");
    };

    let observe = log || trace_out.is_some();
    let mut observed_trace = Vec::new();
    let tools = ToolProviderChoice::from_config(tool_config, example_tools)?;
    let result = if let Some(model_config) = model_config {
        let models = ModelProviderChoice::from_config_file_with_provider_io(model_config, log)?;
        if observe {
            let result = air_linker::run_system_with_observer(
                &system,
                base_dir,
                inputs,
                tools,
                models,
                |event| {
                    observe_event_with_trace_file(
                        event,
                        log,
                        &mut observed_trace,
                        trace_out.as_ref(),
                        trace_redact,
                    )
                },
            );
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            air_linker::run_system(&system, base_dir, inputs, tools, models)?
        }
    } else if observe {
        let result = air_linker::run_system_with_observer(
            &system,
            base_dir,
            inputs,
            tools,
            ModelProviderChoice::echo(),
            |event| {
                observe_event_with_trace_file(
                    event,
                    log,
                    &mut observed_trace,
                    trace_out.as_ref(),
                    trace_redact,
                )
            },
        );
        if result.is_err() {
            write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
        }
        result?
    } else {
        air_linker::run_system(
            &system,
            base_dir,
            inputs,
            tools,
            ModelProviderChoice::echo(),
        )?
    };
    if let Some(trace_out) = trace_out {
        let mut trace = result.trace.clone();
        trace.push(system_return_event(result.outputs.clone()));
        write_trace(trace_out, &trace, trace_redact)?;
    }
    println!("{}", serde_json::to_string_pretty(&result.outputs)?);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{module_catalog, parse_planner_response, planner_request, recipe_catalog};
    use crate::profile::read_json_object;
    use crate::run_plan::{
        run_plan_with_inputs, run_plan_with_inputs_capture, write_checkpoint_state, PlanStateFile,
        RunPlanExecutionOptions,
    };
    use crate::tools::{provider_error_snippet, search_docs, ConfigTools, LocalDoc};
    use air_runtime::{read_trace_jsonl, TraceStatus};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn materializes_recipe_selection_from_planner_response() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();

        let plan = parse_planner_response(
            json!({
                "recipe_id": "deep_research.four_topic_report@0.1.0",
                "rationale": "matches the four-topic deep research request"
            }),
            &store,
            &root,
            true,
        )
        .unwrap();

        assert_eq!(plan.nodes.len(), 6);
        assert_eq!(plan.outputs["result"], "final.final_report");
        assert_eq!(
            plan.decisions[0].selected,
            vec!["deep_research.four_topic_report@0.1.0"]
        );
    }

    #[test]
    fn rejects_internal_recipe_selection_without_allow_internal() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();
        store.recipes[0].visibility = air_linker::ModuleVisibility::Internal;

        let error = parse_planner_response(
            json!({"recipe_id": "deep_research.four_topic_report@0.1.0"}),
            &store,
            &root,
            false,
        )
        .unwrap_err();

        assert!(error.to_string().contains("without --allow-internal"));
    }

    #[test]
    fn parses_recipe_selection_from_content_wrapper() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();

        let plan = parse_planner_response(
            json!({
                "content": "```json\n{\"recipe_id\":\"deep_research.four_topic_report@0.1.0\"}\n```"
            }),
            &store,
            &root,
            true,
        )
        .unwrap();

        assert_eq!(plan.entry, "plan");
    }

    #[test]
    fn planner_request_teaches_dynamic_fanout_ir() {
        let store = air_linker::ModuleStore {
            store: air_linker::StoreMetadata {
                name: "test-store".to_string(),
                version: "0.1.0".to_string(),
            },
            imports: vec![],
            modules: Default::default(),
            recipes: vec![],
        };

        let request = planner_request("fan out over runtime topics", &store, vec![], vec![], true);
        let instructions = request["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .unwrap()
            .join("\n");

        assert!(instructions.contains("Use dynamic.fanouts"));
        assert!(instructions.contains("$item"));
        assert!(instructions.contains("$each.<input>"));
        assert!(instructions.contains("Do not pre-list dynamic fan-out instances"));
        assert!(instructions.contains("$parent.<array_output>"));
        assert_eq!(
            request["example_dynamic_fanout_shape"]["dynamic"]["fanouts"][0]["source"],
            json!("plan.plan.topics")
        );
        assert_eq!(
            request["example_dynamic_fanout_shape"]["dynamic"]["fanouts"][0]["input"][1]["from"],
            json!("$item")
        );
        assert_eq!(
            request["example_nested_dynamic_fanout_shape"]["dynamic"]["fanouts"][1]["after"],
            json!("parent_wave")
        );
        assert_eq!(
            request["example_nested_dynamic_fanout_shape"]["dynamic"]["fanouts"][1]["source"],
            json!("$parent.followups")
        );
    }

    #[test]
    fn planner_request_ranks_large_components_before_small_building_blocks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Build a four topic deep research report with web research and synthesis.",
            &store,
            catalog,
            recipes,
            true,
        );

        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["id"],
            json!("deep_research.four_topic_report@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["source"],
            json!("recipe")
        );
        let recommended = request["module_store"]["component_selection"]["recommended"]
            .as_array()
            .unwrap();
        let topic_position = recommended
            .iter()
            .position(|candidate| candidate["id"] == json!("deep_research.research_topic@0.1.0"))
            .unwrap();
        let final_position = recommended
            .iter()
            .position(|candidate| candidate["id"] == json!("deep_research.final_report@0.1.0"))
            .unwrap();
        assert!(final_position < topic_position);

        let instructions = request["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(instructions.contains("component_selection.first_choice"));
        assert!(instructions.contains("fallback-to-primitives"));
    }

    #[test]
    fn planner_request_ranks_public_composite_over_internal_primitives() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("tests/plans/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Triage customer billing feedback, extract the issue, score risk, and generate a report.",
            &store,
            catalog,
            recipes,
            true,
        );

        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["id"],
            json!("customer.triage@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["tier"],
            json!("large_component")
        );
    }

    #[test]
    fn planner_request_does_not_force_code_edit_loop_for_open_ended_questions() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("skills/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Explore how command_run is implemented and identify relevant repository files.",
            &store,
            catalog,
            recipes,
            true,
        );

        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["id"],
            json!("context.compact@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["tier"],
            json!("public_building_block")
        );
    }

    #[test]
    fn planner_request_ranks_code_edit_loop_for_plan_act_observe_tasks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("skills/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Explore the repository with a dynamic plan-act-observe loop that lets the model choose declared read-only tools.",
            &store,
            catalog,
            recipes,
            true,
        );

        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["id"],
            json!("code.edit@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["source"],
            json!("module")
        );
    }

    #[test]
    fn planner_request_routes_code_agent_task_shapes_to_matching_components() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("skills/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();

        for (task, expected_id, expected_source) in [
            (
                "Review the command_run implementation for safety, provenance, and diagnostics.",
                "code.edit@0.1.0",
                "module",
            ),
            (
                "Edit a failing test using structured diagnostics, apply a bounded patch, and retest.",
                "code.edit@0.1.0",
                "module",
            ),
            (
                "Explore how command_run is implemented and identify relevant repository files.",
                "context.compact@0.1.0",
                "module",
            ),
        ] {
            let catalog = module_catalog(&store, &root, true).unwrap();
            let recipes = recipe_catalog(&store, &root, true).unwrap();
            let request = planner_request(task, &store, catalog, recipes, true);
            let first_choice = &request["module_store"]["component_selection"]["first_choice"];

            assert_eq!(
                first_choice["id"],
                json!(expected_id),
                "task should route to {expected_id}: {task}"
            );
            assert_eq!(
                first_choice["source"],
                json!(expected_source),
                "task should route through {expected_source}: {task}"
            );
        }
    }

    #[test]
    fn planner_request_hides_unmatched_code_agent_components_from_recommendations() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("skills/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();
        let catalog = module_catalog(&store, &root, true).unwrap();
        let recipes = recipe_catalog(&store, &root, true).unwrap();

        let request = planner_request(
            "Build an Apple-style landing page as a single HTML file and verify screenshots.",
            &store,
            catalog,
            recipes,
            true,
        );
        let recommended = request["module_store"]["component_selection"]["recommended"]
            .as_array()
            .unwrap();

        assert!(recommended
            .iter()
            .all(|candidate| candidate["matched_terms"]
                .as_array()
                .is_some_and(|terms| !terms.is_empty())));
        assert!(!recommended
            .iter()
            .any(|candidate| candidate["id"] == json!("context.compact@0.1.0")));
    }

    #[test]
    fn local_docs_search_dedupes_raw_content_before_limit() {
        let documents = vec![
            LocalDoc {
                id: "a".to_string(),
                title: "EV readiness A".to_string(),
                content: "Duplicate EV readiness evidence".to_string(),
            },
            LocalDoc {
                id: "b".to_string(),
                title: "EV readiness B".to_string(),
                content: "Duplicate EV readiness evidence".to_string(),
            },
            LocalDoc {
                id: "c".to_string(),
                title: "EV readiness C".to_string(),
                content: "Distinct EV readiness evidence".to_string(),
            },
        ];

        let result = search_docs(&json!({"query": "EV readiness"}), &documents, 2);

        assert_eq!(result["documents"].as_array().unwrap().len(), 2);
        assert_eq!(result["documents"][0]["id"], json!("a"));
        assert_eq!(result["documents"][1]["id"], json!("c"));
    }

    #[test]
    fn read_json_object_reports_label_and_path() {
        let path = temp_file_path("air-input-diagnostic", "json");
        fs::write(&path, "[]").unwrap();

        let error = read_json_object(path.clone(), "--input").unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("--input JSON file"));
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("must contain a JSON object"));
    }

    #[test]
    fn tool_config_reports_invalid_http_json_url_field() {
        let path = temp_file_path("air-tool-diagnostic", "json");
        fs::write(
            &path,
            r#"{
              "tools": {
                "web.search": {
                  "kind": "http_json",
                  "capability": "search",
                  "url": "localhost/search"
                }
              }
            }"#,
        )
        .unwrap();

        let error = ConfigTools::from_file(path.clone()).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("tool config"));
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("tools.web.search.url"));
        assert!(message.contains("http:// or https://"));
    }

    #[test]
    fn provider_error_snippet_redacts_http_tool_body() {
        let body = format!(
            "token=tool-secret Authorization: Bearer header-secret {}",
            "x".repeat(4096)
        );
        let message = provider_error_snippet(&body);

        assert!(!message.contains("tool-secret"));
        assert!(!message.contains("header-secret"));
        assert!(message.contains("[AIR_REDACTED]"));
        assert!(message.contains("[AIR_TRUNCATED]"));
        assert!(message.len() < body.len());
    }

    #[test]
    fn tool_config_reports_invalid_local_docs_limit_field() {
        let path = temp_file_path("air-doc-tool-diagnostic", "json");
        fs::write(
            &path,
            r#"{
              "tools": {
                "docs.search": {
                  "kind": "local_docs_search",
                  "capability": "retrieval.local",
                  "documents": [
                    {"id": "doc-1", "title": "Doc", "content": "Content"}
                  ],
                  "max_results": 0
                }
              }
            }"#,
        )
        .unwrap();

        let error = ConfigTools::from_file(path.clone()).unwrap_err();
        let _ = fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("tools.docs.search.max_results"));
        assert!(message.contains("greater than 0"));
    }

    fn temp_file_path(prefix: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }

    fn temp_repo_file_path(root: &std::path::Path, prefix: &str, extension: &str) -> PathBuf {
        let dir = root.join("target/generated/test-tmp");
        fs::create_dir_all(&dir).unwrap();
        dir.join(format!(
            "{prefix}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }

    fn temp_dir_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let source = entry.path();
            let target = to.join(entry.file_name());
            if source.is_dir() {
                copy_dir_recursive(&source, &target);
            } else {
                fs::copy(&source, &target).unwrap();
            }
        }
    }

    #[test]
    fn code_agent_fixture_runs_through_workspace_tests() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture_root = temp_dir_path("air-code-agent-fixture");
        let trace_path = temp_repo_file_path(&root, "code-agent-minimal", "jsonl");
        let _ = fs::remove_dir_all(&fixture_root);
        fs::create_dir_all(fixture_root.join("examples")).unwrap();
        fs::create_dir_all(fixture_root.join("modules")).unwrap();
        copy_dir_recursive(
            &root.join("skills/code-agent"),
            &fixture_root.join("skills/code-agent"),
        );
        copy_dir_recursive(&root.join("modules/std"), &fixture_root.join("modules/std"));

        let git_init = std::process::Command::new("git")
            .arg("-C")
            .arg(&fixture_root)
            .arg("init")
            .arg("-q")
            .status()
            .unwrap();
        assert!(git_init.success());
        let git_add = std::process::Command::new("git")
            .arg("-C")
            .arg(&fixture_root)
            .arg("add")
            .arg("examples")
            .arg("modules")
            .status()
            .unwrap();
        assert!(git_add.success());

        let result = run_plan_with_inputs_capture(
            fixture_root.join("skills/code-agent/code-edit.air-plan.yaml"),
            fixture_root.join("skills/code-agent/module-store.air-store.yaml"),
            serde_json::Map::from_iter([(
                "task".to_string(),
                json!("fix the failing add function and retest"),
            )]),
            RunPlanExecutionOptions {
                model_config: Some(
                    fixture_root.join("skills/code-agent/fixtures/model-fixtures.json"),
                ),
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: Some(fixture_root.join("skills/code-agent/tools.json")),
                model_replay: None,
            },
        )
        .unwrap();

        let edit = &result["edit"];
        assert_eq!(edit["final_success"], json!(true));
        assert_eq!(edit["patch_applied"], json!(true));
        assert!(edit["workspace_diff"]["diff"]
            .as_str()
            .unwrap()
            .contains("skills/code-agent/edit-fixture/math.js"));

        let trace = read_trace_jsonl(&trace_path).unwrap();
        let first_decider = trace
            .iter()
            .find(|event| {
                event.action == "model_call_start"
                    && event.meta.as_ref().unwrap()["model"] == json!("code_edit_decider")
            })
            .expect("code_edit_decider model call");
        assert_eq!(
            first_decider.input.as_ref().unwrap()["allowed_tools"],
            json!([
                "question",
                "bash",
                "grep",
                "glob",
                "lsp",
                "task",
                "read_contains",
                "read_range",
                "edit",
                "write",
                "apply_patch",
                "webfetch",
                "todowrite",
                "todoread",
                "skill"
            ])
        );

        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_dir_all(fixture_root);
    }

    #[test]
    fn writes_resume_compatible_checkpoint_state() {
        let path = std::env::temp_dir().join(format!(
            "air-checkpoint-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let checkpoint = air_linker::SystemCheckpoint {
            completed_module: "plan".to_string(),
            module_outputs: BTreeMap::from([(
                "plan".to_string(),
                serde_json::Map::from_iter([("plan".to_string(), json!({"topic_1": "EV"}))]),
            )]),
        };

        write_checkpoint_state(Some(&path), &checkpoint).unwrap();

        let state: PlanStateFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let _ = fs::remove_file(path);

        assert_eq!(
            state.status,
            air_linker::RunStatus::InProgress {
                after: "plan".to_string()
            }
        );
        assert_eq!(state.outputs, json!({}));
        assert_eq!(state.module_outputs["plan"]["plan"]["topic_1"], json!("EV"));
    }

    #[test]
    fn reads_run_plan_profile_with_relative_paths() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let profile_path = root.join("examples/deep-research/profile.air-profile.yaml");
        let profile = read_run_plan_profile(&profile_path).unwrap();

        assert_eq!(
            profile.plan,
            PathBuf::from("deep-research-clarified.air-plan.yaml")
        );
        assert_eq!(
            resolve_profile_path(&profile_path, &profile.plan),
            root.join("examples/deep-research/deep-research-clarified.air-plan.yaml")
        );
        assert_eq!(
            resolve_profile_path(&profile_path, &profile.store),
            root.join("examples/deep-research/module-store.air-store.yaml")
        );
        assert_eq!(
            resolve_profile_path(&profile_path, profile.input_file.as_ref().unwrap()),
            root.join("examples/deep-research/input.json")
        );
    }

    #[test]
    fn default_help_exposes_only_primary_commands() {
        let mut output = Vec::new();
        <Cli as clap::CommandFactory>::command()
            .write_help(&mut output)
            .unwrap();
        let help = String::from_utf8(output).unwrap();
        let command_names = help
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter(|line| !line.starts_with("-"))
            .filter_map(|line| line.split_whitespace().next())
            .collect::<Vec<_>>();

        for command in ["run", "skill", "bench", "dev"] {
            assert!(
                command_names.contains(&command),
                "expected {command} in help"
            );
        }

        for command in [
            "project",
            "validate-plan",
            "plan",
            "run-plan",
            "resume-plan",
            "validate-system",
            "run-system",
            "validate",
            "code",
            "lower",
            "lower-plan",
            "replay",
            "deep-research",
        ] {
            assert!(
                !command_names.contains(&command),
                "did not expect {command} in default help"
            );
        }
    }

    #[test]
    fn dev_help_exposes_internal_commands() {
        let mut output = Vec::new();
        let mut command = <Cli as clap::CommandFactory>::command();
        let dev = command.find_subcommand_mut("dev").unwrap();
        dev.write_help(&mut output).unwrap();
        let help = String::from_utf8(output).unwrap();
        let command_names = help
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter(|line| !line.starts_with("-"))
            .filter_map(|line| line.split_whitespace().next())
            .collect::<Vec<_>>();

        for command in [
            "validate-module",
            "validate-system",
            "validate-plan",
            "make-plan",
            "run-module",
            "run-system",
            "run-plan",
            "resume-plan",
            "replay",
        ] {
            assert!(
                command_names.contains(&command),
                "expected {command} in dev help"
            );
        }
    }

    #[test]
    fn dev_run_plan_accepts_profile() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "run-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
            "--log",
        ])
        .unwrap();

        let Command::Dev {
            command: DevCommand::RunPlan {
                plan, profile, log, ..
            },
        } = cli.command
        else {
            panic!("expected dev run-plan command");
        };

        assert_eq!(plan, None);
        assert_eq!(
            profile,
            Some(PathBuf::from(
                "examples/deep-research/profile.air-profile.yaml"
            ))
        );
        assert!(log);
    }

    #[test]
    fn skill_help_exposes_lifecycle_not_execution_debug_commands() {
        let mut output = Vec::new();
        let mut command = <Cli as clap::CommandFactory>::command();
        let skill = command.find_subcommand_mut("skill").unwrap();
        skill.write_help(&mut output).unwrap();
        let help = String::from_utf8(output).unwrap();
        let command_names = help
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter(|line| !line.starts_with("-"))
            .filter_map(|line| line.split_whitespace().next())
            .collect::<Vec<_>>();

        for command in ["list", "validate", "audit", "import", "route", "explain"] {
            assert!(
                command_names.contains(&command),
                "expected {command} in skill help"
            );
        }

        for command in ["run", "compile"] {
            assert!(
                !command_names.contains(&command),
                "did not expect {command} in default skill help"
            );
        }
    }

    #[test]
    fn skill_route_accepts_explain_flag() {
        let cli = Cli::try_parse_from([
            "air",
            "skill",
            "route",
            "fix a security vulnerability",
            "--explain",
        ])
        .unwrap();

        let Command::Skill {
            command:
                SkillCommand::Route {
                    task,
                    top_k,
                    explain,
                },
        } = cli.command
        else {
            panic!("expected skill route command");
        };

        assert_eq!(task, "fix a security vulnerability");
        assert_eq!(top_k, 3);
        assert!(explain);
    }

    #[test]
    fn run_accepts_natural_language_task() {
        let cli = Cli::try_parse_from([
            "air",
            "run",
            "use TDD to fix the failing add function",
            "--skills",
            "tdd-workflow",
            "--mode",
            "code",
        ])
        .unwrap();

        let Command::Run {
            target,
            mode,
            skills,
            execute,
            ..
        } = cli.command
        else {
            panic!("expected run command");
        };

        assert_eq!(target, "use TDD to fix the failing add function");
        assert_eq!(mode, EntryMode::Code);
        assert_eq!(skills, vec!["tdd-workflow"]);
        assert!(!execute);
    }

    #[test]
    fn run_project_execute_is_explicit() {
        let cli = Cli::try_parse_from([
            "air",
            "run",
            "refactor the whole crate",
            "--mode",
            "project",
            "--execute",
        ])
        .unwrap();

        let Command::Run {
            mode,
            plan_only,
            execute,
            ..
        } = cli.command
        else {
            panic!("expected run command");
        };

        assert_eq!(mode, EntryMode::Project);
        assert!(!plan_only);
        assert!(execute);
    }

    #[test]
    fn bundled_subagent_configs_use_dev_run_plan() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(|path| path.parent())
            .expect("air-cli crate lives under crates/air-cli");
        let config_path = repo_root.join("skills/code-agent/tools.json");
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        let command = config
            .pointer("/tools/task/subagents/explore/command")
            .and_then(serde_json::Value::as_array)
            .expect("explore subagent command");

        let run_plan_index = command
            .iter()
            .position(|value| value == "run-plan")
            .expect("subagent command includes run-plan");
        assert_eq!(
            command
                .get(run_plan_index.saturating_sub(1))
                .and_then(serde_json::Value::as_str),
            Some("dev")
        );
    }

    #[test]
    fn dev_run_module_accepts_low_level_module_input() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "run-module",
            "examples/model-smoke.air.yaml",
            "--input",
            "examples/model-smoke.input.json",
        ])
        .unwrap();

        let Command::Dev {
            command: DevCommand::RunModule { file, input, .. },
        } = cli.command
        else {
            panic!("expected dev run-module command");
        };

        assert_eq!(file, PathBuf::from("examples/model-smoke.air.yaml"));
        assert_eq!(input, PathBuf::from("examples/model-smoke.input.json"));
    }

    #[test]
    fn skill_explain_accepts_code_agent() {
        let cli = Cli::try_parse_from(["air", "skill", "explain", "code-agent"]).unwrap();

        let Command::Skill {
            command: SkillCommand::Explain { skill, profile },
        } = cli.command
        else {
            panic!("expected skill explain command");
        };

        assert_eq!(skill, "code-agent");
        assert_eq!(profile, None);
    }

    #[test]
    fn skill_validate_audit_parse() {
        for args in [
            ["air", "skill", "validate", "code-agent"].as_slice(),
            ["air", "skill", "audit", "code-agent"].as_slice(),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Command::Skill { .. } = cli.command else {
                panic!("expected skill command");
            };
        }
    }

    #[test]
    fn resume_plan_accepts_profile_without_positional_plan() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "resume-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
            "--state",
            "target/generated/deep_research_profile.state.json",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::ResumePlan {
                    plan,
                    profile,
                    store,
                    input,
                    state,
                    ..
                },
        } = cli.command
        else {
            panic!("expected dev resume-plan command");
        };

        assert_eq!(plan, None);
        assert_eq!(
            profile,
            Some(PathBuf::from(
                "examples/deep-research/profile.air-profile.yaml"
            ))
        );
        assert_eq!(store, None);
        assert_eq!(input, None);
        assert_eq!(
            state,
            PathBuf::from("target/generated/deep_research_profile.state.json")
        );
    }

    #[test]
    fn run_plan_jit_cache_reuses_specialized_dynamic_hot_path() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let unique = format!(
            "air-jit-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let cache_dir = std::env::temp_dir().join(&unique);
        let trace_path = std::env::temp_dir().join(format!("{unique}.trace.jsonl"));
        let store_path = root
            .join("target/generated/test-tmp")
            .join(format!("{unique}.store.yaml"));
        fs::create_dir_all(store_path.parent().unwrap()).unwrap();

        let plan_path = root.join("tests/plans/dynamic-smoke.air-plan.yaml");
        let store = air_linker::parse_module_store_file(
            root.join("tests/plans/dynamic-smoke.air-store.yaml"),
        )
        .unwrap();
        fs::write(&store_path, serde_yaml::to_string(&store).unwrap()).unwrap();
        let inputs = serde_json::Map::new();

        run_plan_with_inputs(
            plan_path.clone(),
            store_path.clone(),
            inputs.clone(),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: None,
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: Some(cache_dir.clone()),
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        let cached_plans = fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "yaml")
            })
            .collect::<Vec<_>>();
        assert_eq!(cached_plans.len(), 1);
        assert!(fs::read_to_string(&cached_plans[0])
            .unwrap()
            .contains("topic_1"));

        run_plan_with_inputs(
            plan_path,
            store_path.clone(),
            inputs,
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: Some(cache_dir.clone()),
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        let trace = read_trace_jsonl(&trace_path).unwrap();
        let resolved = trace
            .iter()
            .find(|event| event.agent == "$planner" && event.action == "resolve_plan")
            .and_then(|event| event.output.as_ref())
            .unwrap();
        assert!(resolved["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == json!("topic_1")));
        assert!(!trace
            .iter()
            .any(|event| { event.agent == "$planner" && event.action == "resolve_dynamic_plan" }));

        let _ = fs::remove_dir_all(cache_dir);
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn run_plan_uses_tool_config_approvals() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let unique = format!(
            "air-approval-smoke-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let store_path = root
            .join("target/generated/test-tmp")
            .join(format!("{unique}.store.yaml"));
        fs::create_dir_all(store_path.parent().unwrap()).unwrap();
        let trace_path = std::env::temp_dir().join(format!("{unique}.trace.jsonl"));
        let plan_path = root.join("tests/plans/approval-smoke.air-plan.yaml");
        let tool_config = root.join("tests/plans/approval-smoke.tools.json");

        let store = air_linker::parse_module_store_file(
            root.join("tests/plans/approval-smoke.air-store.yaml"),
        )
        .unwrap();
        fs::write(&store_path, serde_yaml::to_string(&store).unwrap()).unwrap();

        run_plan_with_inputs(
            plan_path,
            store_path.clone(),
            serde_json::Map::from_iter([("deployment_id".to_string(), json!("deploy-123"))]),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: Some(tool_config),
                model_replay: None,
            },
        )
        .unwrap();

        let trace = read_trace_jsonl(&trace_path).unwrap();
        let approval = trace
            .iter()
            .find(|event| event.action == "approval")
            .expect("approval event");
        assert_eq!(approval.status, TraceStatus::Ok);
        assert_eq!(approval.meta.as_ref().unwrap()["approved"], json!(true));
        assert_eq!(
            approval.meta.as_ref().unwrap()["approver"],
            json!("verification")
        );

        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn run_plan_accepts_parallel_flag_with_profile() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "run-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
            "--parallel",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::RunPlan {
                    plan,
                    profile,
                    parallel,
                    ..
                },
        } = cli.command
        else {
            panic!("expected dev run-plan command");
        };

        assert_eq!(plan, None);
        assert_eq!(
            profile,
            Some(PathBuf::from(
                "examples/deep-research/profile.air-profile.yaml"
            ))
        );
        assert!(parallel);
    }

    #[test]
    fn run_plan_parallel_executes_schedule_groups_from_cli_path() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let trace_path = std::env::temp_dir().join(format!(
            "air-parallel-cli-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store_path = temp_repo_file_path(&root, "air-parallel-cli-store", "yaml");
        fs::write(
            &store_path,
            r#"store:
  name: parallel-smoke-store
  version: 0.1.0

modules:
  test.model_smoke@0.1.0:
    path: tests/agents/model-smoke.air.yaml
    kind: primitive
    visibility: public
"#,
        )
        .unwrap();

        run_plan_with_inputs(
            root.join("tests/plans/parallel-smoke.air-plan.yaml"),
            store_path.clone(),
            serde_json::Map::from_iter([("prompt".to_string(), json!("parallel cli"))]),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: true,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        let trace = read_trace_jsonl(&trace_path).unwrap();
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);

        assert!(trace.iter().any(|event| {
            event.agent == "$system"
                && event.action == "schedule_batch"
                && event.input.as_ref().unwrap()["execution"] == "parallel"
        }));
        assert!(trace.iter().any(|event| {
            event.agent == "$system"
                && event.action == "return"
                && event.output.as_ref().unwrap()["left"]["input"] == json!("parallel cli")
                && event.output.as_ref().unwrap()["right"]["input"] == json!("parallel cli")
        }));
    }

    #[test]
    fn replay_specializes_trace_to_validated_run_plan() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let trace_path = temp_repo_file_path(&root, "air-specialize-cli", "jsonl");
        let output_path = trace_path.with_extension("air-plan.yaml");
        let identity_path = trace_path.with_extension("identity.json");
        let store_path = trace_path.with_extension("store.yaml");
        fs::write(
            &store_path,
            r#"store:
  name: parallel-smoke-store
  version: 0.1.0

modules:
  test.model_smoke@0.1.0:
    path: tests/agents/model-smoke.air.yaml
    kind: primitive
    visibility: public
"#,
        )
        .unwrap();

        run_plan_with_inputs(
            root.join("tests/plans/parallel-smoke.air-plan.yaml"),
            store_path.clone(),
            serde_json::Map::from_iter([("prompt".to_string(), json!("jit cli"))]),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: true,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        replay(ReplayOptions {
            trace: trace_path.clone(),
            output: Some(output_path.clone()),
            specialize_run_plan: true,
            store: Some(store_path.clone()),
            identity_out: Some(identity_path.clone()),
            stats: false,
        })
        .unwrap();

        let plan = air_linker::parse_run_plan_file(&output_path).unwrap();
        let identity: Value =
            serde_json::from_str(&fs::read_to_string(&identity_path).unwrap()).unwrap();
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(output_path);
        let _ = fs::remove_file(identity_path);
        let _ = fs::remove_file(store_path);

        assert_eq!(plan.plan.name, "parallel-smoke");
        assert_eq!(
            identity["nodes"]["left"]["store_module"],
            json!("test.model_smoke@0.1.0")
        );
    }

    #[test]
    fn specialized_dynamic_trace_can_be_validated() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let trace_path = temp_repo_file_path(&root, "air-dynamic-specialize", "jsonl");
        let store_path = trace_path.with_extension("store.yaml");
        let specialized_path = trace_path.with_extension("specialized.air-plan.yaml");
        let identity_path = trace_path.with_extension("identity.json");
        let store = air_linker::parse_module_store_file(
            root.join("tests/plans/dynamic-smoke.air-store.yaml"),
        )
        .unwrap();
        fs::write(&store_path, serde_yaml::to_string(&store).unwrap()).unwrap();

        run_plan_with_inputs(
            root.join("tests/plans/dynamic-smoke.air-plan.yaml"),
            store_path.clone(),
            serde_json::Map::new(),
            RunPlanExecutionOptions {
                model_config: None,
                trace_out: Some(trace_path.clone()),
                trace_redact: false,
                state_out: None,
                checkpoint_out: None,
                jit_cache: None,
                parallel: false,
                log: false,
                example_tools: false,
                tool_config: None,
                model_replay: None,
            },
        )
        .unwrap();

        replay(ReplayOptions {
            trace: trace_path.clone(),
            output: Some(specialized_path.clone()),
            specialize_run_plan: true,
            store: Some(store_path.clone()),
            identity_out: Some(identity_path.clone()),
            stats: false,
        })
        .unwrap();

        let specialized = air_linker::parse_run_plan_file(&specialized_path).unwrap();
        let validation = air_linker::validate_run_plan(&specialized, &store, &root);
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);
        let _ = fs::remove_file(specialized_path);
        let _ = fs::remove_file(identity_path);

        assert!(validation.is_success(), "{:?}", validation.diagnostics);
        assert!(specialized.dynamic.is_none());
        assert!(specialized.nodes.iter().any(|node| node.id == "topic_1"));
    }

    #[test]
    fn validate_plan_accepts_profile_without_positional_plan() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "validate-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::ValidatePlan {
                    plan,
                    profile,
                    store,
                    explain,
                },
        } = cli.command
        else {
            panic!("expected dev validate-plan command");
        };

        assert_eq!(plan, None);
        assert_eq!(
            profile,
            Some(PathBuf::from(
                "examples/deep-research/profile.air-profile.yaml"
            ))
        );
        assert_eq!(store, None);
        assert!(!explain);
    }

    #[test]
    fn validate_plan_accepts_explain_flag() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "validate-plan",
            "tests/plans/approval-smoke.air-plan.yaml",
            "--store",
            "tests/plans/approval-smoke.air-store.yaml",
            "--explain",
        ])
        .unwrap();

        let Command::Dev {
            command: DevCommand::ValidatePlan { explain, .. },
        } = cli.command
        else {
            panic!("expected dev validate-plan command");
        };

        assert!(explain);
    }

    #[test]
    fn plan_explain_accepts_no_model_config() {
        let cli = Cli::try_parse_from([
            "air",
            "dev",
            "make-plan",
            "--explain",
            "--store",
            "skills/code-agent/module-store.air-store.yaml",
            "--task",
            "Explore the command_run implementation",
        ])
        .unwrap();

        let Command::Dev {
            command:
                DevCommand::MakePlan {
                    model_config,
                    explain,
                    ..
                },
        } = cli.command
        else {
            panic!("expected dev make-plan command");
        };

        assert_eq!(model_config, None);
        assert!(explain);
    }

    #[test]
    fn plan_requires_model_config_without_explain() {
        let error = Cli::try_parse_from([
            "air",
            "dev",
            "make-plan",
            "--store",
            "skills/code-agent/module-store.air-store.yaml",
            "--task",
            "Explore the command_run implementation",
        ])
        .unwrap_err();

        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn model_profile_keys_are_env_safe() {
        assert_eq!(normalize_model_profile_key("glm"), "GLM");
        assert_eq!(normalize_model_profile_key("local-api"), "LOCAL_API");
        assert_eq!(normalize_model_profile_key(" local api "), "LOCAL_API");
    }
}
