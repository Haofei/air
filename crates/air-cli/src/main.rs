use air_core::Severity;
use air_runtime::{
    read_trace_jsonl, replay_outputs, system_return_event, write_trace_jsonl,
    write_trace_jsonl_with_options, TraceEvent, TraceStatus, TraceWriteOptions, Vm,
};
mod models;
mod tools;
use crate::models::{call_openai_model, ModelProviderChoice};
use crate::tools::ToolProviderChoice;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "air")]
#[command(about = "AIR compiler CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
        #[arg(long)]
        model_config: PathBuf,

        /// Planner model alias from --model-config.
        #[arg(long, default_value = "planner")]
        planner_model: String,

        /// Include internal primitive modules in the planner catalog.
        #[arg(long)]
        allow_internal: bool,

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

        /// Redact sensitive fields and cap trace event size before writing --trace-out.
        #[arg(long)]
        trace_redact: bool,

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

        /// Redact sensitive fields and cap trace event size before writing --trace-out.
        #[arg(long)]
        trace_redact: bool,

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

        /// Redact sensitive fields and cap trace event size before writing --trace-out.
        #[arg(long)]
        trace_redact: bool,

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

        /// Redact sensitive fields and cap trace event size before writing --trace-out.
        #[arg(long)]
        trace_redact: bool,

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
        Command::Validate { file } => validate(file),
        Command::ValidateSystem { file } => validate_system(file),
        Command::ValidatePlan {
            plan,
            profile,
            store,
        } => validate_plan(ValidatePlanOptions {
            plan,
            profile,
            store,
        }),
        Command::Plan {
            task,
            task_file,
            store,
            model_config,
            planner_model,
            allow_internal,
            output,
        } => plan_task(PlanOptions {
            task,
            task_file,
            store,
            model_config,
            planner_model,
            allow_internal,
            output,
        }),
        Command::Run {
            file,
            input,
            model_config,
            trace_out,
            trace_redact,
            log,
            example_tools,
            tool_config,
        } => run(
            file,
            input,
            model_config,
            trace_out,
            trace_redact,
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
            log,
            example_tools,
            tool_config,
        } => run_system(
            file,
            input,
            model_config,
            trace_out,
            trace_redact,
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
            model_config,
            trace_out,
            trace_redact,
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
    let store = air_linker::parse_module_store_file(store)?;
    let base_dir = module_base_dir_for_store(&store)?;
    let report = air_linker::validate_run_plan(&plan, &store, base_dir);

    if report.diagnostics.is_empty() {
        println!("ok: {} {}", plan.plan.name, plan.plan.version);
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

struct PlanOptions {
    task: Option<String>,
    task_file: Option<PathBuf>,
    store: PathBuf,
    model_config: PathBuf,
    planner_model: String,
    allow_internal: bool,
    output: Option<PathBuf>,
}

struct ValidatePlanOptions {
    plan: Option<PathBuf>,
    profile: Option<PathBuf>,
    store: Option<PathBuf>,
}

fn plan_task(options: PlanOptions) -> Result<()> {
    let PlanOptions {
        task,
        task_file,
        store,
        model_config,
        planner_model,
        allow_internal,
        output,
    } = options;

    let task = match (task, task_file) {
        (Some(task), None) => task,
        (None, Some(path)) => fs::read_to_string(path)?,
        (None, None) => anyhow::bail!("one of --task or --task-file is required"),
        (Some(_), Some(_)) => anyhow::bail!("use either --task or --task-file, not both"),
    };

    let store = air_linker::parse_module_store_file(store)?;
    let base_dir = module_base_dir_for_store(&store)?;
    let catalog = module_catalog(&store, &base_dir, allow_internal)?;
    let recipes = recipe_catalog(&store, &base_dir, allow_internal)?;
    let request = planner_request(&task, &store, catalog, recipes, allow_internal);

    let plan_value = call_openai_model(model_config, &planner_model, &request)?;
    let mut plan = parse_planner_response(plan_value, &store, &base_dir, allow_internal)?;
    normalize_planner_run_plan(&mut plan);
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);
    if !report.is_success() {
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        eprintln!("generated run plan:\n{}", serde_yaml::to_string(&plan)?);
        anyhow::bail!("planner generated an invalid run plan");
    }

    let yaml = serde_yaml::to_string(&plan)?;
    if let Some(output) = output {
        fs::write(output, yaml)?;
    } else {
        print!("{yaml}");
    }

    Ok(())
}

fn planner_request(
    task: &str,
    store: &air_linker::ModuleStore,
    catalog: Vec<Value>,
    recipes: Vec<Value>,
    allow_internal: bool,
) -> Value {
    let component_selection = planner_component_selection(task, &catalog, &recipes);
    json!({
        "task": task.trim(),
        "module_store": {
            "name": &store.store.name,
            "version": &store.store.version,
            "catalog_policy": {
                "default_visibility": if allow_internal { "public_and_internal" } else { "public_only" },
                "selection_rule": "Follow component_selection.first_choice when it covers the task. Prefer recipes, then public composite modules, then bounded dynamic fan-out with reusable modules. Use primitive/internal modules only as an explicit fallback when no larger component satisfies the requested input/output contract."
            },
            "modules": catalog,
            "recipes": recipes,
            "component_selection": component_selection,
        },
        "instructions": planner_instructions(),
        "example_shape": {
            "plan": {"name": "task-run-plan", "version": "0.1.0"},
            "requires": {"capabilities": []},
            "nodes": [{"id": "extract", "module": "customer.extract@0.1.0"}],
            "entry": "extract",
            "edges": [],
            "connect": [{"from": "$input.text", "to": "extract.text"}],
            "outputs": {"result": "extract.extracted"},
            "halts": [],
            "schedule": {"max_parallel": 4, "groups": [{"id": "research_wave", "nodes": ["topic_1", "topic_2"], "max_parallel": 2}]},
            "decisions": [{"id": "choose-modules", "rationale": "why these modules were selected", "selected": ["extract"]}]
        },
        "example_dynamic_fanout_shape": {
            "nodes": [
                {"id": "plan", "module": "deep_research.plan_array@0.1.0"},
                {"id": "final", "module": "deep_research.final_report@0.1.0"}
            ],
            "requires": {"capabilities": ["network.search", "research.reflect"]},
            "entry": "plan",
            "edges": [{"from": "plan", "to": "final"}],
            "connect": [
                {"from": "$input.question", "to": "plan.question"},
                {"from": "$input.question", "to": "final.question"},
                {"from": "plan.plan", "to": "final.plan"}
            ],
            "dynamic": {
                "fanouts": [{
                    "id": "topic_wave",
                    "after": "plan",
                    "source": "plan.plan.topics",
                    "module": "deep_research.research_topic@0.1.0",
                    "id_prefix": "topic",
                    "min_items": 1,
                    "max_items": 4,
                    "max_parallel": 4,
                    "input": [
                        {"from": "plan.plan.research_brief", "to": "$each.research_brief"},
                        {"from": "$item", "to": "$each.topic"}
                    ],
                    "fan_in": [
                        {"from": "$each.note", "to": "final.notes[]"}
                    ]
                }]
            },
            "outputs": {"result": "final.final_report"}
        },
        "example_nested_dynamic_fanout_shape": {
            "nodes": [
                {"id": "plan", "module": "planner.with_topics@0.1.0"},
                {"id": "final", "module": "report.with_notes@0.1.0"}
            ],
            "requires": {"capabilities": ["network.search"]},
            "entry": "plan",
            "edges": [{"from": "plan", "to": "final"}],
            "dynamic": {
                "fanouts": [
                    {
                        "id": "parent_wave",
                        "after": "plan",
                        "source": "plan.topics",
                        "module": "research.parent@0.1.0",
                        "id_prefix": "parent",
                        "min_items": 1,
                        "max_items": 4,
                        "input": [
                            {"from": "$item", "to": "$each.topic"}
                        ],
                        "fan_in": [
                            {"from": "$each.note", "to": "final.notes[]"}
                        ]
                    },
                    {
                        "id": "child_wave",
                        "after": "parent_wave",
                        "source": "$parent.followups",
                        "module": "research.child@0.1.0",
                        "id_prefix": "child",
                        "min_items": 0,
                        "max_items": 3,
                        "input": [
                            {"from": "$item", "to": "$each.topic"}
                        ],
                        "fan_in": [
                            {"from": "$each.note", "to": "final.notes[]"}
                        ]
                    }
                ]
            },
            "outputs": {"result": "final.report"}
        },
        "example_recipe_selection": {
            "recipe_id": "deep_research.four_topic_report@0.1.0",
            "rationale": "The task asks for a bounded four-topic deep research report, which this recipe covers."
        },
        "example_repeated_module_shape": {
            "nodes": [
                {"id": "plan", "module": "deep_research.plan@0.1.0"},
                {"id": "topic_1", "module": "deep_research.research_topic@0.1.0"},
                {"id": "topic_2", "module": "deep_research.research_topic@0.1.0"},
                {"id": "final", "module": "deep_research.final_report@0.1.0"}
            ],
            "connect": [
                {"from": "$input.question", "to": "plan.question"},
                {"from": "plan.plan.research_brief", "to": "topic_1.research_brief"},
                {"from": "plan.plan.topic_1", "to": "topic_1.topic"},
                {"from": "topic_1.note", "to": "final.notes[]"},
                {"from": "topic_2.note", "to": "final.notes[]"}
            ]
        }
    })
}

fn planner_instructions() -> Vec<&'static str> {
    vec![
        "Return ONLY JSON.",
        "Read module_store.component_selection before module_store.modules. If component_selection.first_choice covers the task, select it.",
        "If module_store.recipes contains a recipe that covers the task, return a recipe selection object: {\"recipe_id\":\"...\",\"rationale\":\"...\"}. Prefer this over copying the full topology.",
        "If you reject component_selection.first_choice, include a decision with id reject-first-choice explaining the missing input/output or capability requirement.",
        "Otherwise return a full AIR RunPlan object with keys: plan, requires, nodes, entry, edges, connect, outputs, decisions. It may include halts when the plan should stop early on a typed condition.",
        "Set requires.capabilities to the union of all selected module requires.capabilities, including modules used by dynamic.fanouts.",
        "When independent fan-out nodes can run as a bounded parallel wave, include schedule.groups with a stable group id, node ids, and max_parallel.",
        "Use dynamic.fanouts when a module output is a typed array whose runtime length decides how many repeated module instances to create.",
        "For dynamic.fanouts, declare id, after, source, module, id_prefix, max_items, optional min_items, optional max_parallel, input, and fan_in.",
        "In dynamic.fanouts input mappings, use $item for the current source array item and $each.<input> for the materialized module input.",
        "In dynamic.fanouts fan_in mappings, use $each.<output> as the materialized module output and append to an array input such as final.notes[].",
        "Do not pre-list dynamic fan-out instances in nodes, edges, or connect; AIR materializes them after the declared after node.",
        "For constrained nested dynamic fan-out, set after to the parent fanout id and source to $parent.<array_output>; AIR expands one child wave per materialized parent node.",
        "Use static repeated nodes or a recipe when the exact fan-out cardinality is known before execution.",
        "If you return a full RunPlan from a recipe, preserve the recipe topology unless the task requires a smaller or clearly different bounded graph.",
        "Use module identifiers exactly as provided in module_store.modules[].id.",
        "Prefer composite modules with visibility=public and higher priority when they cover the task.",
        "Do not decompose a task into primitive modules when one public composite module covers the same task.",
        "When using primitive/internal modules as fallback, include a decision with id fallback-to-primitives and a rationale naming the missing larger component.",
        "You may instantiate the same module id multiple times with different node ids when the task needs repeated bounded work.",
        "Every node id must be unique and must be used consistently in edges, connect, outputs, and decisions.",
        "edges must contain node ids only, for example {\"from\":\"extract\",\"to\":\"score\"}.",
        "connect must contain endpoint ids, for example {\"from\":\"extract.extracted\",\"to\":\"score.extracted\"}.",
        "connect sources may read nested output object paths, for example {\"from\":\"plan.plan.topic_1\",\"to\":\"topic_1.topic\"}.",
        "connect targets may append to array inputs using [] when the target input schema is an array, for example {\"from\":\"topic_1.note\",\"to\":\"final.notes[]\"}.",
        "Every required module input listed in the catalog must be covered by exactly one direct connection or one or more [] append connections.",
        "Connect $input fields to every module input that is not provided by an upstream module output, for example connect $input.question to every downstream question input.",
        "Connect upstream module outputs to downstream inputs only when the names and schemas make sense.",
        "Set outputs to the final user-facing module output.",
        "Use halts for clarification or approval gates, for example {\"after\":\"clarify\",\"when\":{\"ref\":\"clarify.clarification.needs_clarification\",\"equals\":true},\"outputs\":{\"clarification\":\"clarify.clarification\"}}.",
        "Do not include markdown, comments, or explanatory text.",
    ]
}

fn planner_component_selection(task: &str, catalog: &[Value], recipes: &[Value]) -> Value {
    let task_terms = tokenize_for_component_match(task);
    let mut candidates = Vec::new();

    for recipe in recipes {
        let id = recipe.get("id").and_then(Value::as_str).unwrap_or_default();
        let priority = recipe
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let matched_terms = matched_component_terms(
            &task_terms,
            &[
                recipe.get("id"),
                recipe.get("description"),
                recipe.get("tags"),
                recipe.get("covers"),
            ],
        );
        let score = priority + 1_000 + (matched_terms.len() as i64 * 25);
        candidates.push(json!({
            "id": id,
            "source": "recipe",
            "tier": "large_component",
            "kind": "recipe",
            "priority": priority,
            "score": score,
            "matched_terms": matched_terms,
            "use_when": "Use directly when the task fits this verified topology; return {recipe_id, rationale} instead of copying nodes."
        }));
    }

    for module in catalog {
        let id = module.get("id").and_then(Value::as_str).unwrap_or_default();
        let kind = module
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("primitive");
        let visibility = module
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("public");
        let priority = module
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let matched_terms = matched_component_terms(
            &task_terms,
            &[
                module.get("id"),
                module.get("description"),
                module.get("tags"),
                module.get("covers"),
                module
                    .get("agent")
                    .and_then(|agent| agent.get("description")),
            ],
        );
        let tier = match (kind, visibility) {
            ("composite", "public") => "large_component",
            ("composite", _) => "internal_composite",
            ("primitive", "public") => "public_building_block",
            _ => "internal_building_block",
        };
        let tier_bonus = match tier {
            "large_component" => 500,
            "internal_composite" => 250,
            "public_building_block" => 50,
            _ => -100,
        };
        let score = priority + tier_bonus + (matched_terms.len() as i64 * 20);
        candidates.push(json!({
            "id": id,
            "source": "module",
            "tier": tier,
            "kind": kind,
            "visibility": visibility,
            "priority": priority,
            "score": score,
            "matched_terms": matched_terms,
            "use_when": match tier {
                "large_component" => "Prefer when its inputs/outputs cover the task.",
                "internal_composite" => "Use only when internal modules are allowed and no public recipe/composite covers the task.",
                "public_building_block" => "Use as a small component when no larger component covers the task.",
                _ => "Use only as explicit fallback glue inside a bounded plan.",
            }
        }));
    }

    candidates.sort_by(|left, right| {
        let left_score = left
            .get("score")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let right_score = right
            .get("score")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        right_score.cmp(&left_score).then_with(|| {
            left.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(right.get("id").and_then(Value::as_str).unwrap_or_default())
        })
    });

    let first_choice = candidates.first().cloned().unwrap_or(Value::Null);
    let recommended = candidates.into_iter().take(8).collect::<Vec<_>>();
    json!({
        "selection_ladder": [
            "1. recipe: pre-validated topology, select with recipe_id when it covers the task",
            "2. public composite module: one larger AIR module that covers the task",
            "3. bounded dynamic fan-out: planner/composite plus repeated reusable module when cardinality is runtime-known",
            "4. primitive/internal modules: fallback only, with explicit rationale"
        ],
        "first_choice": first_choice,
        "recommended": recommended,
        "fallback_requirement": "If using lower-tier components while a higher-tier candidate is present, explain the missing contract in decisions."
    })
}

fn tokenize_for_component_match(value: &str) -> BTreeSet<String> {
    value
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter_map(normalize_component_token)
        .collect()
}

fn matched_component_terms(
    task_terms: &BTreeSet<String>,
    values: &[Option<&Value>],
) -> Vec<String> {
    let mut matched = BTreeSet::new();
    for value in values.iter().flatten() {
        collect_matched_component_terms(task_terms, value, &mut matched);
    }
    matched.into_iter().collect()
}

fn collect_matched_component_terms(
    task_terms: &BTreeSet<String>,
    value: &Value,
    matched: &mut BTreeSet<String>,
) {
    match value {
        Value::String(text) => {
            for token in text
                .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .filter_map(normalize_component_token)
            {
                if task_terms.contains(&token) {
                    matched.insert(token);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_matched_component_terms(task_terms, value, matched);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_matched_component_terms(task_terms, value, matched);
            }
        }
        _ => {}
    }
}

fn normalize_component_token(value: &str) -> Option<String> {
    let token = value.trim().to_ascii_lowercase();
    if token.len() < 3 || COMPONENT_STOP_WORDS.contains(&token.as_str()) {
        None
    } else {
        Some(token)
    }
}

const COMPONENT_STOP_WORDS: &[&str] = &[
    "and", "are", "for", "from", "into", "one", "the", "this", "that", "with", "when",
];

fn module_catalog(
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    allow_internal: bool,
) -> Result<Vec<Value>> {
    let mut catalog = Vec::new();

    for (id, module_ref) in &store.modules {
        if !allow_internal && module_ref.visibility == air_linker::ModuleVisibility::Internal {
            continue;
        }

        let path = air_linker::resolve_module_path(base_dir, id, &module_ref.path)?;
        let module = air_parser::parse_air_file(&path)?;
        let report = air_verify::verify(&module);
        if !report.is_success() {
            anyhow::bail!(
                "store module {id} failed verification: {:?}",
                report.diagnostics
            );
        }

        catalog.push(json!({
            "id": id,
            "kind": module_ref.kind,
            "visibility": module_ref.visibility,
            "description": module_ref.description,
            "tags": module_ref.tags,
            "covers": module_ref.covers,
            "priority": module_ref.priority,
            "path": module_ref.path,
            "agent": {
                "name": module.agent.name,
                "version": module.agent.version,
                "description": module.agent.description,
            },
            "inputs": module.inputs,
            "outputs": module.outputs,
            "requires": module.requires,
            "tools": module.tools,
        }));
    }

    catalog.sort_by(|left, right| {
        let left_priority = left
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let right_priority = right
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        right_priority.cmp(&left_priority).then_with(|| {
            left.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(right.get("id").and_then(Value::as_str).unwrap_or_default())
        })
    });

    Ok(catalog)
}

fn module_base_dir_for_store(store: &air_linker::ModuleStore) -> Result<PathBuf> {
    module_base_dir_for_modules(&store.modules)
}

fn module_base_dir_for_modules(
    modules: &BTreeMap<String, air_linker::ModuleRef>,
) -> Result<PathBuf> {
    let current_dir = std::env::current_dir()?;
    Ok(infer_module_base_dir(&current_dir, modules))
}

fn infer_module_base_dir(
    start_dir: &Path,
    modules: &BTreeMap<String, air_linker::ModuleRef>,
) -> PathBuf {
    if modules.values().all(|module| module.path.is_absolute()) {
        return start_dir.to_path_buf();
    }

    start_dir
        .ancestors()
        .find(|candidate| {
            modules
                .values()
                .filter(|module| !module.path.is_absolute())
                .all(|module| candidate.join(&module.path).exists())
        })
        .unwrap_or(start_dir)
        .to_path_buf()
}

fn recipe_catalog(
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    allow_internal: bool,
) -> Result<Vec<Value>> {
    let mut recipes = Vec::new();

    for recipe in &store.recipes {
        if !allow_internal && recipe_uses_internal_modules(recipe, store) {
            continue;
        }

        let report = air_linker::validate_run_plan(&recipe.plan, store, base_dir);
        if !report.is_success() {
            anyhow::bail!(
                "store recipe {} failed verification: {:?}",
                recipe.id,
                report.diagnostics
            );
        }

        recipes.push(json!({
            "id": recipe.id,
            "description": recipe.description,
            "tags": recipe.tags,
            "covers": recipe.covers,
            "priority": recipe.priority,
            "topology": {
                "nodes": recipe.plan.nodes.iter().map(|node| {
                    json!({
                        "id": node.id,
                        "module": node.module,
                        "when": node.when,
                    })
                }).collect::<Vec<_>>(),
                "entry": &recipe.plan.entry,
                "edges": recipe.plan.edges.iter().map(|edge| {
                    format!("{} -> {}", edge.from, edge.to)
                }).collect::<Vec<_>>(),
                "connect": recipe.plan.connect.iter().map(|connection| {
                    format!("{} -> {}", display_connection_source(connection), connection.to)
                }).collect::<Vec<_>>(),
                "outputs": &recipe.plan.outputs,
                "requires": &recipe.plan.requires,
            },
        }));
    }

    recipes.sort_by(|left, right| {
        let left_priority = left
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let right_priority = right
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        right_priority.cmp(&left_priority).then_with(|| {
            left.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(right.get("id").and_then(Value::as_str).unwrap_or_default())
        })
    });

    Ok(recipes)
}

fn parse_planner_response(
    value: Value,
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    allow_internal: bool,
) -> Result<air_linker::RunPlan> {
    let value = planner_json_value(value)?;
    if let Ok(plan) = serde_json::from_value::<air_linker::RunPlan>(value.clone()) {
        return Ok(plan);
    }

    if let Some(recipe_id) = value
        .get("recipe_id")
        .or_else(|| value.get("recipe"))
        .and_then(Value::as_str)
    {
        return materialize_recipe_selection(recipe_id, &value, store, base_dir, allow_internal);
    }

    anyhow::bail!("planner response was neither a RunPlan nor a recipe selection object")
}

fn planner_json_value(value: Value) -> Result<Value> {
    let Some(content) = value.get("content").and_then(Value::as_str) else {
        return Ok(value);
    };
    let content = strip_json_fence(content);
    Ok(serde_json::from_str(content)?)
}

fn materialize_recipe_selection(
    recipe_id: &str,
    selection: &Value,
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    allow_internal: bool,
) -> Result<air_linker::RunPlan> {
    let recipe = store
        .recipes
        .iter()
        .find(|recipe| recipe.id == recipe_id)
        .ok_or_else(|| anyhow::anyhow!("planner selected unknown recipe {recipe_id}"))?;

    if !allow_internal && recipe_uses_internal_modules(recipe, store) {
        anyhow::bail!("planner selected internal recipe {recipe_id} without --allow-internal");
    }

    let report = air_linker::validate_run_plan(&recipe.plan, store, base_dir);
    if !report.is_success() {
        anyhow::bail!(
            "planner selected invalid recipe {recipe_id}: {:?}",
            report.diagnostics
        );
    }

    let mut plan = recipe.plan.clone();
    let rationale = selection
        .get("rationale")
        .and_then(Value::as_str)
        .unwrap_or("Planner selected a module-store recipe.");
    plan.decisions.insert(
        0,
        air_linker::PlanDecision {
            id: "select-recipe".to_string(),
            rationale: format!("Selected recipe {recipe_id}: {rationale}"),
            selected: vec![recipe_id.to_string()],
        },
    );
    Ok(plan)
}

fn recipe_uses_internal_modules(
    recipe: &air_linker::PlanRecipe,
    store: &air_linker::ModuleStore,
) -> bool {
    recipe.plan.nodes.iter().any(|node| {
        store.modules.get(&node.module).is_some_and(|module_ref| {
            module_ref.visibility == air_linker::ModuleVisibility::Internal
        })
    })
}

fn normalize_planner_run_plan(plan: &mut air_linker::RunPlan) {
    let mut normalized_edges = Vec::new();
    let mut inferred_connections = Vec::new();

    for edge in &plan.edges {
        let from_endpoint = endpoint_node(&edge.from);
        let to_endpoint = endpoint_node(&edge.to);
        if let (Some(from_node), Some(to_node)) = (from_endpoint, to_endpoint) {
            inferred_connections.push(air_linker::Connection {
                from: Some(edge.from.clone()),
                to: edge.to.clone(),
                value: None,
            });
            normalized_edges.push(air_linker::SystemEdge {
                from: from_node.to_string(),
                to: to_node.to_string(),
            });
        } else {
            normalized_edges.push(edge.clone());
        }
    }

    plan.edges = dedupe_edges(normalized_edges);
    plan.connect = dedupe_connections(
        plan.connect
            .iter()
            .cloned()
            .chain(inferred_connections)
            .collect(),
    );
}

fn endpoint_node(endpoint: &str) -> Option<&str> {
    let (node, field) = endpoint.split_once('.')?;
    (!node.is_empty() && !field.is_empty() && !field.contains('.')).then_some(node)
}

fn dedupe_edges(edges: Vec<air_linker::SystemEdge>) -> Vec<air_linker::SystemEdge> {
    let mut deduped = Vec::new();
    for edge in edges {
        if !deduped.iter().any(|existing: &air_linker::SystemEdge| {
            existing.from == edge.from && existing.to == edge.to
        }) {
            deduped.push(edge);
        }
    }
    deduped
}

fn dedupe_connections(connections: Vec<air_linker::Connection>) -> Vec<air_linker::Connection> {
    let mut deduped = Vec::new();
    for connection in connections {
        if !deduped.iter().any(|existing: &air_linker::Connection| {
            existing.from == connection.from
                && existing.value == connection.value
                && existing.to == connection.to
        }) {
            deduped.push(connection);
        }
    }
    deduped
}

fn display_connection_source(connection: &air_linker::Connection) -> String {
    if let Some(from) = &connection.from {
        return from.clone();
    }
    if let Some(value) = &connection.value {
        return serde_json::to_string(value).unwrap_or_else(|_| "<value>".to_string());
    }
    "<missing>".to_string()
}

fn strip_json_fence(content: &str) -> &str {
    let trimmed = content.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(close_index) = after_open.rfind("```") else {
        return trimmed;
    };
    let inner = &after_open[..close_index];
    let inner = inner.trim_start();
    let inner = inner
        .strip_prefix("json")
        .or_else(|| inner.strip_prefix("JSON"))
        .unwrap_or(inner);
    inner.trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{provider_error_snippet, search_docs, ConfigTools, LocalDoc};

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
        let store = air_linker::parse_module_store_file(
            root.join("examples/deep-research/module-store.air-store.yaml"),
        )
        .unwrap();

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
        let store_path = std::env::temp_dir().join(format!("{unique}.store.yaml"));

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
        let store_path = std::env::temp_dir().join(format!("{unique}.store.yaml"));
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
        let store_path = std::env::temp_dir().join(format!(
            "air-parallel-cli-store-{}-{}.yaml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
        let trace_path = std::env::temp_dir().join(format!(
            "air-specialize-cli-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
        let store_path = std::env::temp_dir().join(format!(
            "air-dynamic-lower-store-{}-{}.yaml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
        let trace_path = std::env::temp_dir().join(format!(
            "air-dynamic-specialize-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
        let config = air_backend_openai::parse_config_file(model_config)?;
        let mut vm = Vm {
            tools,
            models: ModelProviderChoice::openai(config)?,
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
        let config = air_backend_openai::parse_config_file(model_config)?;
        if observe {
            let result = air_linker::run_system_with_observer(
                &system,
                base_dir,
                inputs,
                tools,
                ModelProviderChoice::openai(config)?,
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
                ModelProviderChoice::openai(config)?,
            )?
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

struct RunPlanOptions {
    plan: Option<PathBuf>,
    profile: Option<PathBuf>,
    store: Option<PathBuf>,
    input: Option<PathBuf>,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    state_out: Option<PathBuf>,
    checkpoint_out: Option<PathBuf>,
    jit_cache: Option<PathBuf>,
    parallel: bool,
    log: bool,
    example_tools: bool,
    tool_config: Option<PathBuf>,
}

struct ResumePlanOptions {
    plan: Option<PathBuf>,
    profile: Option<PathBuf>,
    store: Option<PathBuf>,
    input: Option<PathBuf>,
    state: PathBuf,
    overrides: Vec<String>,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    state_out: Option<PathBuf>,
    checkpoint_out: Option<PathBuf>,
    log: bool,
    example_tools: bool,
    tool_config: Option<PathBuf>,
}

struct RunPlanExecutionOptions {
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    state_out: Option<PathBuf>,
    checkpoint_out: Option<PathBuf>,
    jit_cache: Option<PathBuf>,
    parallel: bool,
    log: bool,
    example_tools: bool,
    tool_config: Option<PathBuf>,
}

struct ReplayOptions {
    trace: PathBuf,
    specialize_run_plan: bool,
    store: Option<PathBuf>,
    output: Option<PathBuf>,
    identity_out: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct RunPlanProfile {
    plan: PathBuf,
    store: PathBuf,

    #[serde(default)]
    input_file: Option<PathBuf>,

    #[serde(default)]
    inputs: Option<BTreeMap<String, Value>>,

    #[serde(default)]
    model_config: Option<PathBuf>,

    #[serde(default)]
    tool_config: Option<PathBuf>,

    #[serde(default)]
    trace_out: Option<PathBuf>,

    #[serde(default)]
    trace_redact: Option<bool>,

    #[serde(default)]
    state_out: Option<PathBuf>,

    #[serde(default)]
    checkpoint_out: Option<PathBuf>,

    #[serde(default)]
    jit_cache: Option<PathBuf>,

    #[serde(default)]
    parallel: Option<bool>,

    #[serde(default)]
    log: Option<bool>,

    #[serde(default)]
    example_tools: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlanStateFile {
    status: air_linker::RunStatus,
    outputs: Value,
    module_outputs: BTreeMap<String, Value>,
}

fn run_plan(options: RunPlanOptions) -> Result<()> {
    let RunPlanOptions {
        plan,
        profile,
        store,
        input,
        model_config,
        trace_out,
        trace_redact,
        state_out,
        checkpoint_out,
        jit_cache,
        parallel,
        log,
        example_tools,
        tool_config,
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
        .ok_or_else(|| anyhow::anyhow!("run-plan requires a plan path or --profile"))?;
    let store = store
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.store))
        })
        .ok_or_else(|| anyhow::anyhow!("run-plan requires --store or --profile"))?;
    let inputs = if let Some(input) = input {
        read_json_object(input, "--input")?
    } else if let Some((path, profile)) = &profile {
        if let Some(input_file) = &profile.input_file {
            read_json_object(resolve_profile_path(path, input_file), "profile.input_file")?
        } else if let Some(inputs) = &profile.inputs {
            serde_json::Map::from_iter(inputs.clone())
        } else {
            anyhow::bail!("run-plan requires --input, profile.input_file, or profile.inputs");
        }
    } else {
        anyhow::bail!("run-plan requires --input or --profile");
    };
    let model_config = model_config.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .model_config
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let tool_config = tool_config.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .tool_config
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let trace_out = trace_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .trace_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let state_out = state_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .state_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let checkpoint_out = checkpoint_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .checkpoint_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let jit_cache = jit_cache.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .jit_cache
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let parallel = parallel
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.parallel)
            .unwrap_or(false);
    let log = log
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.log)
            .unwrap_or(false);
    let trace_redact = trace_redact
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.trace_redact)
            .unwrap_or(false);
    let example_tools = example_tools
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.example_tools)
            .unwrap_or(false);

    run_plan_with_inputs(
        plan,
        store,
        inputs,
        RunPlanExecutionOptions {
            model_config,
            trace_out,
            trace_redact,
            state_out,
            checkpoint_out,
            jit_cache,
            parallel,
            log,
            example_tools,
            tool_config,
        },
    )
}

fn run_plan_with_inputs(
    plan: PathBuf,
    store: PathBuf,
    inputs: serde_json::Map<String, Value>,
    options: RunPlanExecutionOptions,
) -> Result<()> {
    let RunPlanExecutionOptions {
        model_config,
        trace_out,
        trace_redact,
        state_out,
        checkpoint_out,
        jit_cache,
        parallel,
        log,
        example_tools,
        tool_config,
    } = options;

    let original_plan = air_linker::parse_run_plan_file(plan)?;
    let store = air_linker::parse_module_store_file(store)?;
    let base_dir = module_base_dir_for_store(&store)?;
    let (plan, jit_context) = if let Some(jit_cache) = jit_cache.as_ref() {
        select_jit_cached_run_plan(original_plan, &store, &base_dir, &inputs, jit_cache, log)?
    } else {
        (original_plan, None)
    };
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);
    if !report.is_success() {
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        std::process::exit(1);
    }

    let observe = log || trace_out.is_some();
    let mut observed_trace = Vec::new();
    let tools = ToolProviderChoice::from_config(tool_config, example_tools)?;
    let result = if parallel {
        if let Some(model_config) = model_config {
            let config = air_backend_openai::parse_config_file(model_config)?;
            let models = ModelProviderChoice::openai(config)?;
            run_plan_parallel(
                &plan,
                &store,
                base_dir.clone(),
                inputs,
                tools,
                models,
                observe,
                log,
                trace_out.as_ref(),
                trace_redact,
                checkpoint_out.as_ref(),
                &mut observed_trace,
            )?
        } else {
            run_plan_parallel(
                &plan,
                &store,
                base_dir.clone(),
                inputs,
                tools,
                ModelProviderChoice::echo(),
                observe,
                log,
                trace_out.as_ref(),
                trace_redact,
                checkpoint_out.as_ref(),
                &mut observed_trace,
            )?
        }
    } else if let Some(model_config) = model_config {
        let config = air_backend_openai::parse_config_file(model_config)?;
        if observe {
            let result = air_linker::run_run_plan_with_observer_and_checkpoint(
                &plan,
                &store,
                base_dir.clone(),
                inputs,
                tools,
                ModelProviderChoice::openai(config)?,
                |event| observe_event(event, log, &mut observed_trace),
                |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
            );
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            air_linker::run_run_plan_with_observer_and_checkpoint(
                &plan,
                &store,
                base_dir.clone(),
                inputs,
                tools,
                ModelProviderChoice::openai(config)?,
                |_| {},
                |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
            )?
        }
    } else if observe {
        let result = air_linker::run_run_plan_with_observer_and_checkpoint(
            &plan,
            &store,
            base_dir.clone(),
            inputs,
            tools,
            ModelProviderChoice::echo(),
            |event| observe_event(event, log, &mut observed_trace),
            |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
        );
        if result.is_err() {
            write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
        }
        result?
    } else {
        air_linker::run_run_plan_with_observer_and_checkpoint(
            &plan,
            &store,
            base_dir.clone(),
            inputs,
            tools,
            ModelProviderChoice::echo(),
            |_| {},
            |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
        )?
    };
    if let Some(trace_out) = trace_out {
        let mut trace = result.trace.clone();
        trace.push(system_return_event(result.outputs.clone()));
        write_trace(trace_out, &trace, trace_redact)?;
    }
    if let Some(state_out) = state_out {
        write_plan_state(state_out, &result)?;
    }
    if let Some(jit_context) = jit_context.as_ref() {
        if !jit_context.hit {
            write_jit_cache(jit_context, &result.trace, &store, &base_dir, log)?;
        }
    }
    println!("{}", serde_json::to_string_pretty(&result.outputs)?);

    Ok(())
}

#[derive(Debug, Clone)]
struct JitCacheContext {
    dir: PathBuf,
    key: String,
    hit: bool,
}

fn select_jit_cached_run_plan(
    original_plan: air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    inputs: &serde_json::Map<String, Value>,
    cache_dir: &std::path::Path,
    log: bool,
) -> Result<(air_linker::RunPlan, Option<JitCacheContext>)> {
    fs::create_dir_all(cache_dir)?;
    let key = jit_cache_key(&original_plan, store, base_dir, inputs)?;
    let context = JitCacheContext {
        dir: cache_dir.to_path_buf(),
        key,
        hit: false,
    };
    let plan_path = jit_cache_plan_path(&context);
    if plan_path.exists() {
        match air_linker::parse_run_plan_file(&plan_path) {
            Ok(cached_plan) => {
                let report = air_linker::validate_run_plan(&cached_plan, store, base_dir);
                if report.is_success() {
                    if log {
                        eprintln!(
                            "[air-jit] cache hit key={} plan={}",
                            context.key,
                            plan_path.display()
                        );
                    }
                    return Ok((
                        cached_plan,
                        Some(JitCacheContext {
                            hit: true,
                            ..context
                        }),
                    ));
                }
                if log {
                    eprintln!(
                        "[air-jit] ignoring invalid cache key={} diagnostics={:?}",
                        context.key, report.diagnostics
                    );
                }
            }
            Err(error) => {
                if log {
                    eprintln!(
                        "[air-jit] ignoring unreadable cache key={} error={error}",
                        context.key
                    );
                }
            }
        }
    } else if log {
        eprintln!("[air-jit] cache miss key={}", context.key);
    }

    Ok((original_plan, Some(context)))
}

fn write_jit_cache(
    context: &JitCacheContext,
    trace: &[TraceEvent],
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    log: bool,
) -> Result<()> {
    let specialization = air_linker::specialize_run_plan_trace(trace, store, base_dir)?;
    let plan_path = jit_cache_plan_path(context);
    let identity_path = context.dir.join(format!("{}.identity.json", context.key));
    let meta_path = context.dir.join(format!("{}.meta.json", context.key));
    fs::create_dir_all(&context.dir)?;
    fs::write(&plan_path, serde_yaml::to_string(&specialization.plan)?)?;
    fs::write(
        &identity_path,
        serde_json::to_string_pretty(&specialization.cache_identity)?,
    )?;
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&json!({
            "kind": "air.jit_cache_entry.v1",
            "key": context.key,
            "plan": specialization.plan.plan,
            "identity_path": identity_path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        }))?,
    )?;
    if log {
        eprintln!(
            "[air-jit] wrote cache key={} plan={}",
            context.key,
            plan_path.display()
        );
    }
    Ok(())
}

fn jit_cache_plan_path(context: &JitCacheContext) -> PathBuf {
    context.dir.join(format!("{}.air-plan.yaml", context.key))
}

fn jit_cache_key(
    plan: &air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    inputs: &serde_json::Map<String, Value>,
) -> Result<String> {
    let modules = store
        .modules
        .iter()
        .map(|(id, module_ref)| {
            let path = air_linker::resolve_module_path(base_dir, id, &module_ref.path)?;
            let content = fs::read_to_string(&path)?;
            Ok((
                id.clone(),
                json!({
                    "ref": module_ref,
                    "content": content,
                }),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let identity = json!({
        "kind": "air.jit_cache_key.v1",
        "plan": plan,
        "store": {
            "metadata": &store.store,
            "modules": modules,
            "recipes": &store.recipes,
        },
        "inputs": inputs,
    });
    let encoded = serde_json::to_string(&identity)?;
    let mut hasher = DefaultHasher::new();
    encoded.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

#[allow(clippy::too_many_arguments)]
fn run_plan_parallel(
    plan: &air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: PathBuf,
    inputs: serde_json::Map<String, Value>,
    tools: ToolProviderChoice,
    models: ModelProviderChoice,
    observe: bool,
    log: bool,
    trace_out: Option<&PathBuf>,
    trace_redact: bool,
    checkpoint_out: Option<&PathBuf>,
    observed_trace: &mut Vec<TraceEvent>,
) -> Result<air_linker::SystemRunResult> {
    if observe {
        let result = air_linker::run_run_plan_parallel_with_observer_and_checkpoint(
            plan,
            store,
            base_dir,
            inputs,
            || tools.clone(),
            || models.clone(),
            |event| observe_event(event, log, observed_trace),
            |checkpoint| write_checkpoint_state(checkpoint_out, checkpoint),
        );
        if result.is_err() {
            write_partial_trace(trace_out, observed_trace, trace_redact)?;
        }
        Ok(result?)
    } else {
        Ok(
            air_linker::run_run_plan_parallel_with_observer_and_checkpoint(
                plan,
                store,
                base_dir,
                inputs,
                || tools.clone(),
                || models.clone(),
                |_| {},
                |checkpoint| write_checkpoint_state(checkpoint_out, checkpoint),
            )?,
        )
    }
}

fn resume_plan(options: ResumePlanOptions) -> Result<()> {
    let ResumePlanOptions {
        plan,
        profile,
        store,
        input,
        state,
        overrides,
        model_config,
        trace_out,
        trace_redact,
        state_out,
        checkpoint_out,
        log,
        example_tools,
        tool_config,
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
        .ok_or_else(|| anyhow::anyhow!("resume-plan requires a plan path or --profile"))?;
    let store = store
        .or_else(|| {
            profile
                .as_ref()
                .map(|(path, profile)| resolve_profile_path(path, &profile.store))
        })
        .ok_or_else(|| anyhow::anyhow!("resume-plan requires --store or --profile"))?;
    let input = input.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .input_file
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let model_config = model_config.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .model_config
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let tool_config = tool_config.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .tool_config
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let trace_out = trace_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .trace_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let state_out = state_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .state_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let checkpoint_out = checkpoint_out.or_else(|| {
        profile.as_ref().and_then(|(path, profile)| {
            profile
                .checkpoint_out
                .as_ref()
                .map(|value| resolve_profile_path(path, value))
        })
    });
    let log = log
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.log)
            .unwrap_or(false);
    let trace_redact = trace_redact
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.trace_redact)
            .unwrap_or(false);
    let example_tools = example_tools
        || profile
            .as_ref()
            .and_then(|(_, profile)| profile.example_tools)
            .unwrap_or(false);

    let plan = air_linker::parse_run_plan_file(plan)?;
    let store = air_linker::parse_module_store_file(store)?;
    let base_dir = module_base_dir_for_store(&store)?;
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);
    if !report.is_success() {
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        std::process::exit(1);
    }

    let inputs = if let Some(input) = input {
        read_json_object(input, "--input")?
    } else if let Some((_, profile)) = &profile {
        if let Some(inputs) = &profile.inputs {
            serde_json::Map::from_iter(inputs.clone())
        } else {
            anyhow::bail!("resume-plan requires --input, profile.input_file, or profile.inputs");
        }
    } else {
        anyhow::bail!("resume-plan requires --input or --profile");
    };
    let state_file: PlanStateFile = serde_json::from_str(&fs::read_to_string(state)?)?;
    let mut resume = air_linker::ResumeState {
        module_outputs: state_file
            .module_outputs
            .into_iter()
            .map(|(module, value)| {
                let Value::Object(output) = value else {
                    anyhow::bail!("state module output {module} must be a JSON object");
                };
                Ok((module, output))
            })
            .collect::<Result<_>>()?,
        output_overrides: BTreeMap::new(),
    };
    for override_spec in overrides {
        let (endpoint, value) = parse_output_override(&override_spec)?;
        resume.output_overrides.insert(endpoint, value);
    }

    let observe = log || trace_out.is_some();
    let mut observed_trace = Vec::new();
    let tools = ToolProviderChoice::from_config(tool_config, example_tools)?;
    let result = if let Some(model_config) = model_config {
        let config = air_backend_openai::parse_config_file(model_config)?;
        if observe {
            let result = air_linker::resume_run_plan_with_observer_and_checkpoint(
                &plan,
                &store,
                base_dir,
                inputs,
                resume,
                tools,
                ModelProviderChoice::openai(config)?,
                |event| observe_event(event, log, &mut observed_trace),
                |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
            );
            if result.is_err() {
                write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
            }
            result?
        } else {
            air_linker::resume_run_plan_with_observer_and_checkpoint(
                &plan,
                &store,
                base_dir,
                inputs,
                resume,
                tools,
                ModelProviderChoice::openai(config)?,
                |_| {},
                |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
            )?
        }
    } else if observe {
        let result = air_linker::resume_run_plan_with_observer_and_checkpoint(
            &plan,
            &store,
            base_dir,
            inputs,
            resume,
            tools,
            ModelProviderChoice::echo(),
            |event| observe_event(event, log, &mut observed_trace),
            |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
        );
        if result.is_err() {
            write_partial_trace(trace_out.as_ref(), &observed_trace, trace_redact)?;
        }
        result?
    } else {
        air_linker::resume_run_plan_with_observer_and_checkpoint(
            &plan,
            &store,
            base_dir,
            inputs,
            resume,
            tools,
            ModelProviderChoice::echo(),
            |_| {},
            |checkpoint| write_checkpoint_state(checkpoint_out.as_ref(), checkpoint),
        )?
    };

    if let Some(trace_out) = trace_out {
        let mut trace = result.trace.clone();
        trace.push(system_return_event(result.outputs.clone()));
        write_trace(trace_out, &trace, trace_redact)?;
    }
    if let Some(state_out) = state_out {
        write_plan_state(state_out, &result)?;
    }
    println!("{}", serde_json::to_string_pretty(&result.outputs)?);

    Ok(())
}

fn read_json_object(path: PathBuf, label: &str) -> Result<serde_json::Map<String, Value>> {
    let input = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {label} JSON file {}", path.display()))?;
    let Value::Object(inputs) = serde_json::from_str::<Value>(&input)
        .with_context(|| format!("failed to parse {label} JSON file {}", path.display()))?
    else {
        anyhow::bail!(
            "{label} JSON file {} must contain a JSON object",
            path.display()
        );
    };
    Ok(inputs)
}

fn read_run_plan_profile(path: &PathBuf) -> Result<RunPlanProfile> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read run profile {}", path.display()))?;
    serde_yaml::from_str(&source)
        .with_context(|| format!("failed to parse run profile YAML {}", path.display()))
}

fn resolve_profile_path(profile_path: &std::path::Path, path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path.clone();
    }
    profile_path
        .parent()
        .map(|parent| parent.join(path))
        .unwrap_or_else(|| path.clone())
}

fn parse_output_override(spec: &str) -> Result<(String, Value)> {
    let Some((endpoint, raw_value)) = spec.split_once('=') else {
        anyhow::bail!("override must use endpoint=json syntax");
    };
    if endpoint.trim().is_empty() {
        anyhow::bail!("override endpoint must not be empty");
    }
    let value =
        serde_json::from_str(raw_value).unwrap_or_else(|_| Value::String(raw_value.to_string()));
    Ok((endpoint.to_string(), value))
}

fn write_plan_state(path: PathBuf, result: &air_linker::SystemRunResult) -> Result<()> {
    let state = PlanStateFile {
        status: result.status.clone(),
        outputs: Value::Object(result.outputs.clone()),
        module_outputs: result
            .module_outputs
            .iter()
            .map(|(module, outputs)| (module.clone(), Value::Object(outputs.clone())))
            .collect(),
    };
    fs::write(path, serde_json::to_string_pretty(&state)?)?;
    Ok(())
}

fn write_checkpoint_state(
    path: Option<&PathBuf>,
    checkpoint: &air_linker::SystemCheckpoint,
) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };

    let state = PlanStateFile {
        status: air_linker::RunStatus::InProgress {
            after: checkpoint.completed_module.clone(),
        },
        outputs: Value::Object(Default::default()),
        module_outputs: checkpoint
            .module_outputs
            .iter()
            .map(|(module, outputs)| (module.clone(), Value::Object(outputs.clone())))
            .collect(),
    };
    let content = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
    fs::write(path, content).map_err(|error| error.to_string())?;
    Ok(())
}

fn observe_event(event: &TraceEvent, log: bool, trace: &mut Vec<TraceEvent>) {
    if log {
        log_event(event);
    }
    trace.push(event.clone());
}

fn write_partial_trace(
    trace_out: Option<&PathBuf>,
    trace: &[TraceEvent],
    trace_redact: bool,
) -> Result<()> {
    if let Some(path) = trace_out {
        write_trace(path, trace, trace_redact)?;
    }
    Ok(())
}

fn write_trace(
    path: impl AsRef<std::path::Path>,
    trace: &[TraceEvent],
    redact: bool,
) -> Result<()> {
    if redact {
        write_trace_jsonl_with_options(path, trace, &TraceWriteOptions::redacted())?;
    } else {
        write_trace_jsonl(path, trace)?;
    }
    Ok(())
}

fn log_event(event: &TraceEvent) {
    let status = match event.status {
        TraceStatus::Ok => "ok",
        TraceStatus::Error => "error",
    };
    let input = event
        .input
        .as_ref()
        .map(value_shape)
        .map(|shape| format!(" input={shape}"))
        .unwrap_or_default();
    let output = event
        .output
        .as_ref()
        .map(value_shape)
        .map(|shape| format!(" output={shape}"))
        .unwrap_or_default();
    let meta = event
        .meta
        .as_ref()
        .map(value_shape)
        .map(|shape| format!(" meta={shape}"))
        .unwrap_or_default();
    let error = event
        .error
        .as_ref()
        .map(|error| format!(" error={error}"))
        .unwrap_or_default();

    eprintln!(
        "[air] {} step={} rule={} action={} status={}{}{}{}{}",
        event.agent, event.step, event.rule, event.action, status, meta, input, output, error
    );
}

fn value_shape(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut entries = object
                .iter()
                .take(8)
                .map(|(key, value)| format!("{key}:{}", compact_value(value)))
                .collect::<Vec<_>>();
            if object.len() > entries.len() {
                entries.push("...".to_string());
            }
            format!("{{{}}}", entries.join(","))
        }
        value => compact_value(value),
    }
}

fn compact_value(value: &Value) -> String {
    match value {
        Value::Object(object) => format!("object({})", object.len()),
        Value::Array(values) => format!("[{}]", values.len()),
        Value::String(value) => {
            let max = 80;
            let mut preview = value.chars().take(max).collect::<String>();
            if value.chars().count() > max {
                preview.push_str("...");
            }
            format!("{preview:?}")
        }
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_string(),
    }
}

fn replay(options: ReplayOptions) -> Result<()> {
    let ReplayOptions {
        trace,
        specialize_run_plan,
        store,
        output,
        identity_out,
    } = options;
    let events = read_trace_jsonl(trace)?;
    if specialize_run_plan {
        let store = store
            .ok_or_else(|| anyhow::anyhow!("replay --specialize-run-plan requires --store"))?;
        let store = air_linker::parse_module_store_file(store)?;
        let base_dir = module_base_dir_for_store(&store)?;
        let specialization = air_linker::specialize_run_plan_trace(&events, &store, &base_dir)?;
        let plan_yaml = serde_yaml::to_string(&specialization.plan)?;
        if let Some(output) = output {
            fs::write(output, plan_yaml)?;
        } else {
            print!("{plan_yaml}");
        }
        if let Some(identity_out) = identity_out {
            fs::write(
                identity_out,
                serde_json::to_string_pretty(&specialization.cache_identity)?,
            )?;
        }
    } else {
        let output = replay_outputs(&events)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
    }
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
    let store = air_linker::parse_module_store_file(store)?;
    let base_dir = module_base_dir_for_store(&store)?;
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
