use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const CODE_AGENT_PACK_PATH: &str = "examples/code-agent/code-agent.air-pack.yaml";
const CODE_AGENT_PACK_YAML: &str =
    include_str!("../../../examples/code-agent/code-agent.air-pack.yaml");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodeAgentPack {
    pub(crate) default_profile: PathBuf,
    #[serde(default)]
    pub(crate) intent: Option<String>,
    #[serde(default)]
    pub(crate) completion: Option<CodeAgentCompletion>,
}

#[derive(Clone, Debug)]
pub(crate) struct CodeAgentPackContext {
    pub(crate) path: PathBuf,
    pub(crate) pack: CodeAgentPack,
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
    if pack.default_profile.as_os_str().is_empty() {
        bail!("code-agent pack {source} is missing default_profile");
    }
    let Some(completion) = &pack.completion else {
        bail!("code-agent pack {source} is missing completion rules");
    };
    validate_completion(completion, &format!("code-agent pack {source}"))?;
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
    pub(crate) fn default_profile(&self) -> PathBuf {
        self.resolve_profile_path(&self.pack.default_profile)
    }

    pub(crate) fn complete(&self, outputs: &Value) -> Result<bool> {
        let Some(completion) = self.pack.completion.as_ref() else {
            bail!(
                "code-agent pack {} is missing completion rules",
                self.path.display()
            );
        };
        Ok(completion.is_complete(outputs))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_relative_profile_resolves_from_pack_directory_even_when_cwd_has_match() {
        let pack = CodeAgentPackContext {
            path: PathBuf::from("target/generated/custom-pack/code-agent.air-pack.yaml"),
            pack: CodeAgentPack {
                default_profile: PathBuf::from("edit.air-profile.yaml"),
                intent: None,
                completion: Some(CodeAgentCompletion {
                    all: vec![],
                    any: vec![CodeAgentCompletionRule {
                        equals: Some(CodeAgentCompletionEquals {
                            path: "/edit/final_success".to_string(),
                            value: serde_json::json!(true),
                        }),
                        ..CodeAgentCompletionRule::default()
                    }],
                }),
            },
        };

        assert_eq!(
            pack.default_profile(),
            PathBuf::from("target/generated/custom-pack/edit.air-profile.yaml")
        );
    }

    #[test]
    fn pack_validation_rejects_empty_default_profile() {
        let pack = CodeAgentPack {
            default_profile: PathBuf::new(),
            intent: None,
            completion: Some(CodeAgentCompletion {
                all: vec![],
                any: vec![CodeAgentCompletionRule {
                    equals: Some(CodeAgentCompletionEquals {
                        path: "/edit/final_success".to_string(),
                        value: serde_json::json!(true),
                    }),
                    ..CodeAgentCompletionRule::default()
                }],
            }),
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("empty default_profile should be rejected");

        assert!(
            error.to_string().contains("missing default_profile"),
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
            default_profile: PathBuf::from("edit.air-profile.yaml"),
            intent: None,
            completion: Some(CodeAgentCompletion {
                all: vec![CodeAgentCompletionRule {
                    exists: Some("edit/final_success".to_string()),
                    ..CodeAgentCompletionRule::default()
                }],
                any: vec![],
            }),
        };

        let error = validate_code_agent_pack(&pack, "test-pack")
            .expect_err("completion paths must be JSON pointers");

        assert!(error.to_string().contains("JSON pointer"), "{error}");
    }

    #[test]
    fn default_pack_loads() {
        let pack = load_code_agent_pack(None).unwrap();

        assert_eq!(
            pack.default_profile(),
            PathBuf::from("examples/code-agent/edit.air-profile.yaml")
        );
        assert!(pack
            .complete(&serde_json::json!({
                "edit": {"final_success": true}
            }))
            .unwrap());
    }
}
