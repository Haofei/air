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
    /// Read-only repository exploration and planning.
    Explore,
    /// Grounded code review with repository and external evidence.
    Review,
    /// Unified code edit loop: model-selected read/search/edit/test tools under AIR policy.
    Edit,
}

pub(crate) struct CodeInputOptions {
    pub(crate) task: String,
    pub(crate) recipe: CodeRecipe,
    pub(crate) target: Option<PathBuf>,
    pub(crate) write: Vec<PathBuf>,
    pub(crate) query: Option<String>,
    pub(crate) related: Vec<PathBuf>,
    pub(crate) search_query: Option<String>,
    pub(crate) repo_query: Option<String>,
    pub(crate) required_terms: Vec<String>,
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
        write,
        query,
        related,
        search_query,
        repo_query,
        required_terms,
    } = options;

    let recipe = resolve_recipe_name(
        pack,
        &task,
        recipe,
        target.as_ref(),
        !write.is_empty(),
        search_query.as_ref(),
        repo_query.as_ref(),
        &required_terms,
    )?;

    match recipe {
        CodeRecipe::Auto => unreachable!("auto recipe is resolved before building input"),
        CodeRecipe::Explore => {
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query.unwrap_or(task)));
            input.insert(
                "target_path".to_string(),
                Value::String(optional_path_to_input_string(target)),
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
        CodeRecipe::Edit => {
            let query = query.unwrap_or_else(|| task.clone());
            let target_search_pattern = code_search_pattern(&query);
            let write_paths = edit_write_paths(target.as_ref(), &write);
            let target_symbol_query = code_symbol_query(&query);
            let mut input = Map::new();
            input.insert("task".to_string(), Value::String(task.clone()));
            input.insert("query".to_string(), Value::String(query));
            input.insert(
                "target_search_pattern".to_string(),
                Value::String(target_search_pattern),
            );
            input.insert(
                "target_symbol_query".to_string(),
                Value::String(target_symbol_query),
            );
            input.insert(
                "target_path".to_string(),
                Value::String(optional_path_to_input_string(target)),
            );
            input.insert("related_files".to_string(), path_array(related));
            input.insert("write_paths".to_string(), path_array(write_paths));
            input.insert(
                "acceptance_assertions".to_string(),
                Value::Array(Vec::new()),
            );
            Ok(input)
        }
    }
}

fn edit_write_paths(target: Option<&PathBuf>, write: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(target) = target {
        if !target.as_os_str().is_empty() {
            paths.push(target.clone());
        }
    }
    for path in write {
        if !path.as_os_str().is_empty() && !paths.iter().any(|existing| existing == path) {
            paths.push(path.clone());
        }
    }
    paths
}

fn required_path(value: Option<PathBuf>, flag: &str, recipe: CodeRecipe) -> Result<PathBuf> {
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
        CodeRecipe::Explore => "explore",
        CodeRecipe::Review => "review",
        CodeRecipe::Edit => "edit",
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
    write: bool,
    search_query: Option<&String>,
    repo_query: Option<&String>,
    required_terms: &[String],
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
        write,
        search_query: search_query.is_some(),
        repo_query: repo_query.is_some(),
        required_terms: !required_terms.is_empty(),
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
    write: bool,
    search_query: Option<&String>,
    repo_query: Option<&String>,
    required_terms: &[String],
) -> Result<CodeRecipe> {
    Ok(resolve_recipe(
        pack,
        task,
        recipe,
        target,
        write,
        search_query,
        repo_query,
        required_terms,
    )?
    .recipe)
}

fn recipe_from_name(recipe: &str) -> Option<CodeRecipe> {
    match recipe {
        "auto" => Some(CodeRecipe::Auto),
        "explore" => Some(CodeRecipe::Explore),
        "review" => Some(CodeRecipe::Review),
        "edit" => Some(CodeRecipe::Edit),
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

pub(crate) fn optional_path_to_input_string(target: Option<PathBuf>) -> String {
    target.map(path_to_input_string).unwrap_or_default()
}

pub(crate) fn path_ref_to_input_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn code_search_pattern(query: &str) -> String {
    let tokens = code_search_tokens(query);

    if tokens.is_empty() {
        "TODO_DO_NOT_MATCH_EMPTY_CODE_SEARCH_PATTERN".to_string()
    } else {
        tokens.join("|")
    }
}

pub(crate) fn code_symbol_query(query: &str) -> String {
    let tokens = code_search_tokens(query);
    if let Some(token) = tokens.iter().find(|token| {
        !is_low_signal_search_token(token)
            && token
                .chars()
                .any(|character| character.is_ascii_uppercase())
    }) {
        return token.clone();
    }
    tokens
        .into_iter()
        .filter(|token| !is_low_signal_search_token(token) && token.contains('_'))
        .max_by_key(|token| token.len())
        .unwrap_or_default()
}

fn code_search_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in query.chars() {
        if character == '_' || character == '-' || character.is_ascii_alphanumeric() {
            current.push(character);
        } else if !current.is_empty() {
            push_search_token(&mut tokens, &current);
            current.clear();
        }
    }
    if !current.is_empty() {
        push_search_token(&mut tokens, &current);
    }

    select_search_tokens(tokens)
}

fn push_search_token(tokens: &mut Vec<String>, token: &str) {
    let token = token.trim_matches('-').replace('-', "_");
    if token.len() < 3 || tokens.iter().any(|existing| existing == &token) {
        return;
    }
    tokens.push(token);
}

fn select_search_tokens(tokens: Vec<String>) -> Vec<String> {
    const MAX_SEARCH_TOKENS: usize = 8;
    if tokens.len() <= MAX_SEARCH_TOKENS {
        return tokens;
    }

    let mut ranked = tokens
        .into_iter()
        .enumerate()
        .map(|(index, token)| {
            let rank = search_token_rank(&token);
            (index, rank, token)
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(index, rank, _)| (*rank, *index));
    ranked.truncate(MAX_SEARCH_TOKENS);
    ranked.sort_by_key(|(index, _, _)| *index);
    ranked
        .into_iter()
        .map(|(_, _, token)| token)
        .collect::<Vec<_>>()
}

fn search_token_rank(token: &str) -> u8 {
    let mut rank: u8 =
        if token.contains('_') || token.chars().any(|character| character.is_uppercase()) {
            0
        } else if token.len() >= 8 {
            1
        } else if token.len() >= 5 {
            2
        } else {
            3
        };

    if is_low_signal_search_token(token) {
        rank = rank.saturating_add(4);
    }
    rank
}

fn is_low_signal_search_token(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "the"
            | "and"
            | "when"
            | "with"
            | "that"
            | "this"
            | "should"
            | "only"
            | "instead"
            | "hidden"
            | "option"
            | "print"
            | "json"
            | "final"
            | "output"
            | "keep"
            | "existing"
            | "behavior"
            | "unchanged"
            | "use"
            | "using"
            | "around"
            | "preserve"
            | "configured"
            | "verification"
            | "helper"
            | "small"
            | "duplicated"
            | "omitted"
            | "set"
            | "not"
            | "run"
            | "plan"
            | "add"
            | "refactor"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edit_input_does_not_guess_split_acceptance_assertions() {
        let input = build_input(CodeInputOptions {
            task: "split context tool".to_string(),
            recipe: CodeRecipe::Edit,
            target: Some(PathBuf::from("crates/air-tools/src/lib.rs")),
            write: vec![PathBuf::from("crates/air-tools/src/context_tools.rs")],
            query: Some("call_context_measure_tool".to_string()),
            related: Vec::new(),
            search_query: None,
            repo_query: None,
            required_terms: Vec::new(),
        })
        .unwrap();

        assert_eq!(
            input.get("acceptance_assertions").unwrap(),
            &json!([]),
            "AIR should not invent structural postconditions from a natural-language edit task"
        );
    }

    #[test]
    fn symbol_query_ignores_generic_refactor_verbs() {
        assert_eq!(
            code_symbol_query(
                "Refactor crates/air-tools/src/candidate_tools.rs by extracting a helper"
            ),
            "candidate_tools"
        );
    }

    #[test]
    fn symbol_query_ignores_sentence_case_instruction_words() {
        assert_eq!(
            code_symbol_query(
                "Use a small helper around duplicated full_output_path artifact metadata"
            ),
            "full_output_path"
        );
    }
}
