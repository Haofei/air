use air_backend_openai::{OpenAiCompatibleConfig, OpenAiCompatibleModelProvider};
use air_runtime::{ModelProvider, RuntimeError};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct EchoModels;

impl ModelProvider for EchoModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        Ok(json!({
            "model": name,
            "input": input
        }))
    }
}

#[derive(Clone)]
pub(crate) struct FixtureModels {
    fixtures: BTreeMap<String, FixtureModelOutput>,
}

#[derive(Clone)]
enum FixtureModelOutput {
    Static(Value),
    Sequence { values: Vec<Value>, index: usize },
}

impl ModelProvider for FixtureModels {
    fn call_model(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        let Some(output) = self.fixtures.get_mut(name) else {
            return Err(RuntimeError::Provider(format!(
                "missing fixture model {name}"
            )));
        };
        match output {
            FixtureModelOutput::Static(value) => Ok(value.clone()),
            FixtureModelOutput::Sequence { values, index } => {
                if values.is_empty() {
                    return Err(RuntimeError::Provider(format!(
                        "fixture model {name} sequence must not be empty"
                    )));
                }
                let value = values
                    .get(*index)
                    .unwrap_or_else(|| values.last().expect("non-empty sequence"))
                    .clone();
                *index = index.saturating_add(1);
                Ok(value)
            }
        }
    }
}

#[derive(Clone)]
pub(crate) enum ModelProviderChoice {
    Echo(EchoModels),
    Fixture(FixtureModels),
    OpenAi(OpenAiCompatibleModelProvider),
}

impl ModelProviderChoice {
    pub(crate) fn echo() -> Self {
        Self::Echo(EchoModels)
    }

    pub(crate) fn openai(config: OpenAiCompatibleConfig) -> Result<Self> {
        Ok(Self::OpenAi(OpenAiCompatibleModelProvider::new(config)?))
    }

    pub(crate) fn from_config_file(path: PathBuf) -> Result<Self> {
        if let Some(fixtures) = parse_fixture_model_config(&path)? {
            return Ok(Self::Fixture(FixtureModels { fixtures }));
        }
        let config = air_backend_openai::parse_config_file(path)?;
        Self::openai(config)
    }
}

impl ModelProvider for ModelProviderChoice {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match self {
            ModelProviderChoice::Echo(provider) => provider.call_model(name, input),
            ModelProviderChoice::Fixture(provider) => provider.call_model(name, input),
            ModelProviderChoice::OpenAi(provider) => provider.call_model(name, input),
        }
    }

    fn call_model_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        match self {
            ModelProviderChoice::Echo(provider) => {
                provider.call_model_with_timeout(name, input, timeout)
            }
            ModelProviderChoice::Fixture(provider) => {
                provider.call_model_with_timeout(name, input, timeout)
            }
            ModelProviderChoice::OpenAi(provider) => {
                provider.call_model_with_timeout(name, input, timeout)
            }
        }
    }
}

#[derive(Deserialize)]
struct FixtureModelConfig {
    fixtures: Option<BTreeMap<String, Value>>,
}

fn parse_fixture_model_config(
    path: &PathBuf,
) -> Result<Option<BTreeMap<String, FixtureModelOutput>>> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read model config {}", path.display()))?;
    let config: FixtureModelConfig = serde_json::from_str(&source)
        .with_context(|| format!("failed to parse model config {}", path.display()))?;
    let Some(fixtures) = config.fixtures else {
        return Ok(None);
    };
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut resolved = BTreeMap::new();
    for (name, fixture) in fixtures {
        let fixture = resolve_fixture_files(fixture, base_dir)?;
        resolved.insert(name, fixture_model_output(fixture)?);
    }
    Ok(Some(resolved))
}

fn fixture_model_output(value: Value) -> Result<FixtureModelOutput> {
    if let Some(object) = value.as_object() {
        if object.len() == 1 && object.contains_key("$sequence") {
            let values = object
                .get("$sequence")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("fixture $sequence value must be an array"))?;
            if values.is_empty() {
                anyhow::bail!("fixture $sequence value must not be empty");
            }
            return Ok(FixtureModelOutput::Sequence { values, index: 0 });
        }
    }
    Ok(FixtureModelOutput::Static(value))
}

fn resolve_fixture_files(value: Value, base_dir: &Path) -> Result<Value> {
    match value {
        Value::Array(values) => values
            .into_iter()
            .map(|value| resolve_fixture_files(value, base_dir))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(mut values) => {
            if values.len() == 1 && values.contains_key("$file") {
                let path_value = values.remove("$file").expect("checked fixture file key");
                let path = path_value.as_str().ok_or_else(|| {
                    anyhow::anyhow!("fixture $file value must be a relative string")
                })?;
                let path = safe_fixture_file_path(base_dir, path)?;
                let content = fs::read_to_string(&path)
                    .with_context(|| format!("failed to read fixture file {}", path.display()))?;
                return Ok(Value::String(content));
            }
            let mut resolved = serde_json::Map::new();
            for (key, value) in values {
                resolved.insert(key, resolve_fixture_files(value, base_dir)?);
            }
            Ok(Value::Object(resolved))
        }
        other => Ok(other),
    }
}

fn safe_fixture_file_path(base_dir: &Path, path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("fixture $file path must stay inside the model config directory");
    }
    let base_dir = base_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve fixture base dir {}", base_dir.display()))?;
    let candidate = base_dir.join(path).canonicalize().with_context(|| {
        format!(
            "failed to resolve fixture file {}",
            base_dir.join(path).display()
        )
    })?;
    if !candidate.starts_with(&base_dir) {
        anyhow::bail!("fixture $file path must stay inside the model config directory");
    }
    Ok(candidate)
}

pub(crate) fn call_openai_model(
    model_config: PathBuf,
    model_alias: &str,
    input: &Value,
) -> Result<Value> {
    let config = air_backend_openai::parse_config_file(model_config)?;
    let mut models = OpenAiCompatibleModelProvider::new(config)?;
    Ok(models.call_model(model_alias, input)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use air_core::{StateAction, TypeSpec, Workflow};
    use std::collections::BTreeSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("air-cli-models-{name}-{stamp}.json"))
    }

    #[test]
    fn fixture_model_config_returns_static_model_outputs() {
        let path = temp_file("fixture");
        fs::write(
            &path,
            r#"{
              "fixtures": {
                "reviewer": {
                  "summary": "fixture review",
                  "findings": [],
                  "source_ids": [],
                  "search_quality": { "sufficient": true, "gaps": [] },
                  "next_steps": []
                }
              }
            }"#,
        )
        .unwrap();

        let mut provider = ModelProviderChoice::from_config_file(path).unwrap();
        let output = provider
            .call_model("reviewer", &json!({"ignored": true}))
            .unwrap();
        assert_eq!(output["summary"], json!("fixture review"));
    }

    #[test]
    fn fixture_model_config_loads_file_backed_strings() {
        let dir = std::env::temp_dir().join(format!(
            "air-cli-models-file-backed-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("fixtures")).unwrap();
        fs::write(
            dir.join("fixtures/page.html"),
            "<!doctype html>\n<html></html>\n",
        )
        .unwrap();
        let path = dir.join("models.json");
        fs::write(
            &path,
            r#"{
              "fixtures": {
                "html_fixture": {
                  "content": { "$file": "fixtures/page.html" }
                }
              }
            }"#,
        )
        .unwrap();

        let mut provider = ModelProviderChoice::from_config_file(path).unwrap();
        let output = provider
            .call_model("html_fixture", &json!({"ignored": true}))
            .unwrap();
        assert_eq!(output["content"], json!("<!doctype html>\n<html></html>\n"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fixture_model_config_supports_sequences() {
        let path = temp_file("sequence");
        fs::write(
            &path,
            r#"{
              "fixtures": {
                "planner": {
                  "$sequence": [
                    { "complete": false, "step": 1 },
                    { "complete": true, "step": 2 }
                  ]
                }
              }
            }"#,
        )
        .unwrap();

        let mut provider = ModelProviderChoice::from_config_file(path).unwrap();
        let first = provider.call_model("planner", &json!({})).unwrap();
        let second = provider.call_model("planner", &json!({})).unwrap();
        let third = provider.call_model("planner", &json!({})).unwrap();

        assert_eq!(first["step"], json!(1));
        assert_eq!(second["step"], json!(2));
        assert_eq!(third["step"], json!(2));
    }

    #[test]
    fn model_config_prompts_match_code_agent_schemas() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let config = air_backend_openai::parse_config_file(
            root.join("examples/bigmodel-openai-compatible.json"),
        )
        .unwrap();
        let requirements = code_agent_model_output_requirements(&root);
        assert_eq!(
            requirements
                .get("code_edit_decider")
                .cloned()
                .unwrap_or_default(),
            BTreeSet::from([
                "complete".to_string(),
                "rationale".to_string(),
                "tool_calls".to_string()
            ])
        );
        assert!(
            requirements
                .get("code_edit_summarizer")
                .is_some_and(|required| required.contains("final_success")
                    && required.contains("patch_applied")),
            "code_edit_summarizer prompt must track edit completion fields"
        );

        for (alias, required) in requirements {
            let prompt = config
                .models
                .get(&alias)
                .and_then(|model| model.system_prompt.as_deref())
                .unwrap_or_else(|| panic!("model config must define system_prompt for {alias}"))
                .to_lowercase();

            if !prompt.contains("json object") {
                continue;
            }
            for key in &required {
                assert!(
                    prompt.contains(key),
                    "{alias} prompt must mention required output key {key}"
                );
            }
        }
        let edit_prompt = config
            .models
            .get("code_edit_decider")
            .and_then(|model| model.system_prompt.as_deref())
            .unwrap_or_default()
            .to_lowercase();
        let edit_config = config.models.get("code_edit_decider").unwrap();
        assert_eq!(
            edit_config.native_tool_calls,
            Some(true),
            "code_edit_decider should use native provider tool calls instead of prompt-only JSON tool selection"
        );
        assert!(
            !edit_prompt.contains("exactly these keys: patch"),
            "code_edit_decider prompt must not require the old patch-only schema"
        );
        assert!(
            !edit_prompt.contains("patch must be"),
            "code_edit_decider prompt must not describe the old patch-only schema"
        );
        assert!(
            edit_prompt.contains("file.patch"),
            "code_edit_decider prompt must describe the unified diff patch tool"
        );
        assert!(
            edit_prompt.contains("tool_schemas"),
            "code_edit_decider prompt must tell models to follow tool_schemas"
        );
        assert!(
            edit_prompt.contains("native provider tool calls"),
            "code_edit_decider prompt must prefer native provider tool calls"
        );
        assert!(
            edit_prompt.contains("full extensions such as .json"),
            "code_edit_decider prompt must preserve concrete path extensions"
        );
        assert!(
            edit_prompt.contains("byte-for-byte"),
            "code_edit_decider prompt must require exact path copying"
        );
        assert!(
            edit_prompt
                .contains("skip todo tools for single-file refactors and already-targeted edits"),
            "code_edit_decider prompt must avoid todo churn on targeted refactors"
        );
        assert!(
            edit_prompt.contains("do not do broad helper archaeology"),
            "code_edit_decider prompt must discourage over-exploration on targeted edits"
        );
        let summarize_prompt = config
            .models
            .get("code_edit_summarizer")
            .and_then(|model| model.system_prompt.as_deref())
            .unwrap_or_default()
            .to_lowercase();
        assert!(
            summarize_prompt.contains("preexisting_changed_files")
                && summarize_prompt.contains("array of objects")
                && summarize_prompt.contains("\"path\": string")
                && summarize_prompt.contains("must not be an array of strings"),
            "code_edit_summarizer prompt must match preexisting_changed_files object schema"
        );
    }

    fn code_agent_model_output_requirements(root: &Path) -> BTreeMap<String, BTreeSet<String>> {
        let mut requirements = BTreeMap::<String, BTreeSet<String>>::new();
        for relative in [
            "examples/code-agent/fixtures/code-dynamic-explore.air.yaml",
            "examples/code-agent/code-edit-loop.air.yaml",
            "examples/code-agent/code-explore.air.yaml",
            "examples/code-agent/fixtures/code-review-analyze.air.yaml",
            "examples/code-agent/code-review.air.yaml",
        ] {
            let module = air_parser::parse_air_file(root.join(relative)).unwrap();
            for (alias, required) in model_output_required_keys_by_alias(&module) {
                requirements.entry(alias).or_default().extend(required);
            }
        }
        requirements
    }

    fn model_output_required_keys_by_alias(
        module: &air_core::AirModule,
    ) -> BTreeMap<String, BTreeSet<String>> {
        let mut by_alias = BTreeMap::<String, BTreeSet<String>>::new();
        let Workflow::StateMachine(workflow) = &module.workflow else {
            return by_alias;
        };
        for rule in &workflow.rules {
            for action in &rule.actions {
                let StateAction::ModelCall { model, output, .. } = action else {
                    continue;
                };
                let required = state_field_required_keys(module, output);
                if !required.is_empty() {
                    by_alias.entry(model.clone()).or_default().extend(required);
                }
            }
        }
        by_alias
    }

    fn state_field_required_keys(module: &air_core::AirModule, output: &str) -> BTreeSet<String> {
        match module.state.get(output) {
            Some(TypeSpec::Detailed(schema)) => schema.required.iter().cloned().collect(),
            _ => BTreeSet::new(),
        }
    }
}
