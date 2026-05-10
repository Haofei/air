use air_backend_openai::{OpenAiCompatibleConfig, OpenAiCompatibleModelProvider};
use air_runtime::{ModelProvider, RuntimeError};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
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
    Ok(config.fixtures)
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
}
