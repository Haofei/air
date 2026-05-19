use crate::models::{ModelProviderChoice, ModelReplayOptions};
use crate::tools::ToolProviderChoice;
use crate::trace_io::{observe_event_with_trace_file, write_trace};
use air_runtime::{
    compact_model_context_value, system_return_event, take_last_within_bytes_value, ModelProvider,
    RuntimeError, ToolProvider, TraceEvent, TraceStatus,
};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
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
                    "task",
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
    changed_files: BTreeSet<String>,
    verification_commands: Vec<String>,
    last_tool_error: Option<Value>,
}

impl EditProgress {
    fn as_value(&self) -> Value {
        json!({
            "verification_status": verification_status_value(self.verification_status),
            "verification_failed_count": self.verification_failed_count,
            "patch_seen": self.patch_seen,
            "changed_files": self.changed_files.iter().cloned().collect::<Vec<_>>(),
            "verification_commands": self.verification_commands.iter().rev().take(4).cloned().collect::<Vec<_>>(),
            "last_tool_error": self.last_tool_error,
        })
    }

    fn record_summary(&mut self, summary: &ToolBatchSummary) {
        for path in &summary.changed_files {
            self.changed_files.insert(path.clone());
        }
        for command in &summary.verification_commands {
            if !self.verification_commands.contains(command) {
                self.verification_commands.push(command.clone());
            }
        }
        if let Some(error) = summary.tool_errors.first() {
            self.last_tool_error = Some(error.clone());
        }
    }
}

impl Default for EditProgress {
    fn default() -> Self {
        Self {
            verification_status: VerificationStatus::Unknown,
            verification_failed_count: 0,
            patch_seen: false,
            changed_files: BTreeSet::new(),
            verification_commands: Vec::new(),
            last_tool_error: None,
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

struct ModelContextBuilder {
    input: Map<String, Value>,
}

impl ModelContextBuilder {
    fn for_native_loop(
        runner: &NativeLoopRunner<'_>,
        memory: &RunMemory,
        verification_gate: bool,
    ) -> Self {
        let allowed_tools = if verification_gate {
            runner.spec.verify_allowed_tools
        } else {
            runner.spec.allowed_tools
        };
        let observation_bytes = if verification_gate {
            30_000
        } else {
            runner.spec.observation_bytes
        };
        let mut input = Map::new();
        input.insert("task".to_string(), Value::String(runner.task.clone()));
        input.insert("runtime".to_string(), runner.runtime_context());
        input.insert(
            "environment_context".to_string(),
            runner.environment_context(),
        );
        input.insert(
            "completion_policy".to_string(),
            runner.completion_policy(memory, verification_gate),
        );
        input.insert(
            "model_runtime".to_string(),
            serde_json::to_value(
                runner
                    .models
                    .model_runtime_info(runner.spec.model_name)
                    .unwrap_or_default(),
            )
            .unwrap_or_else(|_| json!({})),
        );
        input.insert(
            "context_policy".to_string(),
            Self::context_policy(observation_bytes),
        );
        input.insert(
            "protected_facts".to_string(),
            Self::protected_facts(runner, memory, verification_gate),
        );
        input.insert(
            "observations".to_string(),
            compact_observations(&memory.history, observation_bytes),
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
                allowed_tools
                    .iter()
                    .map(|tool| Value::String((*tool).to_string()))
                    .collect(),
            ),
        );
        input.insert(
            "tool_capabilities".to_string(),
            runner.tool_capabilities(allowed_tools),
        );
        if runner.spec.completion == CompletionMode::Review {
            input.insert(
                "review_mode".to_string(),
                Value::String(
                    "Read-only review: use evidence tools only; do not edit files, run tests, or post remote comments."
                        .to_string(),
                ),
            );
        }
        if runner.spec.completion == CompletionMode::Bench {
            input.insert(
                "bench_mode".to_string(),
                Value::String(
                    "Benchmark-analysis mode: run controlled experiments in isolated temp workdirs, collect trace metrics, and recommend default/optional/reject. Keep the source workspace unchanged."
                        .to_string(),
                ),
            );
        }
        if verification_gate {
            input.insert(
                "verification_gate".to_string(),
                json!({
                    "required": true,
                    "instruction": "Run exactly one real verification command such as cargo test, cargo check, cargo clippy, npm test, pytest, or another test/check/lint command. Do not inspect files, grep, print source, or run exploratory commands in the verification gate."
                }),
            );
        } else if runner.spec.completion == CompletionMode::Edit {
            input.insert("edit_progress".to_string(), memory.edit.as_value());
            input.insert(
                "verification_status".to_string(),
                Value::String(
                    verification_status_value(memory.edit.verification_status).to_string(),
                ),
            );
        }
        Self { input }
    }

    fn context_policy(observation_bytes: usize) -> Value {
        json!({
            "builder": "air.native.model_context.v1",
            "observation_bytes": observation_bytes,
            "history_normalization": {
                "pairs_tool_calls_with_outputs": true,
                "drops_orphan_tool_outputs": true,
                "inserts_synthetic_aborted_outputs": true,
                "removes_invalid_tool_call_few_shots": true,
                "compacts_tool_outputs_for_model": true
            },
            "protected_keys": [
                "task",
                "runtime",
                "environment_context",
                "completion_policy",
                "protected_facts",
                "tool_capabilities",
                "edit_progress"
            ]
        })
    }

    fn protected_facts(
        runner: &NativeLoopRunner<'_>,
        memory: &RunMemory,
        verification_gate: bool,
    ) -> Value {
        json!({
            "task": runner.task.clone(),
            "mode": runner.spec.completion.output_key(),
            "verification_gate": verification_gate,
            "requires_edit": runner.requires_edit,
            "edit_ledger": memory.edit.as_value(),
            "last_feedback": memory.history.iter().rev().find(|item| {
                item.get("action")
                    .and_then(Value::as_str)
                    .is_some_and(|action| action.ends_with("required") || action.contains("error") || action.contains("failed"))
            }).cloned().unwrap_or(Value::Null),
        })
    }

    fn build(self) -> Value {
        Value::Object(self.input)
    }
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
            let action = model_turn_action(decision);
            match action {
                ModelTurnAction::RespondToModel { action, result } => {
                    memory.push_feedback(action, result);
                    continue;
                }
                ModelTurnAction::Complete => {
                    if let Some(output) = self.handle_completion(&mut memory)? {
                        return Ok(output);
                    }
                    continue;
                }
                ModelTurnAction::DispatchTools(calls) => {
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
        }
    }

    fn handle_completion(&mut self, memory: &mut RunMemory) -> Result<Option<Value>> {
        match self.spec.completion {
            CompletionMode::Edit => {
                if self.edit_can_finish(memory)? {
                    return self.finish(memory).map(Some);
                }
                Ok(None)
            }
            CompletionMode::Result => {
                let result = json!({
                    "answer": assistant_content(&memory.last_decision),
                    "decision": memory.last_decision,
                });
                self.finish_with_output(result).map(Some)
            }
            CompletionMode::Review | CompletionMode::Bench => self.finish(memory).map(Some),
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
        match verification_turn_action(decision) {
            ModelTurnAction::RespondToModel { action, result } => {
                memory.push_feedback(action, result);
                Ok(None)
            }
            ModelTurnAction::Complete => {
                memory.push_feedback(
                    "verification_required",
                    "Return exactly one bash verification command before completing.",
                );
                Ok(None)
            }
            ModelTurnAction::DispatchTools(calls) => self
                .run_tool_calls(
                    memory,
                    &calls,
                    self.spec.verify_allowed_tools,
                    self.spec.dispatch_max_calls,
                )
                .map(Some),
        }
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
        let input = ModelContextBuilder::for_native_loop(self, memory, verification_gate).build();
        let output = self.call_model(self.spec.model_name, input, "model")?;
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
                if let Some(object) = meta.as_object_mut() {
                    object.insert("error_kind".to_string(), json!("fatal"));
                    object.insert("recoverable".to_string(), json!(false));
                }
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
                "changed_files": memory.edit.changed_files.iter().cloned().collect::<Vec<_>>(),
                "workspace_changed_files": memory.edit.changed_files.iter().cloned().collect::<Vec<_>>(),
                "preexisting_changed_files": [],
                "workspace_diff": {
                    "provider": "native-rust-loop",
                    "diff": "",
                },
                "rationale": "Completed by AIR native Rust code loop.",
                "assistant": memory.last_decision,
                "verification_status": verification_status_value(memory.edit.verification_status),
                "edit_ledger": memory.edit.as_value(),
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
        let health = native_loop_trace_health(&self.trace);
        self.emit(TraceEvent {
            agent: "$system".to_string(),
            step: self.step,
            rule: "audit".to_string(),
            action: "trace_health".to_string(),
            input: None,
            output: Some(health),
            meta: Some(json!({
                "schema": "air.native_trace_health.v1",
            })),
            status: TraceStatus::Ok,
            error: None,
        });
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
            "remaining_steps": self.spec.max_steps.saturating_sub(self.step),
            "is_last_action_step": self.step.saturating_add(1) >= self.spec.max_steps,
        })
    }

    fn environment_context(&self) -> Value {
        json!({
            "cwd": std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            "shell": std::env::var("SHELL").unwrap_or_default(),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "path_separator": std::path::MAIN_SEPARATOR.to_string(),
        })
    }

    fn tool_capabilities(&self, allowed_tools: &[&str]) -> Value {
        let mut tools = Map::new();
        for tool in allowed_tools {
            let info = self
                .tools
                .tool_runtime_info(tool)
                .unwrap_or_else(|| air_runtime::ToolRuntimeInfo::generic(None));
            let mut value = serde_json::to_value(info).unwrap_or_else(|_| json!({}));
            if let Some(object) = value.as_object_mut() {
                object.insert("allowed".to_string(), Value::Bool(true));
                object.insert("name".to_string(), Value::String((*tool).to_string()));
            }
            tools.insert((*tool).to_string(), value);
        }
        Value::Object(tools)
    }

    fn completion_policy(&self, memory: &RunMemory, verification_gate: bool) -> Value {
        let can_finish_edit = memory.edit.verification_status == VerificationStatus::Passed
            && (!self.requires_edit || memory.edit.patch_seen);
        json!({
            "mode": self.spec.completion.output_key(),
            "verification_gate": verification_gate,
            "requires_edit": self.spec.completion == CompletionMode::Edit && self.requires_edit,
            "requires_verification": self.spec.completion == CompletionMode::Edit,
            "can_finish_now": match self.spec.completion {
                CompletionMode::Edit => can_finish_edit,
                CompletionMode::Result | CompletionMode::Review | CompletionMode::Bench => true,
            },
            "finish_requirements": match self.spec.completion {
                CompletionMode::Edit => json!({
                    "verification_status": "passed",
                    "patch_seen": if self.requires_edit { "required" } else { "not_required" },
                }),
                _ => json!({}),
            },
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
    changed_files: Vec<String>,
    any_verification_passed: bool,
    any_verification_failed: bool,
    verification_tools: Vec<String>,
    verification_commands: Vec<String>,
    verification_status_update: VerificationStatusUpdate,
    tool_errors: Vec<Value>,
}

impl ToolBatchSummary {
    fn as_value(&self) -> Value {
        json!({
            "tool_count": self.tool_count,
            "error_count": self.error_count,
            "any_workspace_change": self.any_workspace_change,
            "workspace_change_tools": self.workspace_change_tools,
            "changed_files": self.changed_files,
            "any_verification_passed": self.any_verification_passed,
            "any_verification_failed": self.any_verification_failed,
            "verification_tools": self.verification_tools,
            "verification_commands": self.verification_commands,
            "tool_errors": self.tool_errors,
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
        changed_files: Vec::new(),
        any_verification_passed: false,
        any_verification_failed: false,
        verification_tools: Vec::new(),
        verification_commands: Vec::new(),
        verification_status_update: VerificationStatusUpdate::Unchanged,
        tool_errors: Vec::new(),
    };
    for item in results {
        if item.get("status").and_then(Value::as_str) == Some("error") {
            summary.error_count += 1;
            summary.tool_errors.push(tool_error_detail(item));
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
            collect_changed_files(output, &mut summary.changed_files);
            summary.verification_status_update = VerificationStatusUpdate::Unknown;
        }
        if output
            .get("verification")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            summary.verification_tools.push(tool.to_string());
            if let Some(command) = item
                .get("input")
                .and_then(|input| input.get("command"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|command| !command.is_empty())
            {
                push_unique(&mut summary.verification_commands, command.to_string());
            }
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

fn collect_changed_files(output: &Value, changed_files: &mut Vec<String>) {
    if let Some(path) = output
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
    {
        push_unique(changed_files, path.to_string());
    }
    for key in ["changed_files", "workspace_changed_files", "files"] {
        if let Some(items) = output.get(key).and_then(Value::as_array) {
            for item in items {
                if let Some(path) = item.as_str() {
                    push_unique(changed_files, path.to_string());
                } else if let Some(path) = item.get("path").and_then(Value::as_str) {
                    push_unique(changed_files, path.to_string());
                }
            }
        }
    }
    if let Some(snippets) = output.get("post_edit_snippets").and_then(Value::as_array) {
        for snippet in snippets {
            if let Some(path) = snippet.get("path").and_then(Value::as_str) {
                push_unique(changed_files, path.to_string());
            }
        }
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn tool_error_detail(item: &Value) -> Value {
    let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
    let output = item.get("output").unwrap_or(&Value::Null);
    let error = item
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| output.get("error").and_then(Value::as_str))
        .or_else(|| output.get("message").and_then(Value::as_str))
        .unwrap_or("tool call failed");
    let error_code = item
        .get("error_code")
        .filter(|value| !value.is_null())
        .cloned()
        .or_else(|| output.get("error_code").cloned())
        .or_else(|| output.get("permission").cloned());
    let error_kind = item
        .get("error_kind")
        .and_then(Value::as_str)
        .unwrap_or("recoverable");
    let mut detail = Map::new();
    detail.insert("tool".to_string(), Value::String(tool.to_string()));
    detail.insert("error".to_string(), Value::String(error.to_string()));
    detail.insert(
        "error_kind".to_string(),
        Value::String(error_kind.to_string()),
    );
    detail.insert(
        "recoverable".to_string(),
        Value::Bool(
            item.get("recoverable")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
    );
    if let Some(error_code) = error_code {
        detail.insert("error_code".to_string(), error_code);
    }
    if let Some(input) = item.get("input") {
        detail.insert("input".to_string(), input.clone());
    }
    Value::Object(detail)
}

fn update_edit_progress_from_tools(memory: &mut RunMemory, summary: &ToolBatchSummary) -> bool {
    if summary.error_count > 0
        && summary.verification_status_update != VerificationStatusUpdate::Failed
    {
        memory.push_feedback("tool_error", tool_error_feedback(summary));
    }
    apply_edit_summary(memory, summary)
}

fn tool_error_feedback(summary: &ToolBatchSummary) -> Value {
    json!({
        "message": "One or more requested tool calls failed. Fix the first listed error before retrying; do not repeat the same invalid call.",
        "failed_tool_calls": summary.tool_errors.iter().take(3).cloned().collect::<Vec<_>>(),
    })
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
    memory.edit.record_summary(summary);
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

enum ModelTurnAction {
    RespondToModel { action: &'static str, result: Value },
    Complete,
    DispatchTools(Vec<ToolCall>),
}

fn model_turn_action(decision: Decision) -> ModelTurnAction {
    if let Some(feedback) = invalid_tool_call_feedback(&decision.raw) {
        return ModelTurnAction::RespondToModel {
            action: "malformed_tool_call",
            result: feedback,
        };
    }
    if decision.complete.is_none() {
        return ModelTurnAction::RespondToModel {
            action: "malformed_decision",
            result: Value::String(
                "The prior decision was missing complete/tool_calls. Return valid structured output before tool execution."
                    .to_string(),
            ),
        };
    }
    let Some(calls) = decision.tool_calls else {
        return ModelTurnAction::RespondToModel {
            action: "malformed_decision",
            result: Value::String(
                "The prior decision was missing tool_calls. Return valid structured output before tool execution."
                    .to_string(),
            ),
        };
    };
    if decision.complete == Some(true) {
        return ModelTurnAction::Complete;
    }
    if calls.is_empty() {
        return ModelTurnAction::RespondToModel {
            action: "malformed_decision",
            result: Value::String(
                "The prior decision set complete=false but did not request any tool calls. Request concrete tool calls, or set complete=true with a final answer."
                    .to_string(),
            ),
        };
    }
    ModelTurnAction::DispatchTools(calls)
}

fn verification_turn_action(decision: Decision) -> ModelTurnAction {
    if let Some(feedback) = invalid_tool_call_feedback(&decision.raw) {
        return ModelTurnAction::RespondToModel {
            action: "malformed_tool_call",
            result: feedback,
        };
    }
    if decision.complete.is_none() || decision.tool_calls.is_none() {
        return ModelTurnAction::RespondToModel {
            action: "malformed_decision",
            result: Value::String(
                "The verification decision was missing complete/tool_calls. Return exactly one bash verification command."
                    .to_string(),
            ),
        };
    }
    let calls = decision.tool_calls.unwrap_or_default();
    if calls.is_empty() {
        return ModelTurnAction::RespondToModel {
            action: "verification_required",
            result: Value::String(
                "Return exactly one bash verification command before completing.".to_string(),
            ),
        };
    }
    ModelTurnAction::DispatchTools(calls)
}

fn invalid_tool_call_feedback(decision: &Value) -> Option<Value> {
    let invalid_calls = decision
        .get("_air_invalid_tool_calls")
        .and_then(Value::as_array)?;
    let valid_count = decision
        .get("tool_calls")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if invalid_calls.is_empty() || valid_count > 0 {
        return None;
    }

    let errors = invalid_calls
        .iter()
        .take(4)
        .map(|call| {
            json!({
                "tool": call.get("name").and_then(Value::as_str).unwrap_or("unknown"),
                "arguments": call.get("arguments").and_then(Value::as_str).unwrap_or(""),
                "error": call.get("error").and_then(Value::as_str).unwrap_or("invalid native tool call"),
            })
        })
        .collect::<Vec<_>>();

    Some(json!({
        "message": "The previous assistant response contained only malformed tool calls, so no tool was executed. Retry with complete, concrete arguments for every required field; tool calls cannot be placeholders.",
        "invalid_tool_calls": errors,
    }))
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
        "error_kind": "recoverable",
        "recoverable": true,
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
        "error_kind": "recoverable",
        "recoverable": true,
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
    let value = Value::Array(normalize_history_for_model(observations));
    take_last_within_bytes_value(&value, max_bytes).unwrap_or(value)
}

fn normalize_history_for_model(observations: &[Value]) -> Vec<Value> {
    observations
        .iter()
        .map(normalize_observation_for_model)
        .collect()
}

fn normalize_observation_for_model(observation: &Value) -> Value {
    let Some(object) = observation.as_object() else {
        return compact_model_context_value(observation);
    };
    let mut normalized = object.clone();
    let mut normalization_meta = None;
    if let (Some(requested), Some(results)) = (
        normalized
            .get("requested")
            .and_then(Value::as_array)
            .cloned(),
        normalized.get("result").and_then(Value::as_array).cloned(),
    ) {
        let requested_len = requested.len();
        let mut normalized_results = Vec::with_capacity(requested_len);
        let mut orphan_outputs = results.len().saturating_sub(requested_len);
        let mut inserted_outputs = 0usize;
        for (index, request) in requested.iter().enumerate() {
            let result = results.get(index);
            if result.is_some_and(|result| tool_result_matches_request(result, request)) {
                normalized_results.push(compact_model_context_value(result.unwrap()));
            } else {
                if result.is_some() {
                    orphan_outputs += 1;
                }
                normalized_results.push(aborted_tool_output_for_request(request));
                inserted_outputs += 1;
            }
        }
        normalized.insert("requested".to_string(), Value::Array(requested));
        normalized.insert("result".to_string(), Value::Array(normalized_results));
        if inserted_outputs > 0 || orphan_outputs > 0 {
            normalization_meta = Some(json!({
                "inserted_aborted_outputs": inserted_outputs,
                "removed_orphan_outputs": orphan_outputs,
            }));
        }
    }
    let mut compacted = compact_model_context_value(&Value::Object(normalized));
    if let (Some(meta), Some(object)) = (normalization_meta, compacted.as_object_mut()) {
        object.insert("_air_history_normalized".to_string(), meta);
    }
    compacted
}

fn tool_result_matches_request(result: &Value, request: &Value) -> bool {
    let Some(request_tool) = request.get("tool").and_then(Value::as_str) else {
        return true;
    };
    result
        .get("tool")
        .and_then(Value::as_str)
        .is_none_or(|result_tool| result_tool == request_tool)
}

fn aborted_tool_output_for_request(request: &Value) -> Value {
    let tool = request
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
    json!({
        "tool": tool,
        "input": input,
        "status": "error",
        "error_code": "aborted",
        "error_kind": "recoverable",
        "recoverable": true,
        "error": "tool output missing from history; treating the call as aborted",
        "output": Value::Null,
    })
}

fn native_loop_trace_health(events: &[TraceEvent]) -> Value {
    let mut model_calls = 0usize;
    let mut invalid_tool_calls = 0usize;
    let mut orphan_tool_outputs = 0usize;
    let mut inserted_aborted_outputs = 0usize;
    let mut empty_tool_call_inputs = 0usize;
    for event in events {
        if event.action == "model_call" {
            model_calls += 1;
            if let Some(invalid) = event
                .output
                .as_ref()
                .and_then(|output| output.get("_air_invalid_tool_calls"))
                .and_then(Value::as_array)
            {
                invalid_tool_calls += invalid.len();
            }
            if let Some(observations) = event
                .input
                .as_ref()
                .and_then(|input| input.get("observations"))
                .and_then(Value::as_array)
            {
                for observation in observations {
                    let Some(object) = observation.as_object() else {
                        continue;
                    };
                    inserted_aborted_outputs += object
                        .get("_air_history_normalized")
                        .and_then(|meta| meta.get("inserted_aborted_outputs"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    orphan_tool_outputs += object
                        .get("_air_history_normalized")
                        .and_then(|meta| meta.get("removed_orphan_outputs"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    if let Some(requested) = object.get("requested").and_then(Value::as_array) {
                        empty_tool_call_inputs += requested
                            .iter()
                            .filter(|request| has_empty_placeholder_input(request))
                            .count();
                    }
                }
            }
        }
        if event.action == "tool_batch_dispatch" {
            let requested = event
                .input
                .as_ref()
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let returned = event
                .output
                .as_ref()
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if returned > requested {
                orphan_tool_outputs += returned - requested;
            }
        }
    }
    let issue_count = invalid_tool_calls
        + orphan_tool_outputs
        + inserted_aborted_outputs
        + empty_tool_call_inputs;
    json!({
        "model_calls": model_calls,
        "invalid_tool_calls": invalid_tool_calls,
        "orphan_tool_outputs": orphan_tool_outputs,
        "inserted_aborted_outputs": inserted_aborted_outputs,
        "empty_tool_call_inputs": empty_tool_call_inputs,
        "issue_count": issue_count,
        "healthy": issue_count == 0,
    })
}

fn has_empty_placeholder_input(request: &Value) -> bool {
    let tool = request
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let input_is_empty = request
        .get("input")
        .and_then(Value::as_object)
        .is_some_and(|object| object.is_empty());
    input_is_empty && !matches!(tool, "todoread")
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
    fn tool_batch_summary_keeps_actionable_error_details() {
        let summary = summarize_tool_batch(&[json!({
            "tool": "edit",
            "status": "error",
            "input": {
                "filePath": "src/lib.rs",
                "oldString": "old",
                "newString": "new"
            },
            "error": "oldString not found in content",
            "output": {
                "error_code": "edit_failed"
            }
        })]);

        assert_eq!(summary.error_count, 1);
        assert_eq!(summary.tool_errors[0]["tool"], json!("edit"));
        assert_eq!(
            summary.tool_errors[0]["error"],
            json!("oldString not found in content")
        );
        assert_eq!(summary.tool_errors[0]["error_code"], json!("edit_failed"));
        assert_eq!(summary.tool_errors[0]["error_kind"], json!("recoverable"));
        assert_eq!(summary.tool_errors[0]["recoverable"], json!(true));

        let feedback = tool_error_feedback(&summary);
        assert!(feedback["message"]
            .as_str()
            .unwrap()
            .contains("Fix the first listed error"));
        assert_eq!(feedback["failed_tool_calls"][0]["tool"], json!("edit"));
    }

    #[test]
    fn edit_progress_is_structured_for_model_input() {
        let progress = EditProgress {
            verification_status: VerificationStatus::Failed,
            verification_failed_count: 1,
            patch_seen: true,
            changed_files: BTreeSet::from(["src/lib.rs".to_string()]),
            verification_commands: vec!["cargo test".to_string()],
            last_tool_error: Some(json!({"tool": "edit", "error": "oldString not found"})),
        };

        let value = progress.as_value();
        assert_eq!(value["verification_status"], json!("failed"));
        assert_eq!(value["verification_failed_count"], json!(1));
        assert_eq!(value["patch_seen"], json!(true));
        assert_eq!(value["changed_files"], json!(["src/lib.rs"]));
        assert_eq!(value["verification_commands"], json!(["cargo test"]));
        assert_eq!(value["last_tool_error"]["tool"], json!("edit"));
    }

    #[test]
    fn completion_policy_exposes_finish_requirements() {
        let mut models = ModelProviderChoice::echo();
        let mut tools = ToolProviderChoice::from_config(None, false).unwrap();
        let runner = NativeLoopRunner {
            spec: NativeLoopSpec::for_kind(NativeLoopKind::CodeEdit),
            task: "fix bug".to_string(),
            verification_command: String::new(),
            requires_edit: true,
            models: &mut models,
            tools: &mut tools,
            trace_out: None,
            trace_redact: true,
            log: false,
            trace: Vec::new(),
            step: 3,
        };
        let mut memory = RunMemory::default();
        memory.edit.verification_status = VerificationStatus::Passed;

        let policy = runner.completion_policy(&memory, false);
        assert_eq!(policy["requires_edit"], json!(true));
        assert_eq!(policy["requires_verification"], json!(true));
        assert_eq!(policy["can_finish_now"], json!(false));
        assert_eq!(
            policy["finish_requirements"]["patch_seen"],
            json!("required")
        );

        memory.edit.patch_seen = true;
        assert_eq!(
            runner.completion_policy(&memory, false)["can_finish_now"],
            json!(true)
        );
        assert_eq!(runner.runtime_context()["remaining_steps"], json!(217));
        assert_eq!(
            runner.environment_context()["os"],
            json!(std::env::consts::OS)
        );

        let model_input = ModelContextBuilder::for_native_loop(&runner, &memory, false).build();
        assert_eq!(
            model_input["context_policy"]["builder"],
            json!("air.native.model_context.v1")
        );
        assert_eq!(model_input["model_runtime"]["provider"], json!("echo"));
        assert_eq!(
            model_input["protected_facts"]["edit_ledger"]["patch_seen"],
            json!(true)
        );
        assert_eq!(
            model_input["tool_capabilities"]["edit"]["permission_profile"],
            json!("echo")
        );
    }

    #[test]
    fn model_history_normalizer_pairs_calls_and_outputs() {
        let normalized = normalize_history_for_model(&[json!({
            "action": "tool_result",
            "requested": [
                {"tool": "grep", "input": {"pattern": "needle"}},
                {"tool": "read_range", "input": {"file": "src/lib.rs", "offset": 0, "limit": 20}}
            ],
            "result": [{
                "tool": "grep",
                "status": "ok",
                "input": {"pattern": "needle"},
                "output": {
                    "matches": ["x"],
                    "large": "line\n".repeat(10_000)
                }
            }, {
                "tool": "orphan",
                "status": "ok",
                "output": {"unused": true}
            }]
        })]);

        let observation = &normalized[0];
        let results = observation["result"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool"], json!("grep"));
        assert_eq!(results[1]["tool"], json!("read_range"));
        assert_eq!(results[1]["status"], json!("error"));
        assert_eq!(results[1]["error_code"], json!("aborted"));
        assert_eq!(
            observation["_air_history_normalized"]["inserted_aborted_outputs"],
            json!(1)
        );
        assert_eq!(
            observation["_air_history_normalized"]["removed_orphan_outputs"],
            json!(1)
        );
    }

    #[test]
    fn tool_batch_summary_records_edit_ledger_facts() {
        let summary = summarize_tool_batch(&[
            json!({
                "tool": "edit",
                "status": "ok",
                "output": {
                    "workspace_changed": true,
                    "path": "src/lib.rs",
                    "post_edit_snippets": [{"path": "src/lib.rs"}]
                }
            }),
            json!({
                "tool": "bash",
                "status": "ok",
                "input": {"command": "cargo test"},
                "output": {
                    "verification": true,
                    "success": true,
                    "workspace_changed": false
                }
            }),
        ]);
        let mut memory = RunMemory::default();

        assert!(!apply_edit_summary(&mut memory, &summary));
        assert!(memory.edit.changed_files.contains("src/lib.rs"));
        assert_eq!(memory.edit.verification_commands, vec!["cargo test"]);
        assert_eq!(memory.edit.verification_status, VerificationStatus::Passed);
    }

    #[test]
    fn native_trace_health_flags_context_pollution() {
        let event = TraceEvent {
            agent: "code".to_string(),
            step: 1,
            rule: "model".to_string(),
            action: "model_call".to_string(),
            input: Some(json!({
                "observations": [{
                    "action": "tool_result",
                    "requested": [{"tool": "edit", "input": {}}],
                    "_air_history_normalized": {
                        "inserted_aborted_outputs": 1,
                        "removed_orphan_outputs": 1
                    }
                }]
            })),
            output: Some(json!({
                "complete": false,
                "tool_calls": [],
                "_air_invalid_tool_calls": [{"name": "edit", "arguments": "{}"}]
            })),
            meta: None,
            status: TraceStatus::Ok,
            error: None,
        };

        let health = native_loop_trace_health(&[event]);
        assert_eq!(health["healthy"], json!(false));
        assert_eq!(health["invalid_tool_calls"], json!(1));
        assert_eq!(health["empty_tool_call_inputs"], json!(1));
        assert_eq!(health["inserted_aborted_outputs"], json!(1));
        assert_eq!(health["orphan_tool_outputs"], json!(1));
    }

    #[test]
    fn code_edit_spec_has_native_allowed_tool_boundary() {
        let spec = NativeLoopSpec::for_kind(NativeLoopKind::CodeEdit);
        assert_eq!(spec.completion.output_key(), "edit");
        assert!(spec.allowed_tools.contains(&"edit"));
        assert!(spec.allowed_tools.contains(&"task"));
        assert!(!spec.allowed_tools.contains(&"todowrite"));
        assert!(!spec.allowed_tools.contains(&"todoread"));
        assert_eq!(spec.verify_allowed_tools, &["bash"]);
    }

    #[test]
    fn invalid_tool_call_feedback_only_when_no_valid_calls_exist() {
        let feedback = invalid_tool_call_feedback(&json!({
            "complete": false,
            "tool_calls": [],
            "_air_invalid_tool_calls": [{
                "name": "edit",
                "arguments": "{}",
                "error": "malformed native tool call for edit: empty arguments {}"
            }]
        }))
        .expect("all-invalid tool calls should produce feedback");

        assert_eq!(feedback["invalid_tool_calls"][0]["tool"], json!("edit"));
        assert!(feedback["message"]
            .as_str()
            .unwrap()
            .contains("no tool was executed"));

        assert!(invalid_tool_call_feedback(&json!({
            "complete": false,
            "tool_calls": [{"tool": "edit", "input": {"filePath": "src/lib.rs", "oldString": "old", "newString": "new"}}],
            "_air_invalid_tool_calls": [{
                "name": "edit",
                "arguments": "{}",
                "error": "malformed native tool call for edit: empty arguments {}"
            }]
        }))
        .is_none());
    }

    #[test]
    fn model_turn_action_matches_codex_follow_up_shape() {
        let malformed = Decision {
            raw: json!({
                "complete": false,
                "tool_calls": [],
                "_air_invalid_tool_calls": [{
                    "name": "edit",
                    "arguments": "{}",
                    "error": "malformed native tool call for edit: empty arguments {}"
                }]
            }),
            complete: Some(false),
            tool_calls: Some(Vec::new()),
        };
        match model_turn_action(malformed) {
            ModelTurnAction::RespondToModel { action, result } => {
                assert_eq!(action, "malformed_tool_call");
                assert!(result["message"]
                    .as_str()
                    .unwrap()
                    .contains("no tool was executed"));
            }
            _ => panic!("malformed tool calls should be model-visible feedback"),
        }

        let tool_call = ToolCall {
            raw: json!({"tool": "bash", "input": {"command": "pwd"}}),
            tool: "bash".to_string(),
            input: json!({"command": "pwd"}),
        };
        let valid = Decision {
            raw: json!({"complete": false, "tool_calls": [tool_call.raw.clone()]}),
            complete: Some(false),
            tool_calls: Some(vec![tool_call]),
        };
        match model_turn_action(valid) {
            ModelTurnAction::DispatchTools(calls) => assert_eq!(calls.len(), 1),
            _ => panic!("valid tool calls should dispatch"),
        }
    }
}
