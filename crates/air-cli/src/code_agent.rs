use crate::explain::build_plan_explanation;
use crate::planner::module_base_dir_for_store_path;
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use crate::run_plan::{run_plan, run_plan_capture, RunPlanOptions};
use anyhow::{bail, Result};
use clap::ValueEnum;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum CodeRecipe {
    /// Select a recipe from typed flags, preferring read-only exploration when ambiguous.
    Auto,
    /// Read-only repository exploration and planning.
    Explore,
    /// Grounded code review with repository and external evidence.
    Review,
    /// Core repair loop: explore, select context, patch, and retest.
    Repair,
    /// Build one bounded static page and verify it.
    Build,
}

pub(crate) struct CodeOptions {
    pub(crate) task: String,
    pub(crate) recipe: CodeRecipe,
    pub(crate) target: Option<PathBuf>,
    pub(crate) test: Option<String>,
    pub(crate) query: Option<String>,
    pub(crate) related: Vec<PathBuf>,
    pub(crate) search_query: Option<String>,
    pub(crate) repo_query: Option<String>,
    pub(crate) required_terms: Vec<String>,
    pub(crate) output: Option<PathBuf>,
    pub(crate) brand: Option<String>,
    pub(crate) product: Option<String>,
    pub(crate) constraints: Vec<String>,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) state_out: Option<PathBuf>,
    pub(crate) checkpoint_out: Option<PathBuf>,
    pub(crate) jit_cache: Option<PathBuf>,
    pub(crate) parallel: bool,
    pub(crate) log: bool,
    pub(crate) explain: bool,
    pub(crate) loop_enabled: bool,
    pub(crate) max_iterations: usize,
    pub(crate) tool_config: Option<PathBuf>,
}

pub(crate) fn code(options: CodeOptions) -> Result<()> {
    let CodeOptions {
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
        profile,
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        state_out,
        checkpoint_out,
        jit_cache,
        parallel,
        log,
        explain,
        loop_enabled,
        max_iterations,
        tool_config,
    } = options;

    let requested_recipe = recipe;
    let recipe = resolve_recipe(
        recipe,
        target.as_ref(),
        test.as_ref(),
        search_query.as_ref(),
        repo_query.as_ref(),
        &required_terms,
        output.as_ref(),
        brand.as_ref(),
        product.as_ref(),
        &constraints,
    );
    let profile = profile.unwrap_or_else(|| default_profile(recipe));
    let input = build_input(CodeInputOptions {
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
    })?;

    if explain {
        print_explain(
            requested_recipe,
            recipe,
            &profile,
            &input,
            loop_enabled,
            max_iterations,
        )?;
        return Ok(());
    }

    if loop_enabled {
        return run_code_loop(CodeLoopOptions {
            recipe,
            profile,
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
            tool_config,
            max_iterations,
        });
    }

    run_plan(RunPlanOptions {
        plan: None,
        profile: Some(profile),
        store: None,
        input: None,
        input_values: Some(input),
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        state_out,
        checkpoint_out,
        jit_cache,
        parallel,
        log,
        example_tools: false,
        tool_config,
    })
}

fn print_explain(
    requested_recipe: CodeRecipe,
    resolved_recipe: CodeRecipe,
    profile: &Path,
    input: &Map<String, Value>,
    loop_enabled: bool,
    max_iterations: usize,
) -> Result<()> {
    let metadata = explain_metadata_for_profile(profile)?;
    let budget_iterations = if loop_enabled { max_iterations } else { 1 };
    let explanation = json!({
        "command": "code",
        "will_run": false,
        "requested_recipe": recipe_name(requested_recipe),
        "resolved_recipe": recipe_name(resolved_recipe),
        "profile": path_ref_to_input_string(profile),
        "plan": path_ref_to_input_string(&metadata.plan),
        "store": path_ref_to_input_string(&metadata.store),
        "capabilities": metadata.capabilities,
        "read_only": metadata.read_only,
        "writes_workspace": metadata.writes_workspace,
        "budget": {
            "per_iteration": {
                "max_estimated_model_calls": metadata.max_estimated_model_calls,
                "max_estimated_tool_calls": metadata.max_estimated_tool_calls
            },
            "total": {
                "iterations": budget_iterations,
                "max_estimated_model_calls": metadata.max_estimated_model_calls.saturating_mul(budget_iterations),
                "max_estimated_tool_calls": metadata.max_estimated_tool_calls.saturating_mul(budget_iterations)
            }
        },
        "loop": {
            "enabled": loop_enabled,
            "max_iterations": max_iterations
        },
        "input": Value::Object(input.clone()),
    });
    serde_json::to_writer_pretty(std::io::stdout(), &explanation)?;
    println!();
    Ok(())
}

struct CodeExplainMetadata {
    plan: PathBuf,
    store: PathBuf,
    capabilities: Vec<String>,
    read_only: bool,
    writes_workspace: bool,
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
}

fn explain_metadata_for_profile(profile: &Path) -> Result<CodeExplainMetadata> {
    let profile_path = profile.to_path_buf();
    let profile = read_run_plan_profile(&profile_path)?;
    let plan_path = resolve_profile_path(&profile_path, &profile.plan);
    let store_path = resolve_profile_path(&profile_path, &profile.store);
    let plan = air_linker::parse_run_plan_file(&plan_path)?;
    let store = air_linker::parse_module_store_file(&store_path)?;
    let base_dir = module_base_dir_for_store_path(&store, &store_path);
    let plan_explanation = build_plan_explanation(&plan, &store, &base_dir)?;
    let mut capabilities = plan.requires.capabilities;
    capabilities.sort();
    capabilities.dedup();
    let writes_workspace = capabilities
        .iter()
        .any(|capability| is_workspace_write_capability(capability));
    Ok(CodeExplainMetadata {
        plan: plan_path,
        store: store_path,
        capabilities,
        read_only: !writes_workspace,
        writes_workspace,
        max_estimated_model_calls: plan_explanation.max_estimated_model_calls(),
        max_estimated_tool_calls: plan_explanation.max_estimated_tool_calls(),
    })
}

fn is_workspace_write_capability(capability: &str) -> bool {
    matches!(capability, "file.write")
}

struct CodeLoopOptions {
    recipe: CodeRecipe,
    profile: PathBuf,
    input: Map<String, Value>,
    model_config: Option<PathBuf>,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    trace_raw: bool,
    state_out: Option<PathBuf>,
    checkpoint_out: Option<PathBuf>,
    jit_cache: Option<PathBuf>,
    parallel: bool,
    log: bool,
    tool_config: Option<PathBuf>,
    max_iterations: usize,
}

fn run_code_loop(options: CodeLoopOptions) -> Result<()> {
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
        completed = code_outputs_complete(options.recipe, &outputs);
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
    serde_json::to_writer_pretty(std::io::stdout(), &summary)?;
    println!();
    Ok(())
}

fn code_loop_iteration_input(
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

fn code_loop_feedback(previous_iterations: &[Value]) -> String {
    let mut lines = Vec::new();
    for iteration in previous_iterations {
        let iteration_number = iteration
            .get("iteration")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let completed = iteration
            .get("completed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let outputs = iteration.get("outputs").cloned().unwrap_or(Value::Null);
        let mut output_summary = serde_json::to_string(&outputs).unwrap_or_default();
        const MAX_OUTPUT_SUMMARY_CHARS: usize = 200_000;
        if output_summary.chars().count() > MAX_OUTPUT_SUMMARY_CHARS {
            output_summary = output_summary
                .chars()
                .take(MAX_OUTPUT_SUMMARY_CHARS)
                .collect::<String>();
            output_summary.push_str(" [AIR_TRUNCATED]");
        }
        lines.push(format!(
            "- iteration {iteration_number}: completed={completed}; outputs={output_summary}"
        ));
    }
    lines.join("\n")
}

fn code_outputs_complete(recipe: CodeRecipe, outputs: &Value) -> bool {
    match recipe {
        CodeRecipe::Auto => false,
        CodeRecipe::Explore => outputs.get("exploration").is_some(),
        CodeRecipe::Review => outputs
            .pointer("/review/search_quality/sufficient")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| outputs.get("review").is_some()),
        CodeRecipe::Repair => outputs
            .pointer("/repair/final_success")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        CodeRecipe::Build => {
            outputs
                .pointer("/build/test_success")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && outputs
                    .pointer("/build/audit_success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        }
    }
}

fn iteration_path(path: &Path, iteration: usize) -> PathBuf {
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

struct CodeInputOptions {
    task: String,
    recipe: CodeRecipe,
    target: Option<PathBuf>,
    test: Option<String>,
    query: Option<String>,
    related: Vec<PathBuf>,
    search_query: Option<String>,
    repo_query: Option<String>,
    required_terms: Vec<String>,
    output: Option<PathBuf>,
    brand: Option<String>,
    product: Option<String>,
    constraints: Vec<String>,
}

fn build_input(options: CodeInputOptions) -> Result<Map<String, Value>> {
    let CodeInputOptions {
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
    } = options;

    let recipe = resolve_recipe(
        recipe,
        target.as_ref(),
        test.as_ref(),
        search_query.as_ref(),
        repo_query.as_ref(),
        &required_terms,
        output.as_ref(),
        brand.as_ref(),
        product.as_ref(),
        &constraints,
    );

    match recipe {
        CodeRecipe::Auto => unreachable!("auto recipe is resolved before building input"),
        CodeRecipe::Explore => {
            let target = required_path(target, "--target", recipe)?;
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query.unwrap_or(task)));
            input.insert(
                "target_path".to_string(),
                Value::String(path_to_input_string(target)),
            );
            Ok(input)
        }
        CodeRecipe::Review => {
            let target = required_path(target, "--target", recipe)?;
            let target = path_to_input_string(target);
            let repo_query = repo_query.or(query).unwrap_or_else(|| target.clone());
            let related = if related.is_empty() {
                vec![PathBuf::from(&target)]
            } else {
                related
            };
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert(
                "search_query".to_string(),
                Value::String(search_query.unwrap_or(task)),
            );
            input.insert("repo_query".to_string(), Value::String(repo_query));
            input.insert("required_terms".to_string(), string_array(required_terms));
            input.insert("target_file".to_string(), Value::String(target));
            input.insert("related_files".to_string(), path_array(related));
            Ok(input)
        }
        CodeRecipe::Repair => {
            let target = required_path(target, "--target", recipe)?;
            let test = required_string(test, "--test", recipe)?;
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query.unwrap_or(task)));
            input.insert(
                "target_path".to_string(),
                Value::String(path_to_input_string(target)),
            );
            input.insert("related_files".to_string(), path_array(related));
            input.insert("test_command".to_string(), Value::String(test));
            Ok(input)
        }
        CodeRecipe::Build => {
            let output = required_path(output, "--output", recipe)?;
            let brand = brand.unwrap_or_else(|| "Product".to_string());
            let product = product.unwrap_or_else(|| brand.clone());
            let constraints = if constraints.is_empty() {
                vec![
                    "single self-contained HTML file".to_string(),
                    "no external network assets".to_string(),
                    "responsive down to mobile width".to_string(),
                    "buttons and text must not overlap".to_string(),
                ]
            } else {
                constraints
            };
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task));
            input.insert(
                "output_path".to_string(),
                Value::String(path_to_input_string(output)),
            );
            input.insert("brand".to_string(), Value::String(brand));
            input.insert("product".to_string(), Value::String(product));
            input.insert("constraints".to_string(), string_array(constraints));
            Ok(input)
        }
    }
}

fn required_path(value: Option<PathBuf>, flag: &str, recipe: CodeRecipe) -> Result<PathBuf> {
    let Some(value) = value else {
        bail!("air code --recipe {} requires {flag}", recipe_name(recipe));
    };
    Ok(value)
}

fn required_string(value: Option<String>, flag: &str, recipe: CodeRecipe) -> Result<String> {
    let Some(value) = value else {
        bail!("air code --recipe {} requires {flag}", recipe_name(recipe));
    };
    Ok(value)
}

fn default_profile(recipe: CodeRecipe) -> PathBuf {
    match recipe {
        CodeRecipe::Auto => PathBuf::from("examples/code-agent/explore.air-profile.yaml"),
        CodeRecipe::Explore => PathBuf::from("examples/code-agent/explore.air-profile.yaml"),
        CodeRecipe::Review => PathBuf::from("examples/code-agent/profile.air-profile.yaml"),
        CodeRecipe::Repair => PathBuf::from("examples/code-agent/repair-core.air-profile.yaml"),
        CodeRecipe::Build => PathBuf::from("examples/code-agent/apple-build.air-profile.yaml"),
    }
}

fn recipe_name(recipe: CodeRecipe) -> &'static str {
    match recipe {
        CodeRecipe::Auto => "auto",
        CodeRecipe::Explore => "explore",
        CodeRecipe::Review => "review",
        CodeRecipe::Repair => "repair",
        CodeRecipe::Build => "build",
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_recipe(
    recipe: CodeRecipe,
    _target: Option<&PathBuf>,
    test: Option<&String>,
    search_query: Option<&String>,
    repo_query: Option<&String>,
    required_terms: &[String],
    output: Option<&PathBuf>,
    brand: Option<&String>,
    product: Option<&String>,
    constraints: &[String],
) -> CodeRecipe {
    if recipe != CodeRecipe::Auto {
        return recipe;
    }
    if output.is_some() || brand.is_some() || product.is_some() || !constraints.is_empty() {
        return CodeRecipe::Build;
    }
    if test.is_some() {
        return CodeRecipe::Repair;
    }
    if search_query.is_some() || repo_query.is_some() || !required_terms.is_empty() {
        return CodeRecipe::Review;
    }
    CodeRecipe::Explore
}

fn path_array(paths: Vec<PathBuf>) -> Value {
    Value::Array(
        paths
            .into_iter()
            .map(path_to_input_string)
            .map(Value::String)
            .collect(),
    )
}

fn string_array(values: Vec<String>) -> Value {
    Value::Array(values.into_iter().map(Value::String).collect())
}

fn path_to_input_string(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn path_ref_to_input_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_repair_input() {
        let input = build_input(CodeInputOptions {
            task: "fix it".to_string(),
            recipe: CodeRecipe::Repair,
            target: Some(PathBuf::from("src/lib.rs")),
            test: Some("unit".to_string()),
            query: None,
            related: vec![PathBuf::from("src/test.rs")],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(input["task"], Value::String("fix it".to_string()));
        assert_eq!(input["query"], Value::String("fix it".to_string()));
        assert_eq!(
            input["target_path"],
            Value::String("src/lib.rs".to_string())
        );
        assert_eq!(input["test_command"], Value::String("unit".to_string()));
        assert_eq!(
            input["related_files"],
            Value::Array(vec![Value::String("src/test.rs".to_string())])
        );
    }

    #[test]
    fn auto_recipe_selects_repair_when_test_is_present() {
        let input = build_input(CodeInputOptions {
            task: "fix it".to_string(),
            recipe: CodeRecipe::Auto,
            target: Some(PathBuf::from("src/lib.rs")),
            test: Some("unit".to_string()),
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(input["test_command"], Value::String("unit".to_string()));
        assert_eq!(
            input["target_path"],
            Value::String("src/lib.rs".to_string())
        );
    }

    #[test]
    fn auto_recipe_selects_build_when_output_is_present() {
        let input = build_input(CodeInputOptions {
            task: "build it".to_string(),
            recipe: CodeRecipe::Auto,
            target: None,
            test: None,
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: Some(PathBuf::from("site/index.html")),
            brand: Some("Acme".to_string()),
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["output_path"],
            Value::String("site/index.html".to_string())
        );
        assert_eq!(input["brand"], Value::String("Acme".to_string()));
        assert_eq!(input["product"], Value::String("Acme".to_string()));
    }

    #[test]
    fn auto_recipe_selects_review_when_review_flags_are_present() {
        let input = build_input(CodeInputOptions {
            task: "review it".to_string(),
            recipe: CodeRecipe::Auto,
            target: Some(PathBuf::from("src/lib.rs")),
            test: None,
            query: None,
            related: vec![],
            search_query: Some("library docs".to_string()),
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["search_query"],
            Value::String("library docs".to_string())
        );
        assert_eq!(
            input["target_file"],
            Value::String("src/lib.rs".to_string())
        );
    }

    #[test]
    fn auto_recipe_defaults_to_read_only_explore() {
        let input = build_input(CodeInputOptions {
            task: "understand this".to_string(),
            recipe: CodeRecipe::Auto,
            target: Some(PathBuf::from("src/lib.rs")),
            test: None,
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(input["query"], Value::String("understand this".to_string()));
        assert_eq!(
            input["target_path"],
            Value::String("src/lib.rs".to_string())
        );
        assert!(input.get("test_command").is_none());
    }

    #[test]
    fn explain_payload_reports_requested_and_resolved_recipe() {
        let input = build_input(CodeInputOptions {
            task: "fix it".to_string(),
            recipe: CodeRecipe::Auto,
            target: Some(PathBuf::from("src/lib.rs")),
            test: Some("unit".to_string()),
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();
        let explanation = json!({
            "command": "code",
            "will_run": false,
            "requested_recipe": recipe_name(CodeRecipe::Auto),
            "resolved_recipe": recipe_name(CodeRecipe::Repair),
            "profile": path_ref_to_input_string(&default_profile(CodeRecipe::Repair)),
            "input": Value::Object(input),
        });

        assert_eq!(
            explanation["requested_recipe"],
            Value::String("auto".to_string())
        );
        assert_eq!(
            explanation["resolved_recipe"],
            Value::String("repair".to_string())
        );
        assert_eq!(
            explanation["profile"],
            Value::String("examples/code-agent/repair-core.air-profile.yaml".to_string())
        );
        assert_eq!(
            explanation["input"]["test_command"],
            Value::String("unit".to_string())
        );
    }

    #[test]
    fn explain_metadata_reports_profile_capabilities() {
        let profile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/code-agent/repair-core.air-profile.yaml");
        let metadata = explain_metadata_for_profile(&profile).unwrap();

        assert!(metadata
            .capabilities
            .iter()
            .any(|capability| capability == "file.write"));
        assert!(metadata.writes_workspace);
        assert!(!metadata.read_only);
        assert!(metadata.max_estimated_model_calls > 0);
        assert!(metadata.max_estimated_tool_calls > 0);
        assert!(metadata
            .plan
            .ends_with("code-repair-with-explore.air-plan.yaml"));
    }

    #[test]
    fn completion_detection_matches_recipe_outputs() {
        assert!(code_outputs_complete(
            CodeRecipe::Repair,
            &json!({"repair": {"final_success": true}})
        ));
        assert!(!code_outputs_complete(
            CodeRecipe::Repair,
            &json!({"repair": {"final_success": false}})
        ));
        assert!(code_outputs_complete(
            CodeRecipe::Build,
            &json!({"build": {"test_success": true, "audit_success": true}})
        ));
        assert!(!code_outputs_complete(
            CodeRecipe::Build,
            &json!({"build": {"test_success": true, "audit_success": false}})
        ));
    }

    #[test]
    fn iteration_path_preserves_extension() {
        assert_eq!(
            iteration_path(Path::new("target/generated/code.trace.jsonl"), 2),
            PathBuf::from("target/generated/code.trace.iter2.jsonl")
        );
    }

    #[test]
    fn loop_iteration_input_appends_previous_outputs_to_task() {
        let mut input = Map::new();
        input.insert("task".to_string(), Value::String("fix it".to_string()));
        input.insert(
            "target_path".to_string(),
            Value::String("src/lib.rs".to_string()),
        );
        let iterations = vec![json!({
            "iteration": 1,
            "completed": false,
            "outputs": {
                "repair": {
                    "final_success": false,
                    "diagnostics": [{"path": "src/lib.rs", "line": 3}]
                }
            }
        })];

        let next = code_loop_iteration_input(&input, &iterations);

        assert_eq!(next["target_path"], Value::String("src/lib.rs".to_string()));
        let task = next["task"].as_str().unwrap();
        assert!(task.starts_with("fix it"));
        assert!(task.contains("AIR loop context from previous iterations"));
        assert!(task.contains("final_success"));
    }

    #[test]
    fn build_recipe_requires_output() {
        let error = build_input(CodeInputOptions {
            task: "build a page".to_string(),
            recipe: CodeRecipe::Build,
            target: None,
            test: None,
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap_err();

        assert!(error.to_string().contains("requires --output"));
    }

    #[test]
    fn builds_review_input() {
        let input = build_input(CodeInputOptions {
            task: "review search".to_string(),
            recipe: CodeRecipe::Review,
            target: Some(PathBuf::from("scripts/search.cjs")),
            test: None,
            query: Some("search".to_string()),
            related: vec![PathBuf::from("scripts/search.test.cjs")],
            search_query: None,
            repo_query: None,
            required_terms: vec!["playwright".to_string()],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["search_query"],
            Value::String("review search".to_string())
        );
        assert_eq!(input["repo_query"], Value::String("search".to_string()));
        assert_eq!(
            input["target_file"],
            Value::String("scripts/search.cjs".to_string())
        );
        assert_eq!(
            input["related_files"],
            Value::Array(vec![Value::String("scripts/search.test.cjs".to_string())])
        );
        assert_eq!(
            input["required_terms"],
            Value::Array(vec![Value::String("playwright".to_string())])
        );
    }

    #[test]
    fn review_input_defaults_related_files_to_target() {
        let input = build_input(CodeInputOptions {
            task: "review search".to_string(),
            recipe: CodeRecipe::Review,
            target: Some(PathBuf::from("scripts/search.cjs")),
            test: None,
            query: None,
            related: vec![],
            search_query: None,
            repo_query: None,
            required_terms: vec![],
            output: None,
            brand: None,
            product: None,
            constraints: vec![],
        })
        .unwrap();

        assert_eq!(
            input["related_files"],
            Value::Array(vec![Value::String("scripts/search.cjs".to_string())])
        );
    }
}
