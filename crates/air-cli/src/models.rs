use air_backend_openai::{OpenAiCompatibleConfig, OpenAiCompatibleModelProvider};
use air_runtime::{ModelProvider, RuntimeError};
use anyhow::Result;
use serde_json::{json, Value};
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
pub(crate) enum ModelProviderChoice {
    Echo(EchoModels),
    OpenAi(OpenAiCompatibleModelProvider),
}

impl ModelProviderChoice {
    pub(crate) fn echo() -> Self {
        Self::Echo(EchoModels)
    }

    pub(crate) fn openai(config: OpenAiCompatibleConfig) -> Result<Self> {
        Ok(Self::OpenAi(OpenAiCompatibleModelProvider::new(config)?))
    }
}

impl ModelProvider for ModelProviderChoice {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match self {
            ModelProviderChoice::Echo(provider) => provider.call_model(name, input),
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
            ModelProviderChoice::OpenAi(provider) => {
                provider.call_model_with_timeout(name, input, timeout)
            }
        }
    }
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
