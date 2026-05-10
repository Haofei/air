use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const CODE_AGENT_PACK_PATH: &str = "examples/code-agent/code-agent.air-pack.yaml";
const CODE_AGENT_PACK_YAML: &str =
    include_str!("../../../examples/code-agent/code-agent.air-pack.yaml");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeAgentPack {
    pub(crate) recipes: Vec<CodeAgentPackRecipe>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_relative_profiles_resolve_from_pack_directory_even_when_cwd_has_match() {
        let pack = CodeAgentPackContext {
            path: PathBuf::from("target/generated/custom-pack/code-agent.air-pack.yaml"),
            pack: CodeAgentPack {
                recipes: vec![CodeAgentPackRecipe {
                    id: "plan".to_string(),
                    default_profile: PathBuf::from("project-plan.air-profile.yaml"),
                    intent: None,
                    completion: None,
                }],
            },
        };

        assert_eq!(
            pack.default_profile_for_recipe("plan").unwrap(),
            PathBuf::from("target/generated/custom-pack/project-plan.air-profile.yaml")
        );
    }

    #[test]
    fn pack_validation_rejects_empty_recipe_list() {
        let pack = CodeAgentPack { recipes: vec![] };

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
            recipes: vec![CodeAgentPackRecipe {
                id: " ".to_string(),
                default_profile: PathBuf::from("repair.air-profile.yaml"),
                intent: None,
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
            recipes: vec![CodeAgentPackRecipe {
                id: "repair".to_string(),
                default_profile: PathBuf::new(),
                intent: None,
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
            recipes: vec![
                CodeAgentPackRecipe {
                    id: "repair".to_string(),
                    default_profile: PathBuf::from("repair.air-profile.yaml"),
                    intent: None,
                    completion: None,
                },
                CodeAgentPackRecipe {
                    id: "repair".to_string(),
                    default_profile: PathBuf::from("other-repair.air-profile.yaml"),
                    intent: None,
                    completion: None,
                },
            ],
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("duplicate recipe ids should be rejected");

        assert!(
            error.to_string().contains("duplicate recipe repair"),
            "{error}"
        );
    }

    #[test]
    fn pack_completion_supports_all_and_any_rules() {
        let completion: CodeAgentCompletion = serde_yaml::from_str(
            r#"
all:
  - equals:
      path: /build/test_success
      value: true
  - any:
      - equals:
          path: /build/audit_success
          value: true
      - exists: /build/manual_approval
"#,
        )
        .unwrap();

        assert!(completion.is_complete(&serde_json::json!({
            "build": {
                "test_success": true,
                "audit_success": false,
                "manual_approval": {"by": "reviewer"}
            }
        })));
        assert!(!completion.is_complete(&serde_json::json!({
            "build": {
                "test_success": false,
                "audit_success": true
            }
        })));
    }

    #[test]
    fn pack_validation_rejects_invalid_completion_rules() {
        let pack = CodeAgentPack {
            recipes: vec![CodeAgentPackRecipe {
                id: "repair".to_string(),
                default_profile: PathBuf::from("repair.air-profile.yaml"),
                intent: None,
                completion: Some(CodeAgentCompletion {
                    all: vec![CodeAgentCompletionRule {
                        exists: Some("repair/final_success".to_string()),
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeAgentPackRecipe {
    pub(crate) id: String,
    pub(crate) default_profile: PathBuf,
    #[serde(default)]
    pub(crate) intent: Option<String>,
    #[serde(default)]
    pub(crate) completion: Option<CodeAgentCompletion>,
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
        if let Some(completion) = &recipe.completion {
            validate_completion(
                completion,
                &format!("code-agent pack {source} recipe {}", recipe.id),
            )?;
        }
    }
    Ok(())
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
