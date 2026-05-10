#[cfg(test)]
use crate::code_pack::load_code_agent_pack;
use crate::code_pack::{CodeAgentPackContext, CodeAgentRouteDecision, CodeAgentRouteFacts};
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum CodeRecipe {
    /// Select a recipe from typed flags, preferring read-only exploration when ambiguous.
    Auto,
    /// Project-level planning with a task graph, evidence, and acceptance criteria.
    Plan,
    /// Read-only repository exploration and planning.
    Explore,
    /// Grounded code review with repository and external evidence.
    Review,
    /// Core repair loop: explore, select context, patch, and retest.
    Repair,
    /// Behavior-preserving refactor loop: explore, patch intentionally, and retest.
    Refactor,
    /// Open-ended bounded refactor loop: explore, select one target/test, patch, and retest.
    OpenRefactor,
    /// Build one bounded static page and verify it.
    Build,
}

pub(crate) struct CodeInputOptions {
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
    pub(crate) force_patch: bool,
}

#[cfg(test)]
pub(crate) fn build_input(options: CodeInputOptions) -> Result<Map<String, Value>> {
    let pack = load_code_agent_pack(None)?;
    build_input_with_pack(&pack, options)
}

pub(crate) fn build_input_with_pack(
    pack: &CodeAgentPackContext,
    options: CodeInputOptions,
) -> Result<Map<String, Value>> {
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
        force_patch,
    } = options;

    let recipe = resolve_recipe_name(
        pack,
        &task,
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
    )?;

    match recipe {
        CodeRecipe::Auto => unreachable!("auto recipe is resolved before building input"),
        CodeRecipe::Plan => {
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query.unwrap_or(task)));
            Ok(input)
        }
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
        CodeRecipe::Repair | CodeRecipe::Refactor => {
            let target = required_path(target, "--target", recipe)?;
            let test = required_string(test, "--test", recipe)?;
            let query = query.unwrap_or_else(|| task.clone());
            let target_search_pattern = code_search_pattern(&query);
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query));
            input.insert(
                "target_search_pattern".to_string(),
                Value::String(target_search_pattern),
            );
            input.insert(
                "target_path".to_string(),
                Value::String(path_to_input_string(target)),
            );
            input.insert("related_files".to_string(), path_array(related));
            input.insert("test_command".to_string(), Value::String(test));
            input.insert(
                "force_patch".to_string(),
                Value::Bool(force_patch || recipe == CodeRecipe::Refactor),
            );
            Ok(input)
        }
        CodeRecipe::OpenRefactor => {
            let query = query.unwrap_or_else(|| task.clone());
            let target_search_pattern = code_search_pattern(&query);
            let allowed_test_commands = match test {
                Some(test) => vec![test],
                None => vec![
                    "repair_fixture_test".to_string(),
                    "repair_multifile_test".to_string(),
                    "refactor_fixture_test".to_string(),
                ],
            };
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query));
            input.insert(
                "target_search_pattern".to_string(),
                Value::String(target_search_pattern),
            );
            input.insert(
                "allowed_test_commands".to_string(),
                string_array(allowed_test_commands),
            );
            Ok(input)
        }
        CodeRecipe::Build => {
            let output = required_path(output, "--output", recipe)?;
            let brand = brand.unwrap_or_else(|| "Product".to_string());
            let product = product.unwrap_or_else(|| brand.clone());
            let constraints = if constraints.is_empty() {
                code_recipe_string_array_default(pack, recipe, "constraints")?
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

fn code_recipe_string_array_default(
    pack: &CodeAgentPackContext,
    recipe: CodeRecipe,
    field: &str,
) -> Result<Vec<String>> {
    let pack_recipe = pack.recipe_for_id(recipe_name(recipe))?;
    let Some(value) = pack_recipe.input.defaults.get(field) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        bail!(
            "code-agent pack {} recipe {} input default {field} must be an array of strings",
            pack.path.display(),
            recipe_name(recipe)
        );
    };
    items
        .iter()
        .map(|item| {
            item.as_str().map(ToString::to_string).with_context(|| {
                format!(
                    "code-agent pack {} recipe {} input default {field} must be an array of strings",
                    pack.path.display(),
                    recipe_name(recipe)
                )
            })
        })
        .collect()
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

pub(crate) fn default_profile(pack: &CodeAgentPackContext, recipe: CodeRecipe) -> Result<PathBuf> {
    pack.default_profile_for_recipe(recipe_name(recipe))
}

pub(crate) fn recipe_name(recipe: CodeRecipe) -> &'static str {
    match recipe {
        CodeRecipe::Auto => "auto",
        CodeRecipe::Plan => "plan",
        CodeRecipe::Explore => "explore",
        CodeRecipe::Review => "review",
        CodeRecipe::Repair => "repair",
        CodeRecipe::Refactor => "refactor",
        CodeRecipe::OpenRefactor => "open-refactor",
        CodeRecipe::Build => "build",
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CodeRecipeResolution {
    pub(crate) recipe: CodeRecipe,
    pub(crate) routing_decision: Option<CodeAgentRouteDecision>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_recipe(
    pack: &CodeAgentPackContext,
    task: &str,
    recipe: CodeRecipe,
    target: Option<&PathBuf>,
    test: Option<&String>,
    search_query: Option<&String>,
    repo_query: Option<&String>,
    required_terms: &[String],
    output: Option<&PathBuf>,
    brand: Option<&String>,
    product: Option<&String>,
    constraints: &[String],
) -> Result<CodeRecipeResolution> {
    if recipe != CodeRecipe::Auto {
        return Ok(CodeRecipeResolution {
            recipe,
            routing_decision: None,
        });
    }
    let routing_decision = pack.resolve_auto_recipe_decision(&CodeAgentRouteFacts {
        task: task.to_string(),
        target: target.is_some(),
        test: test.is_some(),
        search_query: search_query.is_some(),
        repo_query: repo_query.is_some(),
        required_terms: !required_terms.is_empty(),
        output: output.is_some(),
        brand: brand.is_some(),
        product: product.is_some(),
        constraints: !constraints.is_empty(),
    })?;
    let recipe = recipe_from_name(&routing_decision.recipe).with_context(|| {
        format!(
            "code-agent pack {} auto routing returned unsupported recipe {}",
            pack.path.display(),
            routing_decision.recipe
        )
    })?;
    Ok(CodeRecipeResolution {
        recipe,
        routing_decision: Some(routing_decision),
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_recipe_name(
    pack: &CodeAgentPackContext,
    task: &str,
    recipe: CodeRecipe,
    target: Option<&PathBuf>,
    test: Option<&String>,
    search_query: Option<&String>,
    repo_query: Option<&String>,
    required_terms: &[String],
    output: Option<&PathBuf>,
    brand: Option<&String>,
    product: Option<&String>,
    constraints: &[String],
) -> Result<CodeRecipe> {
    Ok(resolve_recipe(
        pack,
        task,
        recipe,
        target,
        test,
        search_query,
        repo_query,
        required_terms,
        output,
        brand,
        product,
        constraints,
    )?
    .recipe)
}

fn recipe_from_name(recipe: &str) -> Option<CodeRecipe> {
    match recipe {
        "auto" => Some(CodeRecipe::Auto),
        "plan" => Some(CodeRecipe::Plan),
        "explore" => Some(CodeRecipe::Explore),
        "review" => Some(CodeRecipe::Review),
        "repair" => Some(CodeRecipe::Repair),
        "refactor" => Some(CodeRecipe::Refactor),
        "open-refactor" => Some(CodeRecipe::OpenRefactor),
        "build" => Some(CodeRecipe::Build),
        _ => None,
    }
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

pub(crate) fn path_to_input_string(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn path_ref_to_input_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn code_search_pattern(query: &str) -> String {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in query.chars() {
        if character == '_' || character.is_ascii_alphanumeric() {
            current.push(character);
        } else if !current.is_empty() {
            push_search_token(&mut tokens, &current);
            current.clear();
        }
    }
    if !current.is_empty() {
        push_search_token(&mut tokens, &current);
    }

    if tokens.is_empty() {
        "TODO_DO_NOT_MATCH_EMPTY_CODE_SEARCH_PATTERN".to_string()
    } else {
        tokens.join("|")
    }
}

fn push_search_token(tokens: &mut Vec<String>, token: &str) {
    if token.len() < 3 || tokens.iter().any(|existing| existing == token) {
        return;
    }
    tokens.push(token.to_string());
}
