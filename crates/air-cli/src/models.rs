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
    fixtures: BTreeMap<String, Value>,
}

impl ModelProvider for FixtureModels {
    fn call_model(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        self.fixtures
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::Provider(format!("missing fixture model {name}")))
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

fn parse_fixture_model_config(path: &PathBuf) -> Result<Option<BTreeMap<String, Value>>> {
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
        resolved.insert(name, resolve_fixture_files(fixture, base_dir)?);
    }
    Ok(Some(resolved))
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
                "page_builder": {
                  "content": { "$file": "fixtures/page.html" }
                }
              }
            }"#,
        )
        .unwrap();

        let mut provider = ModelProviderChoice::from_config_file(path).unwrap();
        let output = provider
            .call_model("page_builder", &json!({"ignored": true}))
            .unwrap();
        assert_eq!(output["content"], json!("<!doctype html>\n<html></html>\n"));
        let _ = fs::remove_dir_all(dir);
    }
}
