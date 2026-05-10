use crate::run_plan::{run_plan, RunPlanOptions};
use anyhow::Result;
use serde_json::{Map, Value};
use std::path::PathBuf;

pub(crate) struct CodeOptions {
    pub(crate) task: String,
    pub(crate) target: PathBuf,
    pub(crate) test: String,
    pub(crate) query: Option<String>,
    pub(crate) related: Vec<PathBuf>,
    pub(crate) profile: PathBuf,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) state_out: Option<PathBuf>,
    pub(crate) checkpoint_out: Option<PathBuf>,
    pub(crate) jit_cache: Option<PathBuf>,
    pub(crate) parallel: bool,
    pub(crate) log: bool,
    pub(crate) tool_config: Option<PathBuf>,
}

pub(crate) fn code(options: CodeOptions) -> Result<()> {
    let CodeOptions {
        task,
        target,
        test,
        query,
        related,
        profile,
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        state_out,
        checkpoint_out,
        jit_cache,
        parallel,
        log,
        tool_config,
    } = options;

    let mut input = Map::new();
    input.insert("task".to_string(), Value::String(task.clone()));
    input.insert("query".to_string(), Value::String(query.unwrap_or(task)));
    input.insert(
        "target_path".to_string(),
        Value::String(path_to_input_string(target)),
    );
    input.insert(
        "related_files".to_string(),
        Value::Array(
            related
                .into_iter()
                .map(path_to_input_string)
                .map(Value::String)
                .collect(),
        ),
    );
    input.insert("test_command".to_string(), Value::String(test));

    run_plan(RunPlanOptions {
        plan: None,
        profile: Some(profile),
        store: None,
        input: None,
        input_values: Some(input),
        model_config,
        trace_out,
        trace_redact,
        trace_raw,
        state_out,
        checkpoint_out,
        jit_cache,
        parallel,
        log,
        example_tools: false,
        tool_config,
    })
}

fn path_to_input_string(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}
