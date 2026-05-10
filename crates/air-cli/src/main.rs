use air_core::Severity;
use air_runtime::{system_return_event, Vm};
mod code_agent;
mod code_budget;
mod code_context;
mod code_input;
mod code_loop;
mod code_pack;
mod code_project_acceptance;
mod code_project_context;
mod code_project_schedule;
mod code_session;
mod explain;
mod models;
mod planner;
mod profile;
mod run_plan;
mod tools;
use crate::code_agent::{code, code_session, CodeOptions, CodeRecipe, CodeSessionOptions};
use crate::explain::{build_plan_explanation, format_plan_explanation};
use crate::models::ModelProviderChoice;
use crate::planner::{
    module_base_dir_for_modules, module_base_dir_for_store_path, plan_task, PlanOptions,
    ValidatePlanOptions,
};
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::run_plan::{
    observe_event, replay, resume_plan, run_plan, write_partial_trace, write_trace, ReplayOptions,
    ResumePlanOptions, RunPlanOptions,
};
use crate::tools::ToolProviderChoice;
use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "air")]
#[command(about = "AIR compiler CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Run an AIR coding-agent recipe from a task and typed context.
    Code {
        /// Natural-language coding task.
        task: String,

        /// Coding recipe to run.
        #[arg(long, value_enum, default_value = "auto")]
        recipe: CodeRecipe,

        /// Primary file the coding agent is allowed to inspect.
        #[arg(long)]
        target: Option<PathBuf>,

        /// Allowlisted test command alias from the selected tool config.
        #[arg(long)]
        test: Option<String>,

        /// Optional search/query string. Defaults to the task.
        #[arg(long)]
        query: Option<String>,

        /// Extra related file to read. May be repeated.
        #[arg(long)]
        related: Vec<PathBuf>,

        /// External search query for review recipes. Defaults to the task.
        #[arg(long)]
        search_query: Option<String>,

        /// Repository search query for review recipes. Defaults to --query or --target.
        #[arg(long)]
        repo_query: Option<String>,

        /// Required review term. May be repeated.
        #[arg(long = "required-term")]
        required_terms: Vec<String>,

        /// Output file for build recipes.
        #[arg(long)]
        output: Option<PathBuf>,

        /// Brand name for build recipes.
        #[arg(long)]
        brand: Option<String>,

        /// Product name for build recipes. Defaults to --brand.
        #[arg(long)]
        product: Option<String>,

        /// Build constraint. May be repeated.
        #[arg(long = "constraint")]
        constraints: Vec<String>,

        /// Continue into patch generation even when the initial repair test already passes.
        #[arg(long)]
        force_patch: bool,

        /// Code-agent pack manifest. Defaults to examples/code-agent/code-agent.air-pack.yaml.
        #[arg(long)]
        pack: Option<PathBuf>,

        /// Coding-agent run profile. Defaults from --recipe.
        #[arg(long)]
        profile: Option<PathBuf>,

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

        /// Persist and reuse bounded AIR code-agent turn context from this JSON session file.
        #[arg(long)]
        session: Option<PathBuf>,

        /// Execute eligible schedule groups and dynamic fan-out modules in parallel.
        #[arg(long)]
        parallel: bool,

        /// Print human-readable execution logs to stderr.
        #[arg(long)]
        log: bool,

        /// Print the resolved recipe, profile, and typed input without running.
        #[arg(long)]
        explain: bool,

        /// Re-run the selected coding recipe until its completion signal passes or the loop budget is exhausted.
        #[arg(long = "loop")]
        loop_enabled: bool,

        /// After a project plan, execute up to --max-iterations planned tasks through their declared AIR recipes.
        #[arg(long, conflicts_with = "loop_enabled")]
        execute_plan: bool,

        /// Maximum iterations for --loop, or maximum planned tasks for --execute-plan.
        #[arg(long, default_value_t = 3)]
        max_iterations: usize,

        /// Reject the coding run when the estimated model-call budget exceeds this limit.
        #[arg(long)]
        max_estimated_model_calls: Option<usize>,

        /// Reject the coding run when the estimated tool-call budget exceeds this limit.
        #[arg(long)]
        max_estimated_tool_calls: Option<usize>,

        /// Optional tool provider config JSON.
        #[arg(long)]
        tool_config: Option<PathBuf>,
    },
    /// Inspect, fork, or truncate an AIR code-agent session file.
    #[command(hide = true)]
    CodeSession {
        /// Path to an AIR code-agent session JSON file.
        session: PathBuf,

        /// Write the resulting session to this new file.
        #[arg(long)]
        fork: Option<PathBuf>,

        /// Keep turns through this turn id or 1-based turn number.
        #[arg(long)]
        revert_to: Option<String>,

        /// Validate or apply reverse patches from this turn's indexed patch_sets.
        #[arg(long)]
        revert_workspace_turn: Option<String>,

        /// Apply --revert-workspace-turn after a successful reverse-patch check.
        #[arg(long, requires = "revert_workspace_turn")]
        apply_workspace: bool,

        /// Allow writing the reverted session back to the source file.
        #[arg(long, conflicts_with = "fork")]
        in_place: bool,
    },
    /// Parse and statically verify an AIR module.
    #[command(hide = true)]
    Validate {
        /// Path to a .air.yaml, .air.yml, or .air.json file.
        file: PathBuf,
    },
    /// Parse and statically verify an AIR system.
    #[command(hide = true)]
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
    Plan {
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
    /// Run a state-machine AIR module with built-in mock providers.
    #[command(hide = true)]
    Run {
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
    #[command(hide = true)]
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
    #[command(hide = true)]
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
    },
    /// Lower an AIR module to a backend target.
    #[command(hide = true)]
    Lower {
        /// Path to a .air.yaml, .air.yml, or .air.json file.
        file: PathBuf,

        /// Backend target to generate.
        #[arg(long, value_enum)]
        backend: LowerBackend,

        /// Optional output path. Prints to stdout when omitted.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Lower a dynamic AIR run plan to a backend target.
    LowerPlan {
        /// Path to a .air-plan.yaml file.
        plan: PathBuf,

        /// Path to a .air-store.yaml file.
        #[arg(long)]
        store: PathBuf,

        /// Backend target to generate.
        #[arg(long, value_enum)]
        backend: LowerPlanBackend,

        /// Optional output path. Prints to stdout when omitted.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LowerBackend {
    Langgraph,
    OpenaiAgentsJsSemantic,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LowerPlanBackend {
    Langgraph,
    OpenaiJsStrict,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Code {
            task,
            recipe,
            target,
            test,
            query,
            related,
            search_query,
            repo_query,
            required_terms,
            output,
            brand,
            product,
            constraints,
            force_patch,
            pack,
            profile,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            state_out,
            checkpoint_out,
            jit_cache,
            session,
            parallel,
            log,
            explain,
            loop_enabled,
            execute_plan,
            max_iterations,
            max_estimated_model_calls,
            max_estimated_tool_calls,
            tool_config,
        } => code(CodeOptions {
            task,
            recipe,
            target,
            test,
            query,
            related,
            search_query,
            repo_query,
            required_terms,
            output,
            brand,
            product,
            constraints,
            force_patch,
            pack,
            profile,
            model_config,
            trace_out,
            trace_redact,
            trace_raw,
            state_out,
            checkpoint_out,
            jit_cache,
            session,
            parallel,
            log,
            explain,
            loop_enabled,
            execute_plan,
            max_iterations,
            max_estimated_model_calls,
            max_estimated_tool_calls,
            tool_config,
        }),
        Command::CodeSession {
            session,
            fork,
            revert_to,
            revert_workspace_turn,
            apply_workspace,
            in_place,
        } => code_session(CodeSessionOptions {
            session,
            fork,
            revert_to,
            revert_workspace_turn,
            apply_workspace,
            in_place,
        }),
        Command::Validate { file } => validate(file),
        Command::ValidateSystem { file } => validate_system(file),
        Command::ValidatePlan {
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
        Command::Plan {
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
        Command::Run {
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
        Command::RunSystem {
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
        Command::RunPlan {
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
        }),
        Command::ResumePlan {
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
        Command::Replay {
            trace,
            specialize_run_plan,
            store,
            output,
            identity_out,
        } => replay(ReplayOptions {
            trace,
            specialize_run_plan,
            store,
            output,
            identity_out,
        }),
        Command::Lower {
            file,
            backend,
            output,
        } => lower(file, backend, output),
        Command::LowerPlan {
            plan,
            store,
            backend,
            output,
        } => lower_plan(plan, store, backend, output),
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

    for diagnostic in &report.diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!("{severity}[{}]: {}", diagnostic.code, diagnostic.message);
    }

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

    for diagnostic in &report.diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!("{severity}[{}]: {}", diagnostic.code, diagnostic.message);
    }

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

    for diagnostic in &report.diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!("{severity}[{}]: {}", diagnostic.code, diagnostic.message);
    }

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
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
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
        let models = ModelProviderChoice::from_config_file(model_config)?;
        let mut vm = Vm { tools, models };
        if observe {
            let result = vm.run_with_observer(&module, inputs, |event| {
                observe_event(event, log, &mut observed_trace)
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
                observe_event(event, log, &mut observed_trace)
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
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
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
        let models = ModelProviderChoice::from_config_file(model_config)?;
        if observe {
            let result = air_linker::run_system_with_observer(
                &system,
                base_dir,
                inputs,
                tools,
                models,
                |event| observe_event(event, log, &mut observed_trace),
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
            |event| observe_event(event, log, &mut observed_trace),
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

fn lower(file: PathBuf, backend: LowerBackend, output: Option<PathBuf>) -> Result<()> {
    let module = air_parser::parse_air_file(&file)?;
    let report = air_verify::verify(&module);
    if !report.is_success() {
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        std::process::exit(1);
    }

    let generated = match backend {
        LowerBackend::Langgraph => air_backend_langgraph::lower_module(&module)?,
        LowerBackend::OpenaiAgentsJsSemantic => {
            air_backend_openai_agents_js::lower_module(&module)?
        }
    };

    if let Some(output) = output {
        fs::write(output, generated)?;
    } else {
        print!("{generated}");
    }
    Ok(())
}

fn lower_plan(
    plan: PathBuf,
    store: PathBuf,
    backend: LowerPlanBackend,
    output: Option<PathBuf>,
) -> Result<()> {
    let plan = air_linker::parse_run_plan_file(plan)?;
    let store_path = store;
    let store = air_linker::parse_module_store_file(&store_path)?;
    let base_dir = module_base_dir_for_store_path(&store, &store_path);
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);
    if !report.is_success() {
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        std::process::exit(1);
    }

    let generated = match backend {
        LowerPlanBackend::Langgraph => {
            air_backend_langgraph::lower_run_plan(&plan, &store, base_dir)?
        }
        LowerPlanBackend::OpenaiJsStrict => {
            air_backend_openai_agents_js::lower_run_plan_strict(&plan, &store, base_dir)?
        }
    };

    if let Some(output) = output {
        fs::write(output, generated)?;
    } else {
        print!("{generated}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{module_catalog, parse_planner_response, planner_request, recipe_catalog};
    use crate::profile::read_json_object;
    use crate::run_plan::{
        run_plan_with_inputs, write_checkpoint_state, PlanStateFile, RunPlanExecutionOptions,
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
    fn planner_request_ranks_code_explore_for_open_ended_code_questions() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("examples/code-agent/module-store.air-store.yaml"),
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
            json!("code.explore@0.1.0")
        );
        assert_eq!(
            request["module_store"]["component_selection"]["first_choice"]["tier"],
            json!("large_component")
        );
    }

    #[test]
    fn planner_request_ranks_dynamic_code_explore_for_plan_act_observe_tasks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store = air_linker::parse_module_store_file(
            root.join("examples/code-agent/module-store.air-store.yaml"),
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
            json!("code.dynamic_explore@0.1.0")
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
            root.join("examples/code-agent/module-store.air-store.yaml"),
        )
        .unwrap();

        for (task, expected_id, expected_source) in [
            (
                "Plan a project-level task graph with milestones and acceptance criteria.",
                "code.project_plan_recipe@0.1.0",
                "recipe",
            ),
            (
                "Review the command_run implementation for safety, provenance, and diagnostics.",
                "code.review_with_std_context@0.1.0",
                "recipe",
            ),
            (
                "Fix a failing test using structured diagnostics, apply a bounded patch, and retest.",
                "code.core_repair@0.1.0",
                "recipe",
            ),
            (
                "Create a polished static landing page and verify it with browser screenshots.",
                "code.build_page@0.1.0",
                "module",
            ),
            (
                "Explore how command_run is implemented and identify relevant repository files.",
                "code.explore@0.1.0",
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
            root.join("examples/code-agent/module-store.air-store.yaml"),
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
    fn default_help_exposes_only_primary_plan_commands() {
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

        for command in [
            "code",
            "validate-plan",
            "plan",
            "run-plan",
            "resume-plan",
            "lower-plan",
        ] {
            assert!(
                command_names.contains(&command),
                "expected {command} in help"
            );
        }

        for command in [
            "validate-system",
            "run-system",
            "validate",
            "run",
            "code-session",
            "lower",
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
    fn code_command_accepts_minimal_typed_repair_input() {
        let cli = Cli::try_parse_from([
            "air",
            "code",
            "fix the failing add function and retest",
            "--target",
            "examples/code-agent/repair-fixture/math.js",
            "--test",
            "repair_fixture_test",
            "--related",
            "examples/code-agent/repair-fixture/test.js",
        ])
        .unwrap();

        let Command::Code {
            task,
            recipe,
            target,
            test,
            related,
            profile,
            explain,
            ..
        } = cli.command
        else {
            panic!("expected code command");
        };

        assert_eq!(task, "fix the failing add function and retest");
        assert_eq!(recipe, CodeRecipe::Auto);
        assert_eq!(
            target,
            Some(std::path::PathBuf::from(
                "examples/code-agent/repair-fixture/math.js"
            ))
        );
        assert_eq!(test, Some("repair_fixture_test".to_string()));
        assert_eq!(
            related,
            vec![std::path::PathBuf::from(
                "examples/code-agent/repair-fixture/test.js"
            )]
        );
        assert_eq!(profile, None);
        assert!(!explain);
    }

    #[test]
    fn code_command_accepts_build_recipe_input() {
        let cli = Cli::try_parse_from([
            "air",
            "code",
            "build a landing page",
            "--recipe",
            "build",
            "--output",
            "examples/apple-landing/index.html",
            "--brand",
            "Apple",
            "--product",
            "Apple Nova",
            "--constraint",
            "single HTML file",
        ])
        .unwrap();

        let Command::Code {
            recipe,
            output,
            brand,
            product,
            constraints,
            ..
        } = cli.command
        else {
            panic!("expected code command");
        };

        assert_eq!(recipe, CodeRecipe::Build);
        assert_eq!(
            output,
            Some(std::path::PathBuf::from(
                "examples/apple-landing/index.html"
            ))
        );
        assert_eq!(brand, Some("Apple".to_string()));
        assert_eq!(product, Some("Apple Nova".to_string()));
        assert_eq!(constraints, vec!["single HTML file".to_string()]);
    }

    #[test]
    fn code_command_accepts_review_recipe_input() {
        let cli = Cli::try_parse_from([
            "air",
            "code",
            "review the search tool",
            "--recipe",
            "review",
            "--target",
            "scripts/playwright_search.cjs",
            "--query",
            "playwright_search",
            "--search-query",
            "Playwright timeout behavior",
            "--required-term",
            "playwright",
        ])
        .unwrap();

        let Command::Code {
            recipe,
            target,
            query,
            search_query,
            required_terms,
            ..
        } = cli.command
        else {
            panic!("expected code command");
        };

        assert_eq!(recipe, CodeRecipe::Review);
        assert_eq!(
            target,
            Some(std::path::PathBuf::from("scripts/playwright_search.cjs"))
        );
        assert_eq!(query, Some("playwright_search".to_string()));
        assert_eq!(
            search_query,
            Some("Playwright timeout behavior".to_string())
        );
        assert_eq!(required_terms, vec!["playwright".to_string()]);
    }

    #[test]
    fn code_command_accepts_refactor_recipe_input() {
        let cli = Cli::try_parse_from([
            "air",
            "code",
            "refactor sum while keeping tests passing",
            "--recipe",
            "refactor",
            "--target",
            "examples/code-agent/refactor-fixture/math.js",
            "--test",
            "refactor_fixture_test",
            "--related",
            "examples/code-agent/refactor-fixture/test.js",
        ])
        .unwrap();

        let Command::Code {
            recipe,
            target,
            test,
            related,
            ..
        } = cli.command
        else {
            panic!("expected code command");
        };

        assert_eq!(recipe, CodeRecipe::Refactor);
        assert_eq!(
            target,
            Some(std::path::PathBuf::from(
                "examples/code-agent/refactor-fixture/math.js"
            ))
        );
        assert_eq!(test, Some("refactor_fixture_test".to_string()));
        assert_eq!(
            related,
            vec![std::path::PathBuf::from(
                "examples/code-agent/refactor-fixture/test.js"
            )]
        );
    }

    #[test]
    fn code_command_accepts_explain_flag() {
        let cli = Cli::try_parse_from([
            "air",
            "code",
            "fix the failing add function and retest",
            "--target",
            "examples/code-agent/repair-fixture/math.js",
            "--test",
            "repair_fixture_test",
            "--explain",
        ])
        .unwrap();

        let Command::Code {
            recipe, explain, ..
        } = cli.command
        else {
            panic!("expected code command");
        };

        assert_eq!(recipe, CodeRecipe::Auto);
        assert!(explain);
    }

    #[test]
    fn code_command_accepts_bounded_loop_flags() {
        let cli = Cli::try_parse_from([
            "air",
            "code",
            "fix the failing add function until tests pass",
            "--target",
            "examples/code-agent/repair-fixture/math.js",
            "--test",
            "repair_fixture_test",
            "--loop",
            "--max-iterations",
            "2",
        ])
        .unwrap();

        let Command::Code {
            loop_enabled,
            max_iterations,
            ..
        } = cli.command
        else {
            panic!("expected code command");
        };

        assert!(loop_enabled);
        assert_eq!(max_iterations, 2);
    }

    #[test]
    fn code_command_accepts_project_execution_flags() {
        let cli = Cli::try_parse_from([
            "air",
            "code",
            "plan and execute the next project milestone",
            "--recipe",
            "plan",
            "--execute-plan",
            "--max-iterations",
            "2",
            "--max-estimated-model-calls",
            "8",
            "--max-estimated-tool-calls",
            "24",
        ])
        .unwrap();

        let Command::Code {
            recipe,
            execute_plan,
            max_iterations,
            max_estimated_model_calls,
            max_estimated_tool_calls,
            ..
        } = cli.command
        else {
            panic!("expected code command");
        };

        assert_eq!(recipe, CodeRecipe::Plan);
        assert!(execute_plan);
        assert_eq!(max_iterations, 2);
        assert_eq!(max_estimated_model_calls, Some(8));
        assert_eq!(max_estimated_tool_calls, Some(24));
    }

    #[test]
    fn hidden_code_session_command_accepts_fork_and_revert() {
        let cli = Cli::try_parse_from([
            "air",
            "code-session",
            "target/generated/session.json",
            "--fork",
            "target/generated/session-fork.json",
            "--revert-to",
            "turn-000001",
        ])
        .unwrap();

        let Command::CodeSession {
            session,
            fork,
            revert_to,
            revert_workspace_turn,
            apply_workspace,
            in_place,
        } = cli.command
        else {
            panic!("expected code-session command");
        };

        assert_eq!(session, PathBuf::from("target/generated/session.json"));
        assert_eq!(
            fork,
            Some(PathBuf::from("target/generated/session-fork.json"))
        );
        assert_eq!(revert_to, Some("turn-000001".to_string()));
        assert_eq!(revert_workspace_turn, None);
        assert!(!apply_workspace);
        assert!(!in_place);
    }

    #[test]
    fn hidden_code_session_command_accepts_workspace_revert_dry_run() {
        let cli = Cli::try_parse_from([
            "air",
            "code-session",
            "target/generated/session.json",
            "--revert-workspace-turn",
            "turn-000001",
        ])
        .unwrap();

        let Command::CodeSession {
            revert_workspace_turn,
            apply_workspace,
            ..
        } = cli.command
        else {
            panic!("expected code-session command");
        };

        assert_eq!(revert_workspace_turn, Some("turn-000001".to_string()));
        assert!(!apply_workspace);
    }

    #[test]
    fn resume_plan_accepts_profile_without_positional_plan() {
        let cli = Cli::try_parse_from([
            "air",
            "resume-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
            "--state",
            "target/generated/deep_research_profile.state.json",
        ])
        .unwrap();

        let Command::ResumePlan {
            plan,
            profile,
            store,
            input,
            state,
            ..
        } = cli.command
        else {
            panic!("expected resume-plan command");
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
            "run-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
            "--parallel",
        ])
        .unwrap();

        let Command::RunPlan {
            plan,
            profile,
            parallel,
            ..
        } = cli.command
        else {
            panic!("expected run-plan command");
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
            },
        )
        .unwrap();

        replay(ReplayOptions {
            trace: trace_path.clone(),
            specialize_run_plan: true,
            store: Some(store_path.clone()),
            output: Some(output_path.clone()),
            identity_out: Some(identity_path.clone()),
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
    fn lower_plan_accepts_dynamic_run_plan_with_air_runtime() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store_path = temp_repo_file_path(&root, "air-dynamic-lower-store", "yaml");
        let output_path = store_path.with_extension("py");
        let store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();
        fs::write(&store_path, serde_yaml::to_string(&store).unwrap()).unwrap();

        lower_plan(
            root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
            store_path.clone(),
            LowerPlanBackend::Langgraph,
            Some(output_path.clone()),
        )
        .unwrap();
        let generated = fs::read_to_string(&output_path).unwrap();
        let _ = fs::remove_file(store_path);
        let _ = fs::remove_file(output_path);

        assert!(generated.contains("def _air_run_dynamic_after"));
        assert!(generated.contains("\"deep_research.research_topic@0.1.0\""));
    }

    #[test]
    fn specialized_dynamic_trace_can_lower_to_generated_backends() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let trace_path = temp_repo_file_path(&root, "air-dynamic-specialize", "jsonl");
        let store_path = trace_path.with_extension("store.yaml");
        let specialized_path = trace_path.with_extension("specialized.air-plan.yaml");
        let identity_path = trace_path.with_extension("identity.json");
        let langgraph_path = trace_path.with_extension("py");
        let openai_path = trace_path.with_extension("mjs");
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
            },
        )
        .unwrap();

        replay(ReplayOptions {
            trace: trace_path.clone(),
            specialize_run_plan: true,
            store: Some(store_path.clone()),
            output: Some(specialized_path.clone()),
            identity_out: Some(identity_path.clone()),
        })
        .unwrap();

        lower_plan(
            specialized_path.clone(),
            store_path.clone(),
            LowerPlanBackend::Langgraph,
            Some(langgraph_path.clone()),
        )
        .unwrap();
        lower_plan(
            specialized_path.clone(),
            store_path.clone(),
            LowerPlanBackend::OpenaiJsStrict,
            Some(openai_path.clone()),
        )
        .unwrap();

        let specialized = air_linker::parse_run_plan_file(&specialized_path).unwrap();
        let langgraph = fs::read_to_string(&langgraph_path).unwrap();
        let openai = fs::read_to_string(&openai_path).unwrap();
        let _ = fs::remove_file(trace_path);
        let _ = fs::remove_file(store_path);
        let _ = fs::remove_file(specialized_path);
        let _ = fs::remove_file(identity_path);
        let _ = fs::remove_file(langgraph_path);
        let _ = fs::remove_file(openai_path);

        assert!(specialized.dynamic.is_none());
        assert!(specialized.nodes.iter().any(|node| node.id == "topic_1"));
        assert!(langgraph.contains("\"topic_1\""));
        assert!(openai.contains("\"topic_1\""));
    }

    #[test]
    fn validate_plan_accepts_profile_without_positional_plan() {
        let cli = Cli::try_parse_from([
            "air",
            "validate-plan",
            "--profile",
            "examples/deep-research/profile.air-profile.yaml",
        ])
        .unwrap();

        let Command::ValidatePlan {
            plan,
            profile,
            store,
            explain,
        } = cli.command
        else {
            panic!("expected validate-plan command");
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
            "validate-plan",
            "tests/plans/approval-smoke.air-plan.yaml",
            "--store",
            "tests/plans/approval-smoke.air-store.yaml",
            "--explain",
        ])
        .unwrap();

        let Command::ValidatePlan { explain, .. } = cli.command else {
            panic!("expected validate-plan command");
        };

        assert!(explain);
    }

    #[test]
    fn plan_explain_accepts_no_model_config() {
        let cli = Cli::try_parse_from([
            "air",
            "plan",
            "--explain",
            "--store",
            "examples/code-agent/module-store.air-store.yaml",
            "--task",
            "Explore the command_run implementation",
        ])
        .unwrap();

        let Command::Plan {
            model_config,
            explain,
            ..
        } = cli.command
        else {
            panic!("expected plan command");
        };

        assert_eq!(model_config, None);
        assert!(explain);
    }

    #[test]
    fn plan_requires_model_config_without_explain() {
        let error = Cli::try_parse_from([
            "air",
            "plan",
            "--store",
            "examples/code-agent/module-store.air-store.yaml",
            "--task",
            "Explore the command_run implementation",
        ])
        .unwrap_err();

        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }
}
