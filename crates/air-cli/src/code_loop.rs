use crate::code_context::{recent_context_feedback, ContextFeedbackEntry};
use crate::code_input::{recipe_name, CodeRecipe};
use crate::code_pack::CodeAgentPackContext;
use crate::run_plan::{run_plan_capture, RunPlanOptions};
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub(crate) struct CodeLoopOptions {
    pub(crate) pack: CodeAgentPackContext,
    pub(crate) recipe: CodeRecipe,
    pub(crate) profile: PathBuf,
    pub(crate) input: Map<String, Value>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) state_out: Option<PathBuf>,
    pub(crate) checkpoint_out: Option<PathBuf>,
    pub(crate) jit_cache: Option<PathBuf>,
    pub(crate) parallel: bool,
    pub(crate) log: bool,
    pub(crate) tool_config: Option<PathBuf>,
    pub(crate) max_iterations: usize,
}

pub(crate) fn run_code_loop(options: CodeLoopOptions) -> Result<Value> {
    if options.max_iterations == 0 {
        bail!("air code --loop requires --max-iterations to be greater than 0");
    }

    let mut iterations = Vec::new();
    let mut final_outputs = Value::Object(Map::new());
    let mut completed = false;

    for iteration in 1..=options.max_iterations {
        if options.log {
            eprintln!(
                "[air-code-loop] iteration={} recipe={}",
                iteration,
                recipe_name(options.recipe)
            );
        }
        let iteration_input = code_loop_iteration_input(&options.input, &iterations);
        let outputs = run_plan_capture(RunPlanOptions {
            plan: None,
            profile: Some(options.profile.clone()),
            store: None,
            input: None,
            input_values: Some(iteration_input),
            model_config: options.model_config.clone(),
            trace_out: options
                .trace_out
                .as_ref()
                .map(|path| iteration_path(path, iteration)),
            trace_redact: options.trace_redact,
            trace_raw: options.trace_raw,
            state_out: options
                .state_out
                .as_ref()
                .map(|path| iteration_path(path, iteration)),
            checkpoint_out: options
                .checkpoint_out
                .as_ref()
                .map(|path| iteration_path(path, iteration)),
            jit_cache: options.jit_cache.clone(),
            parallel: options.parallel,
            log: options.log,
            example_tools: false,
            tool_config: options.tool_config.clone(),
        })?;
        completed = code_outputs_complete(&options.pack, options.recipe, &outputs)?;
        final_outputs = outputs.clone();
        iterations.push(json!({
            "iteration": iteration,
            "completed": completed,
            "outputs": outputs,
        }));
        if completed {
            break;
        }
    }

    let status = if completed {
        "completed"
    } else {
        "max_iterations_exhausted"
    };
    let summary = json!({
        "status": status,
        "recipe": recipe_name(options.recipe),
        "completed": completed,
        "iterations": iterations,
        "final_outputs": final_outputs,
    });
    Ok(summary)
}

pub(crate) fn code_loop_iteration_input(
    base_input: &Map<String, Value>,
    previous_iterations: &[Value],
) -> Map<String, Value> {
    if previous_iterations.is_empty() {
        return base_input.clone();
    }

    let mut input = base_input.clone();
    let Some(task) = input.get("task").and_then(Value::as_str) else {
        return input;
    };
    let feedback = code_loop_feedback(previous_iterations);
    input.insert(
        "task".to_string(),
        Value::String(format!(
            "{task}\n\nAIR loop context from previous iterations:\n{feedback}"
        )),
    );
    input
}

pub(crate) fn code_loop_feedback(previous_iterations: &[Value]) -> String {
    let entries = previous_iterations
        .iter()
        .map(|iteration| {
            let iteration_number = iteration
                .get("iteration")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let completed = iteration
                .get("completed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let outputs = iteration.get("outputs").cloned().unwrap_or(Value::Null);
            ContextFeedbackEntry {
                label: format!("iteration {iteration_number}"),
                summary: format!("completed={completed}"),
                output: serde_json::to_string(&outputs).unwrap_or_default(),
            }
        })
        .collect::<Vec<_>>();
    recent_context_feedback(&entries, "iteration")
}

pub(crate) fn iteration_path(path: &Path, iteration: usize) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("air-code-loop");
    let extension = path.extension().and_then(|value| value.to_str());
    let file_name = if let Some(extension) = extension {
        format!("{stem}.iter{iteration}.{extension}")
    } else {
        format!("{stem}.iter{iteration}")
    };
    parent.join(file_name)
}

pub(crate) fn code_outputs_complete(
    pack: &CodeAgentPackContext,
    recipe: CodeRecipe,
    outputs: &Value,
) -> Result<bool> {
    pack.recipe_complete(recipe_name(recipe), outputs)
}
