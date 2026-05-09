use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub(crate) struct RunPlanProfile {
    pub(crate) plan: PathBuf,
    pub(crate) store: PathBuf,

    #[serde(default)]
    pub(crate) input_file: Option<PathBuf>,

    #[serde(default)]
    pub(crate) inputs: Option<BTreeMap<String, Value>>,

    #[serde(default)]
    pub(crate) model_config: Option<PathBuf>,

    #[serde(default)]
    pub(crate) tool_config: Option<PathBuf>,

    #[serde(default)]
    pub(crate) trace_out: Option<PathBuf>,

    #[serde(default)]
    pub(crate) trace_redact: Option<bool>,

    #[serde(default)]
    pub(crate) state_out: Option<PathBuf>,

    #[serde(default)]
    pub(crate) checkpoint_out: Option<PathBuf>,

    #[serde(default)]
    pub(crate) jit_cache: Option<PathBuf>,

    #[serde(default)]
    pub(crate) parallel: Option<bool>,

    #[serde(default)]
    pub(crate) log: Option<bool>,

    #[serde(default)]
    pub(crate) example_tools: Option<bool>,
}

pub(crate) fn read_json_object(
    path: PathBuf,
    label: &str,
) -> Result<serde_json::Map<String, Value>> {
    let source = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {label} JSON file {}", path.display()))?;
    match serde_json::from_str::<Value>(&source)
        .with_context(|| format!("failed to parse {label} JSON file {}", path.display()))?
    {
        Value::Object(object) => Ok(object),
        other => anyhow::bail!(
            "{label} JSON file {} must contain a JSON object, got {}",
            path.display(),
            json_kind(&other)
        ),
    }
}

pub(crate) fn read_run_plan_profile(path: &PathBuf) -> Result<RunPlanProfile> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read run profile {}", path.display()))?;
    serde_yaml::from_str(&source)
        .with_context(|| format!("failed to parse run profile YAML {}", path.display()))
}

pub(crate) fn resolve_profile_path(profile_path: &Path, path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path.clone();
    }
    profile_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(path)
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
