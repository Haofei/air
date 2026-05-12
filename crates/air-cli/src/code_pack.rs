use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const CODE_AGENT_PACK_PATH: &str = "examples/code-agent/code-agent.air-pack.yaml";
const CODE_AGENT_PACK_YAML: &str =
    include_str!("../../../examples/code-agent/code-agent.air-pack.yaml");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeAgentPack {
    #[serde(default)]
    pub(crate) routing: CodeAgentRouting,
    pub(crate) recipes: Vec<CodeAgentPackRecipe>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CodeAgentRouting {
    #[serde(default)]
    pub(crate) auto: Vec<CodeAgentAutoRoute>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeAgentAutoRoute {
    pub(crate) recipe: String,
    #[serde(default)]
    pub(crate) fallback: bool,
    #[serde(default)]
    pub(crate) when: CodeAgentRouteWhen,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CodeAgentRouteWhen {
    #[serde(default)]
    task_contains_word: Option<String>,
    #[serde(default)]
    all_present: Vec<String>,
    #[serde(default)]
    any_present: Vec<String>,
    #[serde(default)]
    all_absent: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CodeAgentRouteFacts {
    pub(crate) task: String,
    pub(crate) target: bool,
    pub(crate) write: bool,
    pub(crate) search_query: bool,
    pub(crate) repo_query: bool,
    pub(crate) required_terms: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeAgentRouteDecision {
    pub(crate) recipe: String,
    pub(crate) route_index: usize,
    pub(crate) fallback: bool,
    pub(crate) when: CodeAgentRouteWhen,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CodeAgentInputFacts {
    pub(crate) task: bool,
    pub(crate) target: bool,
    pub(crate) write: bool,
    pub(crate) query: bool,
    pub(crate) related: bool,
    pub(crate) search_query: bool,
    pub(crate) repo_query: bool,
    pub(crate) required_terms: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_relative_profiles_resolve_from_pack_directory_even_when_cwd_has_match() {
        let pack = CodeAgentPackContext {
            path: PathBuf::from("target/generated/custom-pack/code-agent.air-pack.yaml"),
            pack: CodeAgentPack {
                routing: CodeAgentRouting::default(),
                recipes: vec![CodeAgentPackRecipe {
                    id: "explore".to_string(),
                    default_profile: PathBuf::from("explore.air-profile.yaml"),
                    intent: None,
                    input: CodeAgentRecipeInput::default(),
                    completion: None,
                }],
            },
        };

        assert_eq!(
            pack.default_profile_for_recipe("explore").unwrap(),
            PathBuf::from("target/generated/custom-pack/explore.air-profile.yaml")
        );
    }

    #[test]
    fn pack_validation_rejects_empty_recipe_list() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("empty recipe list should be rejected");

        assert!(
            error
                .to_string()
                .contains("must declare at least one recipe"),
            "{error}"
        );
    }

    #[test]
    fn pack_validation_rejects_empty_recipe_id() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![CodeAgentPackRecipe {
                id: " ".to_string(),
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                input: CodeAgentRecipeInput::default(),
                completion: None,
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("blank recipe id should be rejected");

        assert!(error.to_string().contains("empty id"), "{error}");
    }

    #[test]
    fn pack_validation_rejects_empty_default_profile() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![CodeAgentPackRecipe {
                id: "edit".to_string(),
                default_profile: PathBuf::new(),
                intent: None,
                input: CodeAgentRecipeInput::default(),
                completion: None,
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("empty default_profile should be rejected");

        assert!(
            error.to_string().contains("missing default_profile"),
            "{error}"
        );
    }

    #[test]
    fn pack_validation_rejects_duplicate_recipe_ids() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![
                CodeAgentPackRecipe {
                    id: "edit".to_string(),
                    default_profile: PathBuf::from("edit.air-profile.yaml"),
                    intent: None,
                    input: CodeAgentRecipeInput::default(),
                    completion: None,
                },
                CodeAgentPackRecipe {
                    id: "edit".to_string(),
                    default_profile: PathBuf::from("other-edit.air-profile.yaml"),
                    intent: None,
                    input: CodeAgentRecipeInput::default(),
                    completion: None,
                },
            ],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("duplicate recipe ids should be rejected");

        assert!(
            error.to_string().contains("duplicate recipe edit"),
            "{error}"
        );
    }

    #[test]
    fn pack_completion_supports_all_and_any_rules() {
        let completion: CodeAgentCompletion = serde_yaml::from_str(
            r#"
all:
  - equals:
      path: /edit/final_success
      value: true
  - any:
      - equals:
          path: /edit/patch_applied
          value: true
      - exists: /edit/manual_approval
"#,
        )
        .unwrap();

        assert!(completion.is_complete(&serde_json::json!({
            "edit": {
                "final_success": true,
                "patch_applied": false,
                "manual_approval": {"by": "reviewer"}
            }
        })));
        assert!(!completion.is_complete(&serde_json::json!({
            "edit": {
                "final_success": false,
                "patch_applied": true
            }
        })));
    }

    #[test]
    fn pack_validation_rejects_invalid_completion_rules() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![CodeAgentPackRecipe {
                id: "edit".to_string(),
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                input: CodeAgentRecipeInput::default(),
                completion: Some(CodeAgentCompletion {
                    all: vec![CodeAgentCompletionRule {
                        exists: Some("edit/final_success".to_string()),
                        ..CodeAgentCompletionRule::default()
                    }],
                    any: vec![],
                }),
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("completion paths must be JSON pointers");

        assert!(error.to_string().contains("JSON pointer"), "{error}");
    }

    #[test]
    fn pack_validation_rejects_unknown_input_fields() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![CodeAgentPackRecipe {
                id: "edit".to_string(),
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                input: CodeAgentRecipeInput {
                    required: vec!["task".to_string()],
                    optional: vec!["unknown_flag".to_string()],
                    defaults: Map::new(),
                },
                completion: None,
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("recipe input should reject unknown fields");

        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn pack_validation_rejects_overlapping_input_fields() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![CodeAgentPackRecipe {
                id: "edit".to_string(),
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                input: CodeAgentRecipeInput {
                    required: vec!["task".to_string()],
                    optional: vec!["task".to_string()],
                    defaults: Map::new(),
                },
                completion: None,
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("recipe input should reject required/optional overlap");

        assert!(
            error.to_string().contains("both required and optional"),
            "{error}"
        );
    }

    #[test]
    fn pack_validation_rejects_default_for_required_input() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![CodeAgentPackRecipe {
                id: "edit".to_string(),
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                input: CodeAgentRecipeInput {
                    required: vec!["task".to_string()],
                    optional: vec![],
                    defaults: Map::from_iter([(
                        "task".to_string(),
                        Value::String("default task".to_string()),
                    )]),
                },
                completion: None,
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("input defaults should not satisfy required fields");

        assert!(
            error.to_string().contains("cannot also be required"),
            "{error}"
        );
    }

    #[test]
    fn pack_validation_rejects_default_for_undeclared_optional_input() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting::default(),
            recipes: vec![CodeAgentPackRecipe {
                id: "edit".to_string(),
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                input: CodeAgentRecipeInput {
                    required: vec!["task".to_string()],
                    optional: vec![],
                    defaults: Map::from_iter([(
                        "query".to_string(),
                        Value::String("default query".to_string()),
                    )]),
                },
                completion: None,
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("input defaults should require optional declaration");

        assert!(error.to_string().contains("must be optional"), "{error}");
    }

    #[test]
    fn pack_input_contract_accepts_required_fields() {
        let pack = load_code_agent_pack(None).unwrap();

        pack.validate_recipe_input_facts(
            "edit",
            &CodeAgentInputFacts {
                task: true,
                target: true,
                ..CodeAgentInputFacts::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn pack_input_contract_rejects_missing_required_fields() {
        let pack = load_code_agent_pack(None).unwrap();

        pack.validate_recipe_input_facts(
            "edit",
            &CodeAgentInputFacts {
                task: true,
                ..CodeAgentInputFacts::default()
            },
        )
        .expect("targetless edit should be accepted");

        let error = pack
            .validate_recipe_input_facts("edit", &CodeAgentInputFacts::default())
            .expect_err("missing task should be rejected by pack input contract");

        assert!(
            error.to_string().contains("missing required input"),
            "{error}"
        );
        assert!(error.to_string().contains("task"), "{error}");
    }

    #[test]
    fn pack_auto_routing_selects_first_matching_route() {
        let pack = load_code_agent_pack(None).unwrap();

        let decision = pack
            .resolve_auto_recipe_decision(&CodeAgentRouteFacts {
                task: "edit provider".to_string(),
                target: true,
                ..CodeAgentRouteFacts::default()
            })
            .unwrap();

        assert_eq!(decision.recipe, "edit");
        assert_eq!(decision.route_index, 1);
        assert!(!decision.fallback);
    }

    #[test]
    fn pack_auto_routing_uses_fallback() {
        let pack = load_code_agent_pack(None).unwrap();

        let decision = pack
            .resolve_auto_recipe_decision(&CodeAgentRouteFacts {
                task: "understand the code-agent architecture".to_string(),
                ..CodeAgentRouteFacts::default()
            })
            .unwrap();

        assert_eq!(decision.recipe, "explore");
        assert_eq!(decision.route_index, 2);
        assert!(decision.fallback);
    }

    #[test]
    fn pack_validation_rejects_auto_route_without_fallback() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting {
                auto: vec![CodeAgentAutoRoute {
                    recipe: "edit".to_string(),
                    fallback: false,
                    when: CodeAgentRouteWhen {
                        any_present: vec!["target".to_string()],
                        ..CodeAgentRouteWhen::default()
                    },
                }],
            },
            recipes: vec![CodeAgentPackRecipe {
                id: "edit".to_string(),
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                input: CodeAgentRecipeInput::default(),
                completion: None,
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("auto routing should require one fallback");

        assert!(
            error.to_string().contains("exactly one fallback route"),
            "{error}"
        );
    }

    #[test]
    fn pack_validation_rejects_auto_route_unknown_fields() {
        let pack = CodeAgentPack {
            routing: CodeAgentRouting {
                auto: vec![CodeAgentAutoRoute {
                    recipe: "edit".to_string(),
                    fallback: true,
                    when: CodeAgentRouteWhen {
                        any_present: vec!["unknown_flag".to_string()],
                        ..CodeAgentRouteWhen::default()
                    },
                }],
            },
            recipes: vec![CodeAgentPackRecipe {
                id: "edit".to_string(),
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                input: CodeAgentRecipeInput::default(),
                completion: None,
            }],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("auto routing should reject unknown fields");

        assert!(error.to_string().contains("unknown field"), "{error}");
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeAgentPackRecipe {
    pub(crate) id: String,
    pub(crate) default_profile: PathBuf,
    #[serde(default)]
    pub(crate) intent: Option<String>,
    #[serde(default)]
    pub(crate) input: CodeAgentRecipeInput,
    #[serde(default)]
    pub(crate) completion: Option<CodeAgentCompletion>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CodeAgentRecipeInput {
    #[serde(default)]
    pub(crate) required: Vec<String>,
    #[serde(default)]
    pub(crate) optional: Vec<String>,
    #[serde(default)]
    pub(crate) defaults: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeAgentCompletion {
    #[serde(default)]
    pub(crate) all: Vec<CodeAgentCompletionRule>,
    #[serde(default)]
    pub(crate) any: Vec<CodeAgentCompletionRule>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CodeAgentCompletionRule {
    #[serde(default)]
    exists: Option<String>,
    #[serde(default)]
    missing: Option<String>,
    #[serde(default)]
    non_empty_array: Option<String>,
    #[serde(default)]
    equals: Option<CodeAgentCompletionEquals>,
    #[serde(default)]
    all: Vec<CodeAgentCompletionRule>,
    #[serde(default)]
    any: Vec<CodeAgentCompletionRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CodeAgentCompletionEquals {
    path: String,
    value: Value,
}

#[derive(Clone, Debug)]
pub(crate) struct CodeAgentPackContext {
    pub(crate) path: PathBuf,
    pub(crate) pack: CodeAgentPack,
}

pub(crate) fn load_code_agent_pack(path: Option<PathBuf>) -> Result<CodeAgentPackContext> {
    match path {
        Some(path) => {
            let pack = read_code_agent_pack(&path)?;
            Ok(CodeAgentPackContext { path, pack })
        }
        None => Ok(CodeAgentPackContext {
            path: PathBuf::from(CODE_AGENT_PACK_PATH),
            pack: default_code_agent_pack()?,
        }),
    }
}

pub(crate) fn default_code_agent_pack() -> Result<CodeAgentPack> {
    let pack = serde_yaml::from_str(CODE_AGENT_PACK_YAML)
        .context("invalid code-agent AIR pack manifest")?;
    validate_code_agent_pack(&pack, CODE_AGENT_PACK_PATH)?;
    Ok(pack)
}

pub(crate) fn read_code_agent_pack(path: &Path) -> Result<CodeAgentPack> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read code-agent pack {}", path.display()))?;
    let pack = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid code-agent AIR pack manifest {}", path.display()))?;
    validate_code_agent_pack(&pack, &path.display().to_string())?;
    Ok(pack)
}

fn validate_code_agent_pack(pack: &CodeAgentPack, source: &str) -> Result<()> {
    if pack.recipes.is_empty() {
        bail!("code-agent pack {source} must declare at least one recipe");
    }
    let mut seen = HashSet::new();
    for recipe in &pack.recipes {
        if recipe.id.trim().is_empty() {
            bail!("code-agent pack {source} contains a recipe with empty id");
        }
        if recipe.default_profile.as_os_str().is_empty() {
            bail!(
                "code-agent pack {source} recipe {} is missing default_profile",
                recipe.id
            );
        }
        if !seen.insert(recipe.id.as_str()) {
            bail!(
                "code-agent pack {source} declares duplicate recipe {}",
                recipe.id
            );
        }
        validate_recipe_input(
            &recipe.input,
            &format!("code-agent pack {source} recipe {}", recipe.id),
        )?;
        if let Some(completion) = &recipe.completion {
            validate_completion(
                completion,
                &format!("code-agent pack {source} recipe {}", recipe.id),
            )?;
        }
    }
    validate_auto_routes(pack, source)?;
    Ok(())
}

fn validate_recipe_input(input: &CodeAgentRecipeInput, source: &str) -> Result<()> {
    let mut required = HashSet::new();
    for field in &input.required {
        if !is_known_recipe_input_field(field) {
            bail!("{source} input references unknown field {field}");
        }
        if !required.insert(field.as_str()) {
            bail!("{source} input declares duplicate required field {field}");
        }
    }
    let mut optional = HashSet::new();
    for field in &input.optional {
        if !is_known_recipe_input_field(field) {
            bail!("{source} input references unknown field {field}");
        }
        if required.contains(field.as_str()) {
            bail!("{source} input field {field} cannot be both required and optional");
        }
        if !optional.insert(field.as_str()) {
            bail!("{source} input declares duplicate optional field {field}");
        }
    }
    for field in input.defaults.keys() {
        if !is_known_recipe_input_field(field) {
            bail!("{source} input defaults reference unknown field {field}");
        }
        if required.contains(field.as_str()) {
            bail!("{source} input default field {field} cannot also be required");
        }
        if !optional.contains(field.as_str()) {
            bail!("{source} input default field {field} must be optional");
        }
    }
    Ok(())
}

fn is_known_recipe_input_field(field: &str) -> bool {
    matches!(
        field,
        "task"
            | "target"
            | "write"
            | "query"
            | "related"
            | "search_query"
            | "repo_query"
            | "required_terms"
            | "target_symbol_query"
            | "target_search_pattern"
    )
}

fn validate_auto_routes(pack: &CodeAgentPack, source: &str) -> Result<()> {
    let recipe_ids = pack
        .recipes
        .iter()
        .map(|recipe| recipe.id.as_str())
        .collect::<HashSet<_>>();
    let mut fallback_count = 0usize;
    for route in &pack.routing.auto {
        if route.recipe.trim().is_empty() {
            bail!("code-agent pack {source} has an auto route with empty recipe");
        }
        if !recipe_ids.contains(route.recipe.as_str()) {
            bail!(
                "code-agent pack {source} auto route references missing recipe {}",
                route.recipe
            );
        }
        if route.fallback {
            fallback_count += 1;
        }
        validate_route_when(&route.when, source)?;
    }
    if !pack.routing.auto.is_empty() && fallback_count != 1 {
        bail!("code-agent pack {source} auto routing must declare exactly one fallback route");
    }
    Ok(())
}

fn validate_route_when(when: &CodeAgentRouteWhen, source: &str) -> Result<()> {
    for field in when
        .all_present
        .iter()
        .chain(when.any_present.iter())
        .chain(when.all_absent.iter())
    {
        if !is_known_route_field(field) {
            bail!("code-agent pack {source} auto route references unknown field {field}");
        }
    }
    Ok(())
}

fn is_known_route_field(field: &str) -> bool {
    matches!(
        field,
        "target" | "write" | "search_query" | "repo_query" | "required_terms"
    )
}

fn validate_completion(completion: &CodeAgentCompletion, source: &str) -> Result<()> {
    if completion.all.is_empty() && completion.any.is_empty() {
        bail!("{source} completion must declare at least one all/any rule");
    }
    for rule in &completion.all {
        validate_completion_rule(rule, source)?;
    }
    for rule in &completion.any {
        validate_completion_rule(rule, source)?;
    }
    Ok(())
}

fn validate_completion_rule(rule: &CodeAgentCompletionRule, source: &str) -> Result<()> {
    let operator_count = [
        rule.exists.is_some(),
        rule.missing.is_some(),
        rule.non_empty_array.is_some(),
        rule.equals.is_some(),
        !rule.all.is_empty(),
        !rule.any.is_empty(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if operator_count != 1 {
        bail!("{source} completion rules must declare exactly one operator");
    }
    if let Some(path) = &rule.exists {
        validate_completion_pointer(path, source)?;
    }
    if let Some(path) = &rule.missing {
        validate_completion_pointer(path, source)?;
    }
    if let Some(path) = &rule.non_empty_array {
        validate_completion_pointer(path, source)?;
    }
    if let Some(equals) = &rule.equals {
        validate_completion_pointer(&equals.path, source)?;
    }
    for nested in &rule.all {
        validate_completion_rule(nested, source)?;
    }
    for nested in &rule.any {
        validate_completion_rule(nested, source)?;
    }
    Ok(())
}

fn validate_completion_pointer(path: &str, source: &str) -> Result<()> {
    if path.is_empty() || !path.starts_with('/') {
        bail!("{source} completion path {path:?} must be a JSON pointer");
    }
    Ok(())
}

impl CodeAgentPackContext {
    pub(crate) fn default_profile_for_recipe(&self, recipe: &str) -> Result<PathBuf> {
        Ok(self.recipe_for_id(recipe)?.default_profile)
    }

    pub(crate) fn recipe_complete(&self, recipe: &str, outputs: &Value) -> Result<bool> {
        let pack_recipe = self.recipe_for_id(recipe)?;
        let Some(completion) = pack_recipe.completion.as_ref() else {
            bail!(
                "code-agent pack {} recipe {recipe} is missing completion rules",
                self.path.display()
            );
        };
        Ok(completion.is_complete(outputs))
    }

    pub(crate) fn validate_recipe_input_facts(
        &self,
        recipe: &str,
        facts: &CodeAgentInputFacts,
    ) -> Result<()> {
        let pack_recipe = self.recipe_for_id(recipe)?;
        let missing = pack_recipe
            .input
            .required
            .iter()
            .filter(|field| !facts.field_present(field))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            bail!(
                "code-agent pack {} recipe {recipe} missing required input field(s): {}",
                self.path.display(),
                missing.join(", ")
            );
        }
        Ok(())
    }

    pub(crate) fn resolve_auto_recipe_decision(
        &self,
        facts: &CodeAgentRouteFacts,
    ) -> Result<CodeAgentRouteDecision> {
        if self.pack.routing.auto.is_empty() {
            bail!(
                "code-agent pack {} is missing routing.auto",
                self.path.display()
            );
        }
        let fallback = self
            .pack
            .routing
            .auto
            .iter()
            .enumerate()
            .find(|(_, route)| route.fallback);
        for (route_index, route) in self.pack.routing.auto.iter().enumerate() {
            if !route.fallback && route.when.matches(facts) {
                return Ok(CodeAgentRouteDecision {
                    recipe: route.recipe.clone(),
                    route_index,
                    fallback: false,
                    when: route.when.clone(),
                });
            }
        }
        let Some((route_index, route)) = fallback else {
            bail!(
                "code-agent pack {} auto routing has no fallback",
                self.path.display()
            );
        };
        Ok(CodeAgentRouteDecision {
            recipe: route.recipe.clone(),
            route_index,
            fallback: true,
            when: route.when.clone(),
        })
    }

    pub(crate) fn recipe_for_id(&self, recipe: &str) -> Result<CodeAgentPackRecipe> {
        let Some(mut pack_recipe) = self
            .pack
            .recipes
            .iter()
            .find(|pack_recipe| pack_recipe.id == recipe)
            .cloned()
        else {
            bail!(
                "code-agent pack {} is missing recipe {recipe}",
                self.path.display()
            );
        };
        pack_recipe.default_profile = self.resolve_profile_path(&pack_recipe.default_profile);
        Ok(pack_recipe)
    }

    fn resolve_profile_path(&self, profile: &Path) -> PathBuf {
        if profile.is_absolute() {
            return profile.to_path_buf();
        }
        self.path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.join(profile))
            .unwrap_or_else(|| profile.to_path_buf())
    }
}

impl CodeAgentRouteWhen {
    fn matches(&self, facts: &CodeAgentRouteFacts) -> bool {
        if let Some(word) = &self.task_contains_word {
            if !task_contains_word(&facts.task, word) {
                return false;
            }
        }
        if !self
            .all_present
            .iter()
            .all(|field| facts.field_present(field))
        {
            return false;
        }
        if !self.any_present.is_empty()
            && !self
                .any_present
                .iter()
                .any(|field| facts.field_present(field))
        {
            return false;
        }
        if !self
            .all_absent
            .iter()
            .all(|field| !facts.field_present(field))
        {
            return false;
        }
        true
    }
}

impl CodeAgentRouteFacts {
    fn field_present(&self, field: &str) -> bool {
        match field {
            "target" => self.target,
            "write" => self.write,
            "search_query" => self.search_query,
            "repo_query" => self.repo_query,
            "required_terms" => self.required_terms,
            _ => false,
        }
    }
}

impl CodeAgentInputFacts {
    fn field_present(&self, field: &str) -> bool {
        match field {
            "task" => self.task,
            "target" => self.target,
            "write" => self.write,
            "query" => self.query,
            "related" => self.related,
            "search_query" => self.search_query,
            "repo_query" => self.repo_query,
            "required_terms" => self.required_terms,
            _ => false,
        }
    }
}

fn task_contains_word(task: &str, word: &str) -> bool {
    task.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token.eq_ignore_ascii_case(word))
}

impl CodeAgentCompletion {
    fn is_complete(&self, outputs: &Value) -> bool {
        let all_complete = self.all.is_empty() || self.all.iter().all(|rule| rule.matches(outputs));
        let any_complete = self.any.is_empty() || self.any.iter().any(|rule| rule.matches(outputs));
        all_complete && any_complete
    }
}

impl CodeAgentCompletionRule {
    fn matches(&self, outputs: &Value) -> bool {
        if let Some(path) = &self.exists {
            return outputs.pointer(path).is_some();
        }
        if let Some(path) = &self.missing {
            return outputs.pointer(path).is_none();
        }
        if let Some(path) = &self.non_empty_array {
            return outputs
                .pointer(path)
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty());
        }
        if let Some(equals) = &self.equals {
            return outputs
                .pointer(&equals.path)
                .is_some_and(|value| value == &equals.value);
        }
        if !self.all.is_empty() {
            return self.all.iter().all(|rule| rule.matches(outputs));
        }
        if !self.any.is_empty() {
            return self.any.iter().any(|rule| rule.matches(outputs));
        }
        false
    }
}
