use air_backend_openai::{OpenAiCompatibleConfig, OpenAiCompatibleModelProvider};
use air_runtime::{
    read_trace_jsonl, ModelProvider, ModelRequestStats, RuntimeError, TraceEvent, TraceStatus,
};
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
    Replay(ReplayModels),
}

#[derive(Clone)]
pub(crate) struct ReplayModels {
    live: Box<ModelProviderChoice>,
    calls: Vec<ReplayModelCall>,
    live_from_event: Option<usize>,
    call_index: usize,
    path_rewrites: Vec<(String, String)>,
    last_request_stats: Option<ModelRequestStats>,
}

#[derive(Clone)]
struct ReplayModelCall {
    event_index: usize,
    model: Option<String>,
    output: Option<Value>,
    error: Option<String>,
    stats: Option<ModelRequestStats>,
}

#[derive(Clone)]
pub(crate) struct ModelReplayOptions {
    pub(crate) trace: PathBuf,
    pub(crate) live_from_event: Option<usize>,
    pub(crate) path_rewrites: Vec<(String, String)>,
}

impl ModelProviderChoice {
    pub(crate) fn echo() -> Self {
        Self::Echo(EchoModels)
    }

    pub(crate) fn openai(config: OpenAiCompatibleConfig) -> Result<Self> {
        Ok(Self::OpenAi(OpenAiCompatibleModelProvider::new(config)?))
    }

    pub(crate) fn from_config_file_with_provider_io(
        path: PathBuf,
        force_provider_io: bool,
    ) -> Result<Self> {
        if let Some(fixtures) = parse_fixture_model_config(&path)? {
            return Ok(Self::Fixture(FixtureModels { fixtures }));
        }
        let mut config = air_backend_openai::parse_config_file(path)?;
        if force_provider_io {
            force_trace_provider_io(&mut config);
        }
        Self::openai(config)
    }

    pub(crate) fn with_replay_prefix(self, options: ModelReplayOptions) -> Result<Self> {
        let events = read_trace_jsonl(&options.trace).map_err(|error| {
            anyhow::anyhow!("read replay trace {}: {error}", options.trace.display())
        })?;
        Ok(Self::Replay(ReplayModels {
            live: Box::new(self),
            calls: replay_model_calls(&events)?,
            live_from_event: options.live_from_event,
            call_index: 0,
            path_rewrites: options.path_rewrites,
            last_request_stats: None,
        }))
    }
}

fn force_trace_provider_io(config: &mut OpenAiCompatibleConfig) {
    for model in config.models.values_mut() {
        model.trace_provider_io = Some(true);
    }
}

impl ModelProvider for ModelProviderChoice {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match self {
            ModelProviderChoice::Echo(provider) => provider.call_model(name, input),
            ModelProviderChoice::Fixture(provider) => provider.call_model(name, input),
            ModelProviderChoice::OpenAi(provider) => provider.call_model(name, input),
            ModelProviderChoice::Replay(provider) => provider.call_model(name, input),
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
            ModelProviderChoice::Replay(provider) => {
                provider.call_model_with_timeout(name, input, timeout)
            }
        }
    }

    fn take_last_request_stats(&mut self) -> Option<air_runtime::ModelRequestStats> {
        match self {
            ModelProviderChoice::Echo(provider) => provider.take_last_request_stats(),
            ModelProviderChoice::Fixture(provider) => provider.take_last_request_stats(),
            ModelProviderChoice::OpenAi(provider) => provider.take_last_request_stats(),
            ModelProviderChoice::Replay(provider) => provider.take_last_request_stats(),
        }
    }
}

impl ModelProvider for ReplayModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        self.call_model_with_timeout(name, input, Duration::from_secs(0))
    }

    fn call_model_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        self.call_index = self.call_index.saturating_add(1);
        let Some(call) = self.calls.get(self.call_index - 1).cloned() else {
            return self.call_live_model(name, input, timeout);
        };
        if self
            .live_from_event
            .is_none_or(|from_event| call.event_index < from_event)
        {
            if let Some(model) = call.model.as_deref() {
                if model != name {
                    return Err(RuntimeError::Provider(format!(
                        "model replay call {} expected model {model}, got {name}",
                        self.call_index
                    )));
                }
            }
            self.last_request_stats = call
                .stats
                .map(|stats| rewrite_model_request_stats(stats, &self.path_rewrites));
            if let Some(error) = call.error {
                return Err(RuntimeError::Provider(error));
            }
            let Some(output) = call.output else {
                return Err(RuntimeError::Provider(format!(
                    "model replay call {} has no output",
                    self.call_index
                )));
            };
            return Ok(rewrite_value(output, &self.path_rewrites));
        }
        self.call_live_model(name, input, timeout)
    }

    fn take_last_request_stats(&mut self) -> Option<ModelRequestStats> {
        self.last_request_stats.take()
    }
}

impl ReplayModels {
    fn call_live_model(
        &mut self,
        name: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        let result = self.live.call_model_with_timeout(name, input, timeout);
        self.last_request_stats = self.live.take_last_request_stats();
        result
    }
}

fn replay_model_calls(events: &[TraceEvent]) -> Result<Vec<ReplayModelCall>> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| event.action == "model_call")
        .map(|(index, event)| {
            let meta = event.meta.as_ref();
            let model = meta
                .and_then(|meta| meta.get("model"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let stats = meta.map(model_request_stats_from_meta).transpose()?;
            let error = (event.status == TraceStatus::Error)
                .then(|| event.error.clone())
                .flatten();
            Ok(ReplayModelCall {
                event_index: index + 1,
                model,
                output: event.output.clone(),
                error,
                stats,
            })
        })
        .collect()
}

fn model_request_stats_from_meta(meta: &Value) -> Result<ModelRequestStats> {
    let provider_request = meta.get("provider_request").cloned();
    let provider_response = meta.get("provider_response").cloned();
    Ok(ModelRequestStats {
        provider_request_bytes: meta
            .get("provider_request_bytes")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .or_else(|| {
                provider_request
                    .as_ref()
                    .and_then(|value| serde_json::to_vec(value).ok())
                    .map(|bytes| bytes.len())
            })
            .unwrap_or(0),
        provider_user_content_bytes: meta
            .get("provider_user_content_bytes")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(0),
        provider_tools_bytes: meta
            .get("provider_tools_bytes")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(0),
        provider_request,
        provider_response,
    })
}

fn rewrite_model_request_stats(
    mut stats: ModelRequestStats,
    rewrites: &[(String, String)],
) -> ModelRequestStats {
    stats.provider_request = stats
        .provider_request
        .map(|value| rewrite_value(value, rewrites));
    stats.provider_response = stats
        .provider_response
        .map(|value| rewrite_value(value, rewrites));
    stats
}

fn rewrite_value(value: Value, rewrites: &[(String, String)]) -> Value {
    match value {
        Value::String(mut string) => {
            for (from, to) in rewrites {
                string = string.replace(from, to);
            }
            Value::String(string)
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| rewrite_value(value, rewrites))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, rewrite_value(value, rewrites)))
                .collect(),
        ),
        value => value,
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

        let mut provider =
            ModelProviderChoice::from_config_file_with_provider_io(path, false).unwrap();
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

        let mut provider =
            ModelProviderChoice::from_config_file_with_provider_io(path, false).unwrap();
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

        let mut provider =
            ModelProviderChoice::from_config_file_with_provider_io(path, false).unwrap();
        let first = provider.call_model("planner", &json!({})).unwrap();
        let second = provider.call_model("planner", &json!({})).unwrap();
        let third = provider.call_model("planner", &json!({})).unwrap();

        assert_eq!(first["step"], json!(1));
        assert_eq!(second["step"], json!(2));
        assert_eq!(third["step"], json!(2));
    }

    #[test]
    fn force_trace_provider_io_enables_all_model_aliases() {
        let mut config = OpenAiCompatibleConfig {
            models: BTreeMap::from([
                (
                    "decider".to_string(),
                    air_backend_openai::OpenAiModelConfig {
                        base_url: None,
                        base_url_env: None,
                        api_key_env: None,
                        model: "glm-5.1".to_string(),
                        model_env: None,
                        temperature: None,
                        request_timeout_seconds: None,
                        system_prompt: None,
                        json_mode: None,
                        response_format: None,
                        extra_body: None,
                        native_tool_calls: None,
                        opencode_tool_mode: None,
                        trace_provider_io: None,
                    },
                ),
                (
                    "summarizer".to_string(),
                    air_backend_openai::OpenAiModelConfig {
                        base_url: None,
                        base_url_env: None,
                        api_key_env: None,
                        model: "glm-5.1".to_string(),
                        model_env: None,
                        temperature: None,
                        request_timeout_seconds: None,
                        system_prompt: None,
                        json_mode: None,
                        response_format: None,
                        extra_body: None,
                        native_tool_calls: None,
                        opencode_tool_mode: None,
                        trace_provider_io: Some(false),
                    },
                ),
            ]),
        };

        force_trace_provider_io(&mut config);

        assert!(config
            .models
            .values()
            .all(|model| model.trace_provider_io == Some(true)));
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
            BTreeSet::from(["complete".to_string(), "tool_calls".to_string()])
        );
        assert!(
            !requirements.contains_key("code_edit_summarizer"),
            "code edit loop should derive final audit fields without a summarizer model call"
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
            edit_prompt == "opencode:qwen",
            "code_edit_decider should use the OpenCode-style provider prompt marker"
        );
        assert!(
            !edit_prompt.contains("complete (boolean)")
                && !edit_prompt.contains("tool_calls (array)")
                && !edit_prompt.contains("complete=true"),
            "code_edit_decider prompt should not expose AIR's outer completion protocol in native tool mode"
        );
    }

    fn code_agent_model_output_requirements(root: &Path) -> BTreeMap<String, BTreeSet<String>> {
        let mut requirements = BTreeMap::<String, BTreeSet<String>>::new();
        for relative in ["skills/code-agent/code-edit-loop.air.yaml"] {
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
