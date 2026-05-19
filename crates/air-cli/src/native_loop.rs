use crate::models::{ModelProviderChoice, ModelReplayOptions};
use crate::tools::ToolProviderChoice;
use crate::trace_io::{observe_event_with_trace_file, write_trace};
use air_runtime::{
    system_return_event, take_last_within_bytes_value, ModelProvider, RuntimeError, ToolProvider,
    TraceEvent, TraceStatus,
};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DECIDER_MODEL: &str = "code_edit_decider";
const PROJECT_SCOUT_DECIDER_MODEL: &str = "project_scout_decider";
const PROJECT_SCOUT_SUMMARIZER_MODEL: &str = "project_scout_summarizer";
const MODEL_TIMEOUT: u64 = 1800;
const TOOL_TIMEOUT: u64 = 1800;
const SKILL_TIMEOUT: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeLoopKind {
    CodeEdit,
    Explore,
    ProjectScout,
    Review,
    Bench,
}

pub(crate) struct NativeLoopOptions {
    pub(crate) kind: NativeLoopKind,
    pub(crate) task: String,
    pub(crate) verification_command: Option<String>,
    pub(crate) requires_edit: bool,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) tool_config: Option<PathBuf>,
    pub(crate) model_replay: Option<ModelReplayOptions>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
}

pub(crate) fn run_native_loop(options: NativeLoopOptions) -> Result<Value> {
    let NativeLoopOptions {
        kind,
        task,
        verification_command,
        requires_edit,
        model_config,
        tool_config,
        model_replay,
        trace_out,
        trace_redact,
        trace_raw,
        log,
    } = options;
    let spec = NativeLoopSpec::for_kind(kind);
    let _model_config_env = EnvVarGuard::set_if_present(
        "AIR_CODE_MODEL_CONFIG",
        model_config.as_ref().map(|path| path.display().to_string()),
    );
    let mut models = build_model_provider(model_config, log, model_replay)?;
    let mut tools = ToolProviderChoice::from_config(tool_config, false)?;
    let mut runner = NativeLoopRunner {
        spec,
        task,
        verification_command: verification_command.unwrap_or_default(),
        requires_edit,
        models: &mut models,
        tools: &mut tools,
        trace_out,
        trace_redact: trace_redact || !trace_raw,
        log,
        trace: Vec::new(),
        step: 0,
    };
    runner.run()
}

pub(crate) fn run_native_project_scout(
    task: String,
    model_config: PathBuf,
    tool_config: PathBuf,
    trace_out: PathBuf,
) -> Result<Value> {
    run_native_loop(NativeLoopOptions {
        kind: NativeLoopKind::ProjectScout,
        task,
        verification_command: None,
        requires_edit: false,
        model_config: Some(model_config),
        tool_config: Some(tool_config),
        model_replay: None,
        trace_out: Some(trace_out),
        trace_redact: false,
        trace_raw: true,
        log: false,
    })
}

fn build_model_provider(
    model_config: Option<PathBuf>,
    log: bool,
    model_replay: Option<ModelReplayOptions>,
) -> Result<ModelProviderChoice> {
    let models = if let Some(model_config) = model_config {
        ModelProviderChoice::from_config_file_with_provider_io(model_config, log)?
    } else {
        ModelProviderChoice::echo()
    };
    if let Some(model_replay) = model_replay {
        models.with_replay_prefix(model_replay)
    } else {
        Ok(models)
    }
}

#[derive(Debug, Clone, Copy)]
struct NativeLoopSpec {
    agent: &'static str,
    completion: CompletionMode,
    max_steps: u32,
    model_name: &'static str,
    completion_model_name: Option<&'static str>,
    observation_bytes: usize,
    dispatch_max_calls: usize,
    allowed_tools: &'static [&'static str],
    verify_allowed_tools: &'static [&'static str],
    bootstrap_skills: bool,
    project_scout_summary_step: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionMode {
    Edit,
    Result,
    Review,
    Bench,
}

impl CompletionMode {
    fn output_key(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Result => "result",
            Self::Review => "review",
            Self::Bench => "bench",
        }
    }
}

impl NativeLoopSpec {
    fn for_kind(kind: NativeLoopKind) -> Self {
        match kind {
            NativeLoopKind::CodeEdit => Self {
                agent: "code",
                completion: CompletionMode::Edit,
                max_steps: 220,
                model_name: DECIDER_MODEL,
                completion_model_name: None,
                observation_bytes: 50_000,
                dispatch_max_calls: 64,
                allowed_tools: &[
                    "bash",
                    "grep",
                    "glob",
                    "read_contains",
                    "read_range",
                    "edit",
                ],
                verify_allowed_tools: &["bash"],
                bootstrap_skills: true,
                project_scout_summary_step: None,
            },
            NativeLoopKind::Explore => Self {
                agent: "code-explore",
                completion: CompletionMode::Result,
                max_steps: 80,
                model_name: DECIDER_MODEL,
                completion_model_name: None,
                observation_bytes: 200_000,
                dispatch_max_calls: 32,
                allowed_tools: &[
                    "question",
                    "grep",
                    "glob",
                    "lsp",
                    "read_contains",
                    "read_range",
                    "webfetch",
                    "skill",
                ],
                verify_allowed_tools: &[],
                bootstrap_skills: false,
                project_scout_summary_step: None,
            },
            NativeLoopKind::ProjectScout => Self {
                agent: "project-scout",
                completion: CompletionMode::Result,
                max_steps: 200,
                model_name: PROJECT_SCOUT_DECIDER_MODEL,
                completion_model_name: Some(PROJECT_SCOUT_SUMMARIZER_MODEL),
                observation_bytes: 200_000,
                dispatch_max_calls: 16,
                allowed_tools: &[
                    "question",
                    "grep",
                    "glob",
                    "lsp",
                    "read_contains",
                    "read_range",
                    "webfetch",
                    "skill",
                ],
                verify_allowed_tools: &[],
                bootstrap_skills: false,
                project_scout_summary_step: Some(24),
            },
            NativeLoopKind::Review => Self {
                agent: "review",
                completion: CompletionMode::Review,
                max_steps: 160,
                model_name: DECIDER_MODEL,
                completion_model_name: None,
                observation_bytes: 600_000,
                dispatch_max_calls: 64,
                allowed_tools: &[
                    "question",
                    "grep",
                    "glob",
                    "read_contains",
                    "read_range",
                    "webfetch",
                    "skill",
                    "gitlab_mr",
                    "gitlab_mr_files",
                    "gitlab_mr_diffs",
                ],
                verify_allowed_tools: &[],
                bootstrap_skills: true,
                project_scout_summary_step: None,
            },
            NativeLoopKind::Bench => Self {
                agent: "bench",
                completion: CompletionMode::Bench,
                max_steps: 180,
                model_name: DECIDER_MODEL,
                completion_model_name: None,
                observation_bytes: 600_000,
                dispatch_max_calls: 64,
                allowed_tools: &[
                    "question",
                    "bash",
                    "grep",
                    "glob",
                    "read_contains",
                    "read_range",
                    "webfetch",
                    "skill",
                ],
                verify_allowed_tools: &[],
                bootstrap_skills: true,
                project_scout_summary_step: None,
            },
        }
    }
}

struct NativeLoopRunner<'a> {
    spec: NativeLoopSpec,
    task: String,
    verification_command: String,
    requires_edit: bool,
    models: &'a mut ModelProviderChoice,
    tools: &'a mut ToolProviderChoice,
    trace_out: Option<PathBuf>,
    trace_redact: bool,
    log: bool,
    trace: Vec<TraceEvent>,
    step: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationStatus {
    Unknown,
    Passed,
    Failed,
}

#[derive(Debug)]
struct RunMemory {
    history: Vec<Value>,
    available_skills: Value,
    last_decision: Value,
    edit: EditProgress,
}

impl RunMemory {
    fn push_feedback(&mut self, action: &str, result: impl Into<Value>) {
        self.history.push(json!({
            "action": action,
            "result": result.into(),
        }));
    }

    fn record_tool_result(&mut self, calls: &[ToolCall], observation: Value) {
        self.history.push(json!({
            "action": "tool_result",
            "assistant": self.last_decision,
            "requested": calls.iter().map(|call| call.raw.clone()).collect::<Vec<_>>(),
            "result": observation,
        }));
    }
}

impl Default for RunMemory {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            available_skills: Value::Null,
            last_decision: Value::Null,
            edit: EditProgress::default(),
        }
    }
}

#[derive(Debug)]
struct EditProgress {
    verification_status: VerificationStatus,
    verification_failed_count: u8,
    patch_seen: bool,
}

impl Default for EditProgress {
    fn default() -> Self {
        Self {
            verification_status: VerificationStatus::Unknown,
            verification_failed_count: 0,
            patch_seen: false,
        }
    }
}

#[derive(Debug)]
struct Decision {
    raw: Value,
    complete: Option<bool>,
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone)]
struct ToolCall {
    raw: Value,
    tool: String,
    input: Value,
}

impl NativeLoopRunner<'_> {
    fn run(&mut self) -> Result<Value> {
        let mut memory = RunMemory::default();
        if self.spec.bootstrap_skills {
            memory.available_skills = self.call_skill_route()?;
        }

        loop {
            if self.step >= self.spec.max_steps {
                memory.history.push(json!({
                    "action": "step_budget_reached",
                    "result": {
                        "runtime": self.runtime_context(),
                        "verification_status": verification_status_value(memory.edit.verification_status)
                    }
                }));
                return self.finish(&memory);
            }
            if let Some(summary_step) = self.spec.project_scout_summary_step {
                if self.step >= summary_step {
                    let result = self.call_project_scout_summarizer(&memory)?;
                    return self.finish_with_output(result);
                }
            }

            let decision = self.call_decider(&memory, false)?;
            memory.last_decision = decision.raw.clone();
            if decision.complete.is_none() {
                memory.push_feedback(
                    "malformed_decision",
                    "The prior decision was missing complete/tool_calls. Return valid structured output before tool execution.",
                );
                continue;
            }
            if decision.tool_calls.is_none() {
                memory.push_feedback(
                    "malformed_decision",
                    "The prior decision was missing tool_calls. Return valid structured output before tool execution.",
                );
                continue;
            }

            if decision.complete == Some(true) {
                match self.spec.completion {
                    CompletionMode::Edit => {
                        if self.edit_can_finish(&mut memory)? {
                            return self.finish(&memory);
                        }
                        continue;
                    }
                    CompletionMode::Result => {
                        let result = json!({
                            "answer": assistant_content(&decision.raw),
                            "decision": decision.raw,
                        });
                        return self.finish_with_output(result);
                    }
                    CompletionMode::Review | CompletionMode::Bench => return self.finish(&memory),
                }
            }

            let calls = decision.tool_calls.unwrap_or_default();
            let summary = self.run_tool_calls(
                &mut memory,
                &calls,
                self.spec.allowed_tools,
                self.spec.dispatch_max_calls,
            )?;
            if self.spec.completion == CompletionMode::Edit
                && update_edit_progress_from_tools(&mut memory, &summary)
            {
                return self.finish(&memory);
            }
        }
    }

    fn edit_can_finish(&mut self, memory: &mut RunMemory) -> Result<bool> {
        if memory.edit.verification_status != VerificationStatus::Passed {
            memory.push_feedback(
                "verification_required",
                "Run an appropriate bash verification command before finishing.",
            );
            let Some(summary) = self.run_verification(memory)? else {
                return Ok(false);
            };
            if apply_verification_gate(memory, &summary) {
                return Ok(true);
            }
            if memory.edit.verification_status == VerificationStatus::Passed {
                if self.requires_edit && !memory.edit.patch_seen {
                    memory.push_feedback(
                        "patch_required",
                        "The task requires a source change, but no workspace patch has been observed. Apply the required edit before finishing.",
                    );
                    return Ok(false);
                }
                return Ok(true);
            }
            return Ok(false);
        }
        if self.requires_edit && !memory.edit.patch_seen {
            memory.push_feedback(
                "patch_required",
                "The task requires a source change, but no workspace patch has been observed. Apply the required edit before finishing.",
            );
            return Ok(false);
        }
        Ok(true)
    }

    fn run_verification(&mut self, memory: &mut RunMemory) -> Result<Option<ToolBatchSummary>> {
        if !self.verification_command.trim().is_empty() {
            let call = ToolCall {
                raw: json!({
                    "tool": "bash",
                    "input": {
                        "command": self.verification_command,
                        "description": "Run deterministic verification command",
                    }
                }),
                tool: "bash".to_string(),
                input: json!({
                    "command": self.verification_command,
                    "description": "Run deterministic verification command",
                }),
            };
            return self
                .run_tool_calls(memory, &[call], self.spec.verify_allowed_tools, 1)
                .map(Some);
        }

        let decision = self.call_decider(memory, true)?;
        memory.last_decision = decision.raw.clone();
        if decision.complete.is_none() || decision.tool_calls.is_none() {
            memory.push_feedback(
                "malformed_decision",
                "The verification decision was missing complete/tool_calls. Return exactly one bash verification command.",
            );
            return Ok(None);
        }
        let calls = decision.tool_calls.unwrap_or_default();
        self.run_tool_calls(
            memory,
            &calls,
            self.spec.verify_allowed_tools,
            self.spec.dispatch_max_calls,
        )
        .map(Some)
    }

    fn run_tool_calls(
        &mut self,
        memory: &mut RunMemory,
        calls: &[ToolCall],
        allowed_tools: &[&str],
        max_calls: usize,
    ) -> Result<ToolBatchSummary> {
        let (observation, summary) = self.dispatch_tool_batch(calls, allowed_tools, max_calls)?;
        memory.record_tool_result(calls, observation);
        Ok(summary)
    }

    fn call_skill_route(&mut self) -> Result<Value> {
        let input = json!({
            "route_task": self.task,
            "top_k": 3,
        });
        let started = Instant::now();
        self.emit(TraceEvent {
            agent: self.spec.agent.to_string(),
            step: self.step,
            rule: "bootstrap".to_string(),
            action: "tool_call_start".to_string(),
            input: Some(input.clone()),
            output: None,
            meta: Some(json!({
                "tool": "skill",
                "timeout_seconds": SKILL_TIMEOUT,
            })),
            status: TraceStatus::Ok,
            error: None,
        });
        let result =
            self.tools
                .call_tool_with_timeout("skill", &input, Duration::from_secs(SKILL_TIMEOUT));
        match result {
            Ok(output) => {
                self.emit(TraceEvent {
                    agent: self.spec.agent.to_string(),
                    step: self.step,
                    rule: "bootstrap".to_string(),
                    action: "tool_call".to_string(),
                    input: Some(input),
                    output: Some(output.clone()),
                    meta: Some(json!({
                        "tool": "skill",
                        "timeout_seconds": SKILL_TIMEOUT,
                        "elapsed_ms": started.elapsed().as_millis(),
                    })),
                    status: TraceStatus::Ok,
                    error: None,
                });
                self.step = self.step.saturating_add(1);
                Ok(output)
            }
            Err(error) => {
                self.emit(TraceEvent {
                    agent: self.spec.agent.to_string(),
                    step: self.step,
                    rule: "bootstrap".to_string(),
                    action: "tool_call".to_string(),
                    input: Some(input),
                    output: None,
                    meta: Some(json!({
                        "tool": "skill",
                        "timeout_seconds": SKILL_TIMEOUT,
                        "elapsed_ms": started.elapsed().as_millis(),
                    })),
                    status: TraceStatus::Error,
                    error: Some(error.to_string()),
                });
                self.step = self.step.saturating_add(1);
                Ok(json!({
                    "kind": "skill_route_error",
                    "error": error.to_string(),
                    "skills": [],
                }))
            }
        }
    }

    fn call_decider(&mut self, memory: &RunMemory, verification_gate: bool) -> Result<Decision> {
        let mut input = Map::new();
        input.insert("task".to_string(), Value::String(self.task.clone()));
        if self.spec.completion == CompletionMode::Review {
            input.insert(
                "review_mode".to_string(),
                Value::String(
                    "Read-only review: use evidence tools only; do not edit files, run tests, or post remote comments."
                        .to_string(),
                ),
            );
        }
        if self.spec.completion == CompletionMode::Bench {
            input.insert(
                "bench_mode".to_string(),
                Value::String(
                    "Benchmark-analysis mode: run controlled experiments in isolated temp workdirs, collect trace metrics, and recommend default/optional/reject. Keep the source workspace unchanged."
                        .to_string(),
                ),
            );
        }
        input.insert(
            "observations".to_string(),
            compact_observations(
                &memory.history,
                if verification_gate {
                    30_000
                } else {
                    self.spec.observation_bytes
                },
            ),
        );
        if !memory.available_skills.is_null() {
            input.insert(
                "available_skills".to_string(),
                memory.available_skills.clone(),
            );
        }
        input.insert(
            "allowed_tools".to_string(),
            Value::Array(
                if verification_gate {
                    self.spec.verify_allowed_tools
                } else {
                    self.spec.allowed_tools
                }
                .iter()
                .map(|tool| Value::String((*tool).to_string()))
                .collect(),
            ),
        );
        if verification_gate {
            input.insert(
                "verification_gate".to_string(),
                json!({
                    "required": true,
                    "instruction": "Run exactly one real verification command such as cargo test, cargo check, cargo clippy, npm test, pytest, or another test/check/lint command. Do not inspect files, grep, print source, or run exploratory commands in the verification gate."
                }),
            );
        } else if self.spec.completion == CompletionMode::Edit {
            input.insert(
                "verification_status".to_string(),
                Value::String(
                    verification_status_value(memory.edit.verification_status).to_string(),
                ),
            );
        }
        let output = self.call_model(self.spec.model_name, Value::Object(input), "model")?;
        Ok(parse_decision(output))
    }

    fn call_project_scout_summarizer(&mut self, memory: &RunMemory) -> Result<Value> {
        let input = json!({
            "task": self.task,
            "observations": compact_observations(&memory.history, self.spec.observation_bytes),
        });
        let model = self
            .spec
            .completion_model_name
            .unwrap_or(PROJECT_SCOUT_SUMMARIZER_MODEL);
        self.call_model(model, input, "summary")
    }

    fn call_model(&mut self, model: &str, input: Value, rule: &str) -> Result<Value> {
        let attempt = 1u32;
        let input_bytes = serde_json::to_vec(&input)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        self.emit(TraceEvent {
            agent: self.spec.agent.to_string(),
            step: self.step,
            rule: rule.to_string(),
            action: "model_call_start".to_string(),
            input: Some(input.clone()),
            output: None,
            meta: Some(json!({
                "model": model,
                "attempt": attempt,
                "max_attempts": 1,
                "output": "decision",
                "timeout_seconds": MODEL_TIMEOUT,
                "input_bytes": input_bytes,
            })),
            status: TraceStatus::Ok,
            error: None,
        });
        let started = Instant::now();
        let result =
            self.models
                .call_model_with_timeout(model, &input, Duration::from_secs(MODEL_TIMEOUT));
        let provider_stats = self.models.take_last_request_stats();
        let mut meta = json!({
            "model": model,
            "attempt": attempt,
            "max_attempts": 1,
            "output": "decision",
            "timeout_seconds": MODEL_TIMEOUT,
            "input_bytes": input_bytes,
            "elapsed_ms": started.elapsed().as_millis(),
            "will_retry": false,
        });
        if let Some(stats) = provider_stats {
            if let Some(object) = meta.as_object_mut() {
                object.insert(
                    "provider_request_bytes".to_string(),
                    json!(stats.provider_request_bytes),
                );
                object.insert(
                    "provider_user_content_bytes".to_string(),
                    json!(stats.provider_user_content_bytes),
                );
                object.insert(
                    "provider_tools_bytes".to_string(),
                    json!(stats.provider_tools_bytes),
                );
                if let Some(request) = stats.provider_request {
                    object.insert("provider_request".to_string(), request);
                }
                if let Some(response) = stats.provider_response {
                    object.insert("provider_response".to_string(), response);
                }
            }
        }
        match result {
            Ok(output) => {
                self.emit(TraceEvent {
                    agent: self.spec.agent.to_string(),
                    step: self.step,
                    rule: rule.to_string(),
                    action: "model_call".to_string(),
                    input: Some(input),
                    output: Some(output.clone()),
                    meta: Some(meta),
                    status: TraceStatus::Ok,
                    error: None,
                });
                self.step = self.step.saturating_add(1);
                Ok(output)
            }
            Err(error) => {
                self.emit(TraceEvent {
                    agent: self.spec.agent.to_string(),
                    step: self.step,
                    rule: rule.to_string(),
                    action: "model_call".to_string(),
                    input: Some(input),
                    output: None,
                    meta: Some(meta),
                    status: TraceStatus::Error,
                    error: Some(error.to_string()),
                });
                Err(anyhow::anyhow!(error.to_string()))
            }
        }
    }

    fn dispatch_tool_batch(
        &mut self,
        calls: &[ToolCall],
        allowed_tools: &[&str],
        max_calls: usize,
    ) -> Result<(Value, ToolBatchSummary)> {
        let mut results = Vec::new();
        let attempted = calls.len();
        if attempted > max_calls {
            let error = format!(
                "tool_batch_dispatch exceeded max_calls={max_calls} with attempted={attempted}"
            );
            results.push(tool_batch_level_error_observation(calls, &error));
            let summary = summarize_tool_batch(&results);
            self.emit(TraceEvent {
                agent: self.spec.agent.to_string(),
                step: self.step,
                rule: "tools".to_string(),
                action: "tool_batch_dispatch".to_string(),
                input: Some(Value::Array(
                    calls.iter().map(|call| call.raw.clone()).collect(),
                )),
                output: Some(Value::Array(results.clone())),
                meta: Some(json!({
                    "count": attempted,
                    "max_calls": max_calls,
                    "error_count": summary.error_count,
                    "summary": summary.as_value(),
                })),
                status: TraceStatus::Ok,
                error: None,
            });
            self.step = self.step.saturating_add(1);
            return Ok((Value::Array(results), summary));
        }

        for (index, call) in calls.iter().enumerate() {
            if !allowed_tools.contains(&call.tool.as_str()) {
                let error = format!(
                    "tool {} is not allowed in this loop; allowed tools are {}",
                    call.tool,
                    allowed_tools.join(", ")
                );
                self.emit_tool_error(index, call, &error, Some(allowed_tools));
                results.push(tool_batch_error_observation(
                    &call.tool,
                    &call.input,
                    &error,
                ));
                continue;
            }

            let started = Instant::now();
            self.emit(TraceEvent {
                agent: self.spec.agent.to_string(),
                step: self.step,
                rule: "tools".to_string(),
                action: "tool_batch_dispatch_item_start".to_string(),
                input: Some(call.input.clone()),
                output: None,
                meta: Some(json!({
                    "tool": call.tool,
                    "index": index,
                    "attempt": 1,
                    "max_attempts": 1,
                    "timeout_seconds": TOOL_TIMEOUT,
                })),
                status: TraceStatus::Ok,
                error: None,
            });
            let result = self.tools.call_tool_with_timeout(
                &call.tool,
                &call.input,
                Duration::from_secs(TOOL_TIMEOUT),
            );
            match result {
                Ok(output) => {
                    self.emit(TraceEvent {
                        agent: self.spec.agent.to_string(),
                        step: self.step,
                        rule: "tools".to_string(),
                        action: "tool_batch_dispatch_item".to_string(),
                        input: Some(call.input.clone()),
                        output: Some(output.clone()),
                        meta: Some(json!({
                            "tool": call.tool,
                            "index": index,
                            "attempt": 1,
                            "max_attempts": 1,
                            "timeout_seconds": TOOL_TIMEOUT,
                            "elapsed_ms": started.elapsed().as_millis(),
                            "will_retry": false,
                        })),
                        status: TraceStatus::Ok,
                        error: None,
                    });
                    results.push(json!({
                        "tool": call.tool,
                        "input": call.input,
                        "status": "ok",
                        "error_code": Value::Null,
                        "output": output,
                    }));
                }
                Err(error) => {
                    let error = error.to_string();
                    self.emit_tool_error(index, call, &error, None);
                    results.push(tool_batch_error_observation(
                        &call.tool,
                        &call.input,
                        &error,
                    ));
                }
            }
        }

        let summary = summarize_tool_batch(&results);
        self.emit(TraceEvent {
            agent: self.spec.agent.to_string(),
            step: self.step,
            rule: "tools".to_string(),
            action: "tool_batch_dispatch".to_string(),
            input: Some(Value::Array(
                calls.iter().map(|call| call.raw.clone()).collect(),
            )),
            output: Some(Value::Array(results.clone())),
            meta: Some(json!({
                "count": attempted,
                "max_calls": max_calls,
                "error_count": summary.error_count,
                "summary": summary.as_value(),
            })),
            status: TraceStatus::Ok,
            error: None,
        });
        self.step = self.step.saturating_add(1);
        Ok((Value::Array(results), summary))
    }

    fn emit_tool_error(
        &mut self,
        index: usize,
        call: &ToolCall,
        error: &str,
        allowed_tools: Option<&[&str]>,
    ) {
        let mut meta = json!({
            "tool": call.tool,
            "index": index,
            "attempt": 1,
            "max_attempts": 1,
            "timeout_seconds": TOOL_TIMEOUT,
            "will_retry": false,
        });
        if let Some(allowed_tools) = allowed_tools {
            if let Some(object) = meta.as_object_mut() {
                object.insert(
                    "allowed_tools".to_string(),
                    Value::Array(
                        allowed_tools
                            .iter()
                            .map(|tool| Value::String((*tool).to_string()))
                            .collect(),
                    ),
                );
            }
        }
        self.emit(TraceEvent {
            agent: self.spec.agent.to_string(),
            step: self.step,
            rule: "tools".to_string(),
            action: "tool_batch_dispatch_item".to_string(),
            input: Some(call.input.clone()),
            output: None,
            meta: Some(meta),
            status: TraceStatus::Error,
            error: Some(error.to_string()),
        });
    }

    fn finish(&mut self, memory: &RunMemory) -> Result<Value> {
        let output = match self.spec.completion {
            CompletionMode::Edit => json!({
                "initial_success": true,
                "final_success": memory.edit.verification_status == VerificationStatus::Passed,
                "patch_applied": memory.edit.patch_seen,
                "changed_files": [],
                "workspace_changed_files": [],
                "preexisting_changed_files": [],
                "workspace_diff": {
                    "provider": "native-rust-loop",
                    "diff": "",
                },
                "rationale": "Completed by AIR native Rust code loop.",
                "assistant": memory.last_decision,
                "verification_status": verification_status_value(memory.edit.verification_status),
                "observations": compact_observations(&memory.history, self.spec.observation_bytes),
            }),
            CompletionMode::Review => json!({
                "review_completed": true,
                "final_success": true,
                "changed_files": [],
                "assistant": memory.last_decision,
                "findings": [],
                "rationale": "Review completed by AIR native review loop; see assistant content and trace evidence for findings.",
            }),
            CompletionMode::Bench => json!({
                "completed": true,
                "final_success": true,
                "assistant": memory.last_decision,
                "experiments": [],
                "recommendation": "See assistant answer and trace evidence for benchmark recommendation.",
                "rationale": "Benchmark analysis completed by AIR native bench loop; trace contains commands, metrics extraction, and recommendation evidence.",
            }),
            CompletionMode::Result => json!({
                "answer": assistant_content(&memory.last_decision),
                "decision": memory.last_decision,
            }),
        };
        self.finish_with_output(output)
    }

    fn finish_with_output(&mut self, output: Value) -> Result<Value> {
        let mut outputs = Map::new();
        outputs.insert(self.spec.completion.output_key().to_string(), output);
        let outputs = Value::Object(outputs.clone());
        let return_event =
            system_return_event(outputs.clone().as_object().cloned().unwrap_or_default());
        self.emit(return_event);
        if let Some(path) = &self.trace_out {
            write_trace(path, &self.trace, self.trace_redact)
                .with_context(|| format!("write trace {}", path.display()))?;
        }
        Ok(outputs)
    }

    fn runtime_context(&self) -> Value {
        json!({
            "step": self.step,
            "max_steps": self.spec.max_steps,
            "is_last_action_step": self.step.saturating_add(1) >= self.spec.max_steps,
        })
    }

    fn emit(&mut self, event: TraceEvent) {
        observe_event_with_trace_file(
            &event,
            self.log,
            &mut self.trace,
            self.trace_out.as_ref(),
            self.trace_redact,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationStatusUpdate {
    Unchanged,
    Unknown,
    Passed,
    Failed,
}

#[derive(Debug)]
struct ToolBatchSummary {
    tool_count: usize,
    error_count: usize,
    any_workspace_change: bool,
    workspace_change_tools: Vec<String>,
    any_verification_passed: bool,
    any_verification_failed: bool,
    verification_tools: Vec<String>,
    verification_status_update: VerificationStatusUpdate,
}

impl ToolBatchSummary {
    fn as_value(&self) -> Value {
        json!({
            "tool_count": self.tool_count,
            "error_count": self.error_count,
            "any_workspace_change": self.any_workspace_change,
            "workspace_change_tools": self.workspace_change_tools,
            "any_verification_passed": self.any_verification_passed,
            "any_verification_failed": self.any_verification_failed,
            "verification_tools": self.verification_tools,
            "verification_status_update": match self.verification_status_update {
                VerificationStatusUpdate::Unchanged => "unchanged",
                VerificationStatusUpdate::Unknown => "unknown",
                VerificationStatusUpdate::Passed => "passed",
                VerificationStatusUpdate::Failed => "failed",
            },
        })
    }
}

fn summarize_tool_batch(results: &[Value]) -> ToolBatchSummary {
    let mut summary = ToolBatchSummary {
        tool_count: results.len(),
        error_count: 0,
        any_workspace_change: false,
        workspace_change_tools: Vec::new(),
        any_verification_passed: false,
        any_verification_failed: false,
        verification_tools: Vec::new(),
        verification_status_update: VerificationStatusUpdate::Unchanged,
    };
    for item in results {
        if item.get("status").and_then(Value::as_str) == Some("error") {
            summary.error_count += 1;
        }
        let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
        let output = item.get("output").unwrap_or(&Value::Null);
        if output
            .get("workspace_changed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            summary.any_workspace_change = true;
            summary.workspace_change_tools.push(tool.to_string());
            summary.verification_status_update = VerificationStatusUpdate::Unknown;
        }
        if output
            .get("verification")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            summary.verification_tools.push(tool.to_string());
            let verification_success = output
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let workspace_changed = output
                .get("workspace_changed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if verification_success && !workspace_changed {
                summary.any_verification_passed = true;
                summary.verification_status_update = VerificationStatusUpdate::Passed;
            } else if !verification_success {
                summary.any_verification_failed = true;
                summary.verification_status_update = VerificationStatusUpdate::Failed;
            }
        }
    }
    summary
}

fn update_edit_progress_from_tools(memory: &mut RunMemory, summary: &ToolBatchSummary) -> bool {
    if summary.error_count > 0
        && summary.verification_status_update != VerificationStatusUpdate::Failed
    {
        memory.push_feedback(
            "tool_error",
            "One or more requested tool calls failed. Inspect the observed errors and choose the next valid action.",
        );
    }
    apply_edit_summary(memory, summary)
}

fn apply_verification_gate(memory: &mut RunMemory, summary: &ToolBatchSummary) -> bool {
    match summary.verification_status_update {
        VerificationStatusUpdate::Unchanged => {
            memory.push_feedback(
                "verification_required",
                "Verification must be a real test/check/lint command. Exploratory bash commands are not verification.",
            );
            true
        }
        VerificationStatusUpdate::Unknown => {
            memory.edit.patch_seen |= summary.any_workspace_change;
            memory.edit.verification_status = VerificationStatus::Unknown;
            memory.push_feedback(
                "verification_required_but_mutated",
                "Verification changed the workspace instead of giving a clean verification result.",
            );
            true
        }
        VerificationStatusUpdate::Passed | VerificationStatusUpdate::Failed => {
            apply_edit_summary(memory, summary)
        }
    }
}

fn apply_edit_summary(memory: &mut RunMemory, summary: &ToolBatchSummary) -> bool {
    match summary.verification_status_update {
        VerificationStatusUpdate::Passed => {
            memory.edit.verification_status = VerificationStatus::Passed;
            memory.edit.verification_failed_count = 0;
            memory.edit.patch_seen |= summary.any_workspace_change;
        }
        VerificationStatusUpdate::Failed => {
            if memory.edit.verification_failed_count > 0
                || memory.edit.verification_status == VerificationStatus::Failed
            {
                memory.push_feedback(
                    "verification_failed_cap",
                    "Verification failed again after a prior failed verification; summarize with the failing command/output instead of looping.",
                );
                memory.edit.verification_status = VerificationStatus::Failed;
                return true;
            }
            memory.edit.verification_status = VerificationStatus::Failed;
            memory.edit.verification_failed_count =
                memory.edit.verification_failed_count.saturating_add(1);
        }
        VerificationStatusUpdate::Unknown => {
            if summary.any_workspace_change {
                memory.edit.patch_seen = true;
                memory.edit.verification_status = VerificationStatus::Unknown;
            }
        }
        VerificationStatusUpdate::Unchanged => {}
    }
    false
}

fn parse_decision(value: Value) -> Decision {
    let complete = value.get("complete").and_then(Value::as_bool);
    let tool_calls = value
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(parse_tool_call)
                .collect::<std::result::Result<Vec<_>, RuntimeError>>()
                .unwrap_or_else(|error| {
                    vec![ToolCall {
                        raw: json!({
                            "tool": "",
                            "input": {},
                            "parse_error": error.to_string(),
                        }),
                        tool: String::new(),
                        input: json!({}),
                    }]
                })
        });
    Decision {
        raw: value,
        complete,
        tool_calls,
    }
}

fn parse_tool_call(value: &Value) -> Result<ToolCall, RuntimeError> {
    let tool = value
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| RuntimeError::Provider("tool call is missing tool".to_string()))?;
    let input = value
        .get("input")
        .cloned()
        .ok_or_else(|| RuntimeError::Provider("tool call is missing input".to_string()))?;
    Ok(ToolCall {
        raw: value.clone(),
        tool: tool.to_string(),
        input,
    })
}

fn tool_batch_error_observation(tool: &str, input: &Value, error: &str) -> Value {
    json!({
        "tool": tool,
        "input": input,
        "status": "error",
        "error_code": "runtime_error",
        "error": error,
        "output": Value::Null,
    })
}

fn tool_batch_level_error_observation(calls: &[ToolCall], error: &str) -> Value {
    json!({
        "tool": "tool_batch_dispatch",
        "input": Value::Array(calls.iter().map(|call| call.raw.clone()).collect()),
        "status": "error",
        "error_code": "batch_error",
        "error": error,
        "output": Value::Null,
    })
}

fn verification_status_value(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Unknown => "unknown",
        VerificationStatus::Passed => "passed",
        VerificationStatus::Failed => "failed",
    }
}

fn assistant_content(decision: &Value) -> String {
    decision
        .pointer("/_air_assistant/content")
        .and_then(Value::as_str)
        .or_else(|| decision.get("answer").and_then(Value::as_str))
        .or_else(|| decision.get("rationale").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

fn compact_observations(observations: &[Value], max_bytes: usize) -> Value {
    let value = Value::Array(observations.to_vec());
    take_last_within_bytes_value(&value, max_bytes).unwrap_or(value)
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_if_present(key: &'static str, value: Option<String>) -> Option<Self> {
        value.map(|value| {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        })
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_batch_summary_marks_clean_verification_passed() {
        let summary = summarize_tool_batch(&[json!({
            "tool": "bash",
            "status": "ok",
            "output": {
                "verification": true,
                "success": true,
                "workspace_changed": false,
            }
        })]);

        assert_eq!(
            summary.verification_status_update,
            VerificationStatusUpdate::Passed
        );
        assert!(summary.any_verification_passed);
    }

    #[test]
    fn tool_batch_summary_resets_verification_after_workspace_change() {
        let summary = summarize_tool_batch(&[json!({
            "tool": "edit",
            "status": "ok",
            "output": {
                "workspace_changed": true,
            }
        })]);

        assert_eq!(
            summary.verification_status_update,
            VerificationStatusUpdate::Unknown
        );
        assert!(summary.any_workspace_change);
    }

    #[test]
    fn code_edit_spec_has_native_allowed_tool_boundary() {
        let spec = NativeLoopSpec::for_kind(NativeLoopKind::CodeEdit);
        assert_eq!(spec.completion.output_key(), "edit");
        assert!(spec.allowed_tools.contains(&"edit"));
        assert!(!spec.allowed_tools.contains(&"todowrite"));
        assert!(!spec.allowed_tools.contains(&"todoread"));
        assert_eq!(spec.verify_allowed_tools, &["bash"]);
    }
}
