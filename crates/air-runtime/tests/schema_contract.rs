use air_core::{StateAction, Workflow};
use air_runtime::{
    read_trace_jsonl, replay_outputs, system_return_event, write_trace_jsonl,
    write_trace_jsonl_with_options, ApprovalDecision, ModelProvider, ModelRequestStats,
    RuntimeError, State, ToolProvider, TraceEvent, TraceStatus, TraceWriteOptions, Vm,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

struct SchemaModels {
    extract: Value,
    score: Value,
    report: Value,
}

impl ModelProvider for SchemaModels {
    fn call_model(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "extractor" => Ok(self.extract.clone()),
            "scorer" => Ok(self.score.clone()),
            "reporter" => Ok(self.report.clone()),
            other => panic!("unexpected model {other}"),
        }
    }
}

struct SchemaTools;

impl ToolProvider for SchemaTools {
    fn call_tool(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        panic!("unexpected tool {name}")
    }
}

struct RequestStatsModels {
    output: Value,
    stats: Option<ModelRequestStats>,
}

impl ModelProvider for RequestStatsModels {
    fn call_model(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "extractor");
        Ok(self.output.clone())
    }

    fn take_last_request_stats(&mut self) -> Option<ModelRequestStats> {
        self.stats.take()
    }
}

struct CountingTools {
    calls: usize,
}

impl ToolProvider for CountingTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "docs.search");
        self.calls += 1;
        Ok(json!({
            "call": self.calls,
            "query": input["query"]
        }))
    }
}

struct FailingBatchTools {
    calls: usize,
}

impl ToolProvider for FailingBatchTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "docs.search");
        self.calls += 1;
        if input["query"] == json!("bad") {
            return Err(RuntimeError::Provider("synthetic tool failure".to_string()));
        }
        Ok(json!({
            "call": self.calls,
            "query": input["query"]
        }))
    }
}

struct BatchApprovalTools {
    calls: usize,
}

impl ToolProvider for BatchApprovalTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        self.calls += 1;
        match name {
            "docs.search" => Ok(json!({
                "query": input["query"],
            })),
            "edit" => Ok(json!({
                "applied": true,
                "path": input["path"],
            })),
            other => panic!("unexpected tool {other}"),
        }
    }

    fn request_approval(
        &mut self,
        module: &air_core::AirModule,
        approval_for: &[String],
        _state: &Value,
    ) -> Result<ApprovalDecision, RuntimeError> {
        assert_eq!(module.agent.name, "tool-batch-dispatch-approval-agent");
        assert_eq!(approval_for, &["file.write".to_string()]);
        Ok(ApprovalDecision::approved(
            "test-approver",
            "fixture approval",
        ))
    }
}

struct DispatchModels;

impl ModelProvider for DispatchModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "dispatcher");
        Ok(json!({
            "tool": "docs.search",
            "input": {
                "query": input["text"]
            }
        }))
    }
}

struct NameArgsDispatchModels;

impl ModelProvider for NameArgsDispatchModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "dispatcher");
        Ok(json!({
            "name": "docs.search",
            "arguments": {
                "query": input["text"]
            }
        }))
    }
}

struct ToolArgsDispatchModels;

impl ModelProvider for ToolArgsDispatchModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "dispatcher");
        Ok(json!({
            "tool": "docs.search",
            "args": {
                "query": input["text"]
            }
        }))
    }
}

struct UppercaseDispatchModels;

impl ModelProvider for UppercaseDispatchModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "dispatcher");
        Ok(json!({
            "tool": "DOCS.SEARCH",
            "input": {
                "query": input["text"]
            }
        }))
    }
}

struct UnsafeDispatchModels;

impl ModelProvider for UnsafeDispatchModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "dispatcher");
        Ok(json!({
            "tool": "shell.run",
            "input": {
                "command": input["text"]
            }
        }))
    }
}

struct MalformedDispatchModels;

impl ModelProvider for MalformedDispatchModels {
    fn call_model(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "dispatcher");
        Ok(json!({
            "tool": 42,
            "input": {}
        }))
    }
}

struct BatchDispatchModels {
    choices: Value,
}

impl ModelProvider for BatchDispatchModels {
    fn call_model(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "batch_planner");
        Ok(self.choices.clone())
    }
}

struct TimeoutRecordingModels {
    seen: Rc<RefCell<Option<Duration>>>,
}

impl ModelProvider for TimeoutRecordingModels {
    fn call_model(&mut self, _name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        panic!("runtime should call call_model_with_timeout")
    }

    fn call_model_with_timeout(
        &mut self,
        name: &str,
        _input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        assert_eq!(name, "extractor");
        *self.seen.borrow_mut() = Some(timeout);
        Ok(valid_extracted())
    }
}

struct TimeoutRecordingTools {
    seen: Rc<RefCell<Option<Duration>>>,
}

impl ToolProvider for TimeoutRecordingTools {
    fn call_tool(&mut self, _name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        panic!("runtime should call call_tool_with_timeout")
    }

    fn call_tool_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        assert_eq!(name, "docs.search");
        *self.seen.borrow_mut() = Some(timeout);
        Ok(json!({
            "query": input["query"],
            "documents": []
        }))
    }

    fn tool_capability(&self, name: &str) -> Option<&str> {
        (name == "docs.search").then_some("retrieval.local")
    }
}

struct CapabilityTools {
    capability: &'static str,
}

impl ToolProvider for CapabilityTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "docs.search");
        Ok(json!({"query": input["query"]}))
    }

    fn tool_capability(&self, name: &str) -> Option<&str> {
        (name == "docs.search").then_some(self.capability)
    }
}

struct ArtifactTools;

impl ToolProvider for ArtifactTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "docs.search");
        Ok(json!({
            "query": input["query"],
            "documents": [{
                "id": "doc-1",
                "title": "AIR provenance",
                "content": "AIR tools return artifacts with stable ids."
            }],
            "artifacts": [{
                "id": "doc-1",
                "kind": "doc_chunk",
                "title": "AIR provenance",
                "uri": "local-doc://doc-1",
                "content": "AIR tools return artifacts with stable ids."
            }]
        }))
    }

    fn tool_capability(&self, name: &str) -> Option<&str> {
        (name == "docs.search").then_some("retrieval.local")
    }
}

struct CitationModels {
    sources: Vec<&'static str>,
}

impl ModelProvider for CitationModels {
    fn call_model(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "reporter");
        Ok(json!({
            "summary": "The answer is grounded in retrieved artifacts.",
            "sources": self.sources,
        }))
    }
}

struct ApprovingTools;

impl ToolProvider for ApprovingTools {
    fn call_tool(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        panic!("unexpected tool {name}")
    }

    fn request_approval(
        &mut self,
        module: &air_core::AirModule,
        approval_for: &[String],
        state: &Value,
    ) -> Result<ApprovalDecision, RuntimeError> {
        assert_eq!(module.agent.name, "approval-gate-agent");
        assert_eq!(approval_for, &["production.deploy".to_string()]);
        assert_eq!(state["phase"], json!("approval"));
        Ok(ApprovalDecision::approved(
            "test-approver",
            "fixture approval",
        ))
    }
}

struct DenyingTools;

impl ToolProvider for DenyingTools {
    fn call_tool(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        panic!("unexpected tool {name}")
    }

    fn request_approval(
        &mut self,
        _module: &air_core::AirModule,
        _approval_for: &[String],
        _state: &Value,
    ) -> Result<ApprovalDecision, RuntimeError> {
        Ok(ApprovalDecision::denied("not allowed in this test"))
    }
}

struct CapturingModels {
    seen: Option<Value>,
}

impl ModelProvider for CapturingModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "expr_model");
        self.seen = Some(input.clone());
        Ok(json!({"accepted": true}))
    }
}

struct LoopModels {
    calls: usize,
}

impl ModelProvider for LoopModels {
    fn call_model(&mut self, name: &str, _input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "loop_decider");
        self.calls += 1;
        Ok(json!({
            "complete": self.calls >= 2,
            "query": format!("query {}", self.calls)
        }))
    }
}

struct LimitModels {
    calls: usize,
}

impl ModelProvider for LimitModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "limit_model");
        self.calls += 1;
        Ok(json!({
            "call": self.calls,
            "text": input["text"]
        }))
    }
}

struct RetryModels {
    calls: usize,
}

impl ModelProvider for RetryModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "retry_model");
        self.calls += 1;
        if self.calls == 1 {
            assert!(input.get("_air_retry").is_none());
            Ok(json!({"summary": "missing required rationale"}))
        } else {
            assert!(input["_air_retry"]["previous_error"]
                .as_str()
                .is_some_and(|error| error.contains("result.rationale")));
            assert_eq!(input["_air_retry"]["attempt"], json!(2));
            Ok(json!({
                "summary": "valid retry result",
                "rationale": "second attempt satisfied the schema"
            }))
        }
    }
}

struct ProviderRetryModels {
    calls: usize,
}

impl ModelProvider for ProviderRetryModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "retry_model");
        self.calls += 1;
        if self.calls == 1 {
            assert!(input.get("_air_retry").is_none());
            Err(RuntimeError::Provider(
                "error sending request for url".to_string(),
            ))
        } else {
            assert_eq!(input["_air_retry"]["reason"], json!("provider_error"));
            assert!(input["_air_retry"]["previous_error"]
                .as_str()
                .is_some_and(|error| error.contains("error sending request")));
            assert!(input["_air_retry"]["instruction"]
                .as_str()
                .is_some_and(|instruction| {
                    instruction.contains("provider call failed")
                        && !instruction.contains("schema validation")
                }));
            Ok(json!({
                "summary": "valid provider retry result",
                "rationale": "second attempt recovered after provider error"
            }))
        }
    }
}

struct ContentWrappedModels;

impl ModelProvider for ContentWrappedModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "retry_model");
        assert!(input.get("_air_retry").is_none());
        Ok(json!({
            "content": "{\"summary\":\"wrapped model result\",\"rationale\":\"content parsed into the declared output schema\"}"
        }))
    }
}

struct TokenLimitRetryModels {
    calls: usize,
}

impl ModelProvider for TokenLimitRetryModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "token_retry_model");
        self.calls += 1;
        if self.calls == 1 {
            assert!(input.get("_air_retry").is_none());
            assert!(input["text"].as_str().unwrap().len() > 16);
            Err(RuntimeError::Provider(
                "context_length_exceeded: too many tokens".to_string(),
            ))
        } else {
            assert_eq!(input["_air_retry"]["reason"], json!("token_limit"));
            assert_eq!(input["_air_retry"]["max_input_chars"], json!(16));
            assert!(input["_air_retry"]["previous_error"]
                .as_str()
                .is_some_and(|error| error.contains("context_length_exceeded")));
            assert!(input["text"]
                .as_str()
                .is_some_and(|text| text.contains("[air:truncated]")));
            Ok(json!({
                "summary": "compacted retry result",
                "rationale": "second attempt used bounded input compaction"
            }))
        }
    }
}

#[test]
fn accepts_model_outputs_that_match_schema() {
    let extract = load_agent("tests/agents/schema-extract.air.yaml");
    let score = load_agent("tests/agents/schema-score.air.yaml");
    let report = load_agent("tests/agents/schema-report.air.yaml");
    let mut vm = valid_vm();

    let extract_result = vm
        .run(
            &extract,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
        )
        .unwrap();
    let score_result = vm
        .run(
            &score,
            State::from_iter([
                ("text".to_string(), json!("billing issue")),
                (
                    "extracted".to_string(),
                    extract_result.outputs["extracted"].clone(),
                ),
            ]),
        )
        .unwrap();
    let report_result = vm
        .run(
            &report,
            State::from_iter([
                ("text".to_string(), json!("billing issue")),
                (
                    "extracted".to_string(),
                    extract_result.outputs["extracted"].clone(),
                ),
                ("score".to_string(), score_result.outputs["score"].clone()),
            ]),
        )
        .unwrap();

    assert_eq!(report_result.outputs["report"]["approved"], json!(true));
}

#[test]
fn evaluates_composable_input_expressions() {
    let module = load_agent("tests/agents/expr-input.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: CapturingModels { seen: None },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([
                ("text".to_string(), json!("please fix billing")),
                (
                    "extracted".to_string(),
                    json!({
                        "customer_issue": "double charge",
                        "product_area": "billing"
                    }),
                ),
                ("notes".to_string(), json!(["first", "second", "third"])),
            ]),
        )
        .unwrap();

    assert_eq!(result.outputs["response"]["accepted"], json!(true));
    assert_eq!(
        vm.models.seen.unwrap(),
        json!({
            "text": "please fix billing",
            "issue": "double charge",
            "kind": "triage",
            "tags": ["customer", "billing"],
            "message": "double charge / please fix billing",
            "short_message": "double charge",
            "recent_notes": ["second", "third"],
            "recent_notes_budgeted": ["second", "third"],
            "changed_files": [
                "crates/air-tools/src/lib.rs",
                "examples/code-agent/code-edit-loop.air.yaml"
            ]
        })
    );
}

#[test]
fn evaluates_set_action_expressions() {
    let module = load_agent("tests/agents/set-expr.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: CapturingModels { seen: None },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([
                ("text".to_string(), json!("please fix billing")),
                (
                    "extracted".to_string(),
                    json!({
                        "customer_issue": "double charge",
                        "product_area": "billing"
                    }),
                ),
            ]),
        )
        .unwrap();

    assert_eq!(
        result.outputs["build"],
        json!({
            "message": "double charge / please fix billing",
            "tags": ["customer", "billing"]
        })
    );
    let set_event = result
        .trace
        .iter()
        .find(|event| event.rule == "init" && event.action == "set")
        .expect("set event");
    assert_eq!(
        set_event.output.as_ref().unwrap()["build"]["tags"][1],
        "billing"
    );
}

#[test]
fn evaluates_compound_state_machine_conditions() {
    let module = load_agent("tests/agents/conditional-loop.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: LoopModels { calls: 0 },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("question".to_string(), json!("research readiness"))]),
        )
        .unwrap();

    assert_eq!(result.outputs["decision"]["complete"], json!(true));
    assert_eq!(result.outputs["decision"]["query"], json!("query 2"));
    assert_eq!(vm.models.calls, 2);
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "model_call")
            .count(),
        2
    );
}

#[test]
fn evaluates_disjunctive_state_machine_conditions() {
    let module = load_agent("tests/agents/conditional-or.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: CapturingModels { seen: None },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("question".to_string(), json!("route with or"))]),
        )
        .unwrap();

    assert_eq!(result.outputs["result"]["route"], json!("disjunction"));
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.rule == "route" && event.action == "set")
            .count(),
        1
    );
}

#[test]
fn appends_values_to_state_arrays() {
    let module = load_agent("tests/agents/append-evidence.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: CapturingModels { seen: None },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("item".to_string(), json!("dynamic evidence"))]),
        )
        .unwrap();

    assert_eq!(
        result.outputs["evidence"],
        json!(["dynamic evidence", "static evidence"])
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "append")
            .count(),
        2
    );
}

#[test]
fn enforces_max_tool_calls_policy() {
    let module = load_agent("tests/agents/tool-limit.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: CapturingModels { seen: None },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("refund policy"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ToolCallLimitExceeded {
            limit: 1,
            attempted: 2
        }
    ));
    assert_eq!(vm.tools.calls, 1);
}

#[test]
fn dispatches_model_selected_tool_with_runtime_governance() {
    let module = load_agent("tests/agents/tool-dispatch.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: DispatchModels,
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("typed agent ir"))]),
        )
        .unwrap();

    assert_eq!(result.outputs["result"]["query"], json!("typed agent ir"));
    assert_eq!(result.outputs["result"]["call"], json!(1));
    assert!(result
        .trace
        .iter()
        .any(|event| event.action == "tool_dispatch_start"
            && event
                .meta
                .as_ref()
                .is_some_and(|meta| meta["tool"] == "docs.search")));
    assert!(result
        .trace
        .iter()
        .any(|event| event.action == "tool_dispatch"
            && event
                .meta
                .as_ref()
                .is_some_and(|meta| meta["tool"] == "docs.search")));
}

#[test]
fn dispatches_common_name_args_tool_shape() {
    let module = load_agent("tests/agents/tool-dispatch-name-args.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: NameArgsDispatchModels,
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("typed agent ir"))]),
        )
        .unwrap();

    assert_eq!(result.outputs["result"]["query"], json!("typed agent ir"));
    assert_eq!(result.outputs["result"]["call"], json!(1));
}

#[test]
fn dispatches_common_tool_args_tool_shape() {
    let module = load_agent("tests/agents/tool-dispatch-name-args.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: ToolArgsDispatchModels,
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("typed agent ir"))]),
        )
        .unwrap();

    assert_eq!(result.outputs["result"]["query"], json!("typed agent ir"));
    assert_eq!(result.outputs["result"]["call"], json!(1));
}

#[test]
fn dispatch_normalizes_model_selected_tool_name_case() {
    let module = load_agent("tests/agents/tool-dispatch.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: UppercaseDispatchModels,
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("typed agent ir"))]),
        )
        .unwrap();

    assert_eq!(result.outputs["result"]["query"], json!("typed agent ir"));
    assert_eq!(result.outputs["result"]["call"], json!(1));
    assert!(result.trace.iter().any(|event| {
        event.action == "tool_dispatch"
            && event.meta.as_ref().is_some_and(|meta| {
                meta["tool"] == "docs.search" && meta["requested_tool"] == "DOCS.SEARCH"
            })
    }));
}

#[test]
fn rejects_model_selected_undeclared_tool_dispatch() {
    let module = load_agent("tests/agents/tool-dispatch.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: UnsafeDispatchModels,
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("malicious patch"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::UndeclaredTool { module, tool }
            if module == "tool-dispatch-agent" && tool == "shell.run"
    ));
    assert_eq!(vm.tools.calls, 0);
}

#[test]
fn rejects_malformed_tool_dispatch_choice() {
    let module = load_agent("tests/agents/tool-dispatch.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: MalformedDispatchModels,
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("bad dispatch"))]),
        )
        .unwrap_err();

    assert_schema_error_contains(error, "choice.tool expected string got integer");
    assert_eq!(vm.tools.calls, 0);
}

#[test]
fn rejects_repeated_identical_tool_calls_when_policy_is_set() {
    let module = load_agent("tests/agents/repeated-tool-call.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: SchemaModels {
            extract: json!({}),
            score: json!({}),
            report: json!({}),
        },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("same query"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::RepeatedToolCallLimitExceeded {
            tool,
            limit: 1,
            attempted: 2
        } if tool == "docs.search"
    ));
    assert_eq!(vm.tools.calls, 1);
}

#[test]
fn dispatches_bounded_model_selected_tool_batch() {
    let module = load_agent("tests/agents/tool-batch-dispatch.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {"tool": "docs.search", "input": {"query": "alpha"}},
                {"tool": "docs.search", "input": {"query": "beta"}}
            ]),
        },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("batch search"))]),
        )
        .unwrap();

    assert_eq!(
        result.outputs["observations"][0]["tool"],
        json!("docs.search")
    );
    assert_eq!(
        result.outputs["observations"][0]["output"]["query"],
        json!("alpha")
    );
    assert_eq!(
        result.outputs["observations"][1]["output"]["query"],
        json!("beta")
    );
    assert_eq!(vm.tools.calls, 2);
    assert!(result
        .trace
        .iter()
        .any(|event| event.action == "tool_batch_dispatch"
            && event.meta.as_ref().is_some_and(|meta| meta["count"] == 2)));
}

#[test]
fn tool_batch_dispatch_can_observe_provider_errors() {
    let mut module = load_agent("tests/agents/tool-batch-dispatch.air.yaml");
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let StateAction::ToolBatchDispatch { on_error, .. } =
        workflow.rules[2].actions.first_mut().unwrap()
    else {
        panic!("expected tool_batch_dispatch action");
    };
    *on_error = air_core::ToolErrorMode::Observe;

    let mut vm = Vm {
        tools: FailingBatchTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {"tool": "docs.search", "input": {"query": "bad"}},
                {"tool": "docs.search", "input": {"query": "good"}}
            ]),
        },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("batch search"))]),
        )
        .unwrap();

    assert_eq!(vm.tools.calls, 2);
    assert_eq!(result.outputs["observations"][0]["status"], json!("error"));
    assert_eq!(
        result.outputs["observations"][0]["error"],
        json!("provider error: synthetic tool failure")
    );
    assert_eq!(result.outputs["observations"][1]["status"], json!("ok"));
    assert_eq!(
        result.outputs["observations"][1]["output"]["query"],
        json!("good")
    );
    assert!(result.trace.iter().any(|event| {
        event.action == "tool_batch_dispatch"
            && event.status == TraceStatus::Ok
            && event
                .meta
                .as_ref()
                .is_some_and(|meta| meta["error_count"] == 1)
    }));
    assert!(result.trace.iter().any(|event| {
        event.action == "tool_batch_dispatch_item"
            && event.status == TraceStatus::Error
            && event.error.as_deref() == Some("provider error: synthetic tool failure")
    }));
}

#[test]
fn tool_batch_dispatch_can_observe_repeated_tool_policy_errors() {
    let mut module = load_agent("tests/agents/tool-batch-dispatch.air.yaml");
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let StateAction::ToolBatchDispatch { on_error, .. } =
        workflow.rules[2].actions.first_mut().unwrap()
    else {
        panic!("expected tool_batch_dispatch action");
    };
    *on_error = air_core::ToolErrorMode::Observe;

    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {"tool": "docs.search", "input": {"query": "same"}},
                {"tool": "docs.search", "input": {"query": "same"}}
            ]),
        },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("batch search"))]),
        )
        .unwrap();

    assert_eq!(vm.tools.calls, 1);
    assert_eq!(result.outputs["observations"][0]["status"], json!("ok"));
    assert_eq!(result.outputs["observations"][1]["status"], json!("error"));
    assert_eq!(
        result.outputs["observations"][1]["error"],
        json!("policy.max_repeated_tool_calls exceeded for tool docs.search: limit=1 attempted=2")
    );
    assert_eq!(
        result.outputs["observations"][1]["error_code"],
        json!("doom_loop")
    );
    assert_eq!(
        result.outputs["observations"][1]["permission"],
        json!("doom_loop")
    );
    assert_eq!(
        result.outputs["observations"][1]["output"]["error_code"],
        json!("doom_loop")
    );
    assert!(result.trace.iter().any(|event| {
        event.action == "tool_batch_dispatch"
            && event.status == TraceStatus::Ok
            && event
                .meta
                .as_ref()
                .is_some_and(|meta| meta["error_count"] == 1)
    }));
}

#[test]
fn tool_batch_dispatch_can_observe_undeclared_tools() {
    let mut module = load_agent("tests/agents/tool-batch-dispatch.air.yaml");
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let StateAction::ToolBatchDispatch { on_error, .. } =
        workflow.rules[2].actions.first_mut().unwrap()
    else {
        panic!("expected tool_batch_dispatch action");
    };
    *on_error = air_core::ToolErrorMode::Observe;

    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {"tool": "docs.lookup", "input": {"query": "bad"}},
                {"tool": "docs.search", "input": {"query": "good"}}
            ]),
        },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("batch search"))]),
        )
        .unwrap();

    assert_eq!(vm.tools.calls, 1);
    assert_eq!(
        result.outputs["observations"][0]["tool"],
        json!("docs.lookup")
    );
    assert_eq!(result.outputs["observations"][0]["status"], json!("error"));
    assert_eq!(
        result.outputs["observations"][0]["error"],
        json!("tool docs.lookup is not declared by module tool-batch-dispatch-agent")
    );
    assert_eq!(result.outputs["observations"][1]["status"], json!("ok"));
    assert_eq!(
        result.outputs["observations"][1]["output"]["query"],
        json!("good")
    );
}

#[test]
fn tool_batch_dispatch_normalizes_tool_name_case() {
    let module = load_agent("tests/agents/tool-batch-dispatch.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {"tool": "DOCS.SEARCH", "input": {"query": "alpha"}},
                {"tool": "docs.search", "input": {"query": "beta"}}
            ]),
        },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("batch search"))]),
        )
        .unwrap();

    assert_eq!(vm.tools.calls, 2);
    assert_eq!(
        result.outputs["observations"][0]["tool"],
        json!("docs.search")
    );
    assert_eq!(
        result.outputs["observations"][0]["output"]["query"],
        json!("alpha")
    );
    assert!(result.trace.iter().any(|event| {
        event.action == "tool_batch_dispatch_item"
            && event.meta.as_ref().is_some_and(|meta| {
                meta["tool"] == "docs.search" && meta["requested_tool"] == "DOCS.SEARCH"
            })
    }));
}

#[test]
fn tool_batch_dispatch_can_observe_malformed_items() {
    let mut module = load_agent("tests/agents/tool-batch-dispatch.air.yaml");
    module.state.insert(
        "choices".to_string(),
        air_core::TypeSpec::Detailed(air_core::DetailedType {
            kind: air_core::DetailedTypeKind::Array,
            additional_properties: true,
            required: Vec::new(),
            properties: Default::default(),
            items: Some(Box::new(air_core::TypeSpec::Shorthand(
                air_core::PrimitiveType::Object,
            ))),
            min_items: None,
            max_items: None,
            enum_values: Vec::new(),
        }),
    );
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let StateAction::ToolBatchDispatch { on_error, .. } =
        workflow.rules[2].actions.first_mut().unwrap()
    else {
        panic!("expected tool_batch_dispatch action");
    };
    *on_error = air_core::ToolErrorMode::Observe;

    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {"input": {"query": "bad"}},
                {"tool": "docs.search", "input": {"query": "good"}}
            ]),
        },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("batch search"))]),
        )
        .unwrap();

    assert_eq!(vm.tools.calls, 1);
    assert_eq!(
        result.outputs["observations"][0]["tool"],
        json!("<invalid>")
    );
    assert_eq!(result.outputs["observations"][0]["status"], json!("error"));
    assert_eq!(
        result.outputs["observations"][0]["error"],
        json!("schema violation: tool_batch_dispatch input[0] invalid: tool_dispatch input.tool must be a string")
    );
    assert_eq!(
        result.outputs["observations"][0]["output"]["requested"],
        json!({"input": {"query": "bad"}})
    );
    assert_eq!(result.outputs["observations"][1]["status"], json!("ok"));
    assert_eq!(
        result.outputs["observations"][1]["output"]["query"],
        json!("good")
    );
}

#[test]
fn rejects_tool_batch_dispatch_over_action_bound_before_provider_calls() {
    let module = load_agent("tests/agents/tool-batch-dispatch.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {"tool": "docs.search", "input": {"query": "alpha"}},
                {"tool": "docs.search", "input": {"query": "beta"}},
                {"tool": "docs.search", "input": {"query": "gamma"}}
            ]),
        },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("batch search"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ToolBatchDispatchLimitExceeded {
            limit: 2,
            attempted: 3
        }
    ));
    assert_eq!(vm.tools.calls, 0);
}

#[test]
fn rejects_repeated_identical_tool_batch_items_when_policy_is_set() {
    let module = load_agent("tests/agents/tool-batch-dispatch.air.yaml");
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {"tool": "docs.search", "input": {"query": "same"}},
                {"tool": "docs.search", "input": {"query": "same"}}
            ]),
        },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("batch search"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::RepeatedToolCallLimitExceeded {
            tool,
            limit: 1,
            attempted: 2
        } if tool == "docs.search"
    ));
    assert_eq!(vm.tools.calls, 1);
}

#[test]
fn allows_mixed_approval_required_tool_batch_after_approval() {
    let module = load_agent("tests/agents/tool-batch-dispatch-approval.air.yaml");
    let mut vm = Vm {
        tools: BatchApprovalTools { calls: 0 },
        models: BatchDispatchModels {
            choices: json!([
                {
                    "tool": "edit",
                    "input": {
                        "filePath": "example.txt",
                        "oldString": "before",
                        "newString": "patched"
                    }
                },
                {"tool": "docs.search", "input": {"query": "alpha"}}
            ]),
        },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("mixed write and search"))]),
        )
        .unwrap();

    assert_eq!(vm.tools.calls, 2);
    assert_eq!(result.outputs["observations"][0]["tool"], json!("edit"));
    assert_eq!(result.outputs["observations"][0]["status"], json!("ok"));
    assert_eq!(
        result.outputs["observations"][1]["tool"],
        json!("docs.search")
    );
    assert_eq!(result.outputs["observations"][1]["status"], json!("ok"));
}

#[test]
fn rejects_provider_tool_capability_mismatch_at_runtime() {
    let module = load_agent("tests/agents/tool-limit.air.yaml");
    let mut vm = Vm {
        tools: CapabilityTools {
            capability: "network.search",
        },
        models: SchemaModels {
            extract: valid_extracted(),
            score: valid_score(),
            report: valid_report(),
        },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ToolCapabilityMismatch {
            ref tool,
            ref provider_capability,
            ref module_capability
        } if tool == "docs.search"
            && provider_capability == "network.search"
            && module_capability == "retrieval.local"
    ));
}

#[test]
fn accepts_model_citations_to_registered_tool_artifacts() {
    let module = load_agent("tests/agents/citation-check.air.yaml");
    let mut vm = Vm {
        tools: ArtifactTools,
        models: CitationModels {
            sources: vec!["doc-1"],
        },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("query".to_string(), json!("AIR provenance"))]),
        )
        .unwrap();

    assert_eq!(result.outputs["answer"]["sources"], json!(["doc-1"]));
    let tool_event = result
        .trace
        .iter()
        .find(|event| event.action == "tool_call" && event.status == TraceStatus::Ok)
        .unwrap();
    assert_eq!(
        tool_event.meta.as_ref().unwrap()["artifact_ids"],
        json!(["doc-1"])
    );
}

#[test]
fn rejects_model_citations_to_unknown_tool_artifacts() {
    let module = load_agent("tests/agents/citation-check.air.yaml");
    let mut vm = Vm {
        tools: ArtifactTools,
        models: CitationModels {
            sources: vec!["missing-doc"],
        },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("query".to_string(), json!("AIR provenance"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::UnknownCitation {
            ref path,
            ref citation,
            ..
        } if path == "answer.sources[0]" && citation == "missing-doc"
    ));
}

#[test]
fn default_approval_provider_fails_closed() {
    let module = load_agent("tests/agents/approval-gate.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: CapturingModels { seen: None },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("deployment_id".to_string(), json!("deploy-123"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ApprovalRequired {
            ref module,
            ref capabilities
        } if module == "approval-gate-agent" && capabilities == &vec!["production.deploy".to_string()]
    ));
}

#[test]
fn approval_provider_allows_approved_gate_and_records_trace() {
    let module = load_agent("tests/agents/approval-gate.air.yaml");
    let mut vm = Vm {
        tools: ApprovingTools,
        models: CapturingModels { seen: None },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("deployment_id".to_string(), json!("deploy-123"))]),
        )
        .unwrap();

    assert_eq!(
        result.outputs["result"]["deployment_id"],
        json!("deploy-123")
    );
    let approval = result
        .trace
        .iter()
        .find(|event| event.action == "approval")
        .expect("approval event");
    assert_eq!(approval.status, TraceStatus::Ok);
    assert_eq!(approval.meta.as_ref().unwrap()["approved"], json!(true));
    assert_eq!(
        approval.meta.as_ref().unwrap()["approver"],
        json!("test-approver")
    );
}

#[test]
fn approval_provider_denial_stops_execution() {
    let module = load_agent("tests/agents/approval-gate.air.yaml");
    let mut vm = Vm {
        tools: DenyingTools,
        models: CapturingModels { seen: None },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("deployment_id".to_string(), json!("deploy-123"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ApprovalDenied {
            ref module,
            ref capabilities,
            ref reason
        } if module == "approval-gate-agent"
            && capabilities == &vec!["production.deploy".to_string()]
            && reason == "not allowed in this test"
    ));
}

#[test]
fn enforces_max_model_calls_policy() {
    let module = load_agent("tests/agents/model-limit.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: LimitModels { calls: 0 },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("research plan"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ModelCallLimitExceeded {
            limit: 1,
            attempted: 2
        }
    ));
    assert_eq!(vm.models.calls, 1);
}

#[test]
fn enforces_model_call_timeout_seconds() {
    let mut module = load_agent("tests/agents/model-limit.air.yaml");
    set_first_call_timeout(&mut module, "model_call", 0);
    let mut vm = Vm {
        tools: SchemaTools,
        models: LimitModels { calls: 0 },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("research plan"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ActionTimeoutExceeded {
            action,
            timeout_seconds: 0,
            ..
        } if action == "model_call"
    ));
    assert_eq!(vm.models.calls, 1);
}

#[test]
fn rejects_model_call_writing_phase_at_runtime() {
    let mut module = load_agent("tests/agents/model-smoke.air.yaml");
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let StateAction::ModelCall { output, .. } = &mut workflow.rules[1].actions[0] else {
        panic!("expected model_call");
    };
    *output = "phase".to_string();
    let mut vm = Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: json!("done"),
            score: json!({}),
            report: json!({}),
        },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("prompt".to_string(), json!("route"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ControlFieldWrite { action } if action == "model_call"
    ));
}

#[test]
fn enforces_tool_call_timeout_seconds() {
    let mut module = load_agent("tests/agents/tool-limit.air.yaml");
    set_first_call_timeout(&mut module, "tool_call", 0);
    let mut vm = Vm {
        tools: CountingTools { calls: 0 },
        models: CapturingModels { seen: None },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("refund policy"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ActionTimeoutExceeded {
            action,
            timeout_seconds: 0,
            ..
        } if action == "tool_call"
    ));
    assert_eq!(vm.tools.calls, 1);
}

#[test]
fn passes_model_action_timeout_to_provider() {
    let mut module = load_agent("tests/agents/schema-extract.air.yaml");
    set_first_call_timeout(&mut module, "model_call", 7);
    let seen = Rc::new(RefCell::new(None));
    let mut vm = Vm {
        tools: SchemaTools,
        models: TimeoutRecordingModels { seen: seen.clone() },
    };

    vm.run(
        &module,
        State::from_iter([("text".to_string(), json!("billing issue"))]),
    )
    .unwrap();

    assert_eq!(*seen.borrow(), Some(Duration::from_secs(7)));
}

#[test]
fn passes_tool_action_timeout_to_provider() {
    let mut module = load_agent("tests/agents/tool-capability-smoke.air.yaml");
    set_first_call_timeout(&mut module, "tool_call", 9);
    let seen = Rc::new(RefCell::new(None));
    let mut vm = Vm {
        tools: TimeoutRecordingTools { seen: seen.clone() },
        models: CapturingModels { seen: None },
    };

    vm.run(
        &module,
        State::from_iter([("text".to_string(), json!("refund policy"))]),
    )
    .unwrap();

    assert_eq!(*seen.borrow(), Some(Duration::from_secs(9)));
}

#[test]
fn enforces_module_timeout_policy() {
    let mut module = load_agent("tests/agents/review-string-input.air.yaml");
    module.policy.timeout_seconds = Some(0);
    let mut vm = Vm {
        tools: SchemaTools,
        models: CapturingModels { seen: None },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("pr_payload_id".to_string(), json!("pr-1"))]),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeError::ModuleTimeoutExceeded { limit: 0, .. }
    ));
}

#[test]
fn retries_model_call_after_schema_violation() {
    let module = load_agent("tests/agents/model-retry.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: RetryModels { calls: 0 },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("research plan"))]),
        )
        .unwrap();

    assert_eq!(vm.models.calls, 2);
    assert_eq!(
        result.outputs["result"]["summary"],
        json!("valid retry result")
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "model_call" && event.status == TraceStatus::Error)
            .count(),
        1
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "model_call" && event.status == TraceStatus::Ok)
            .count(),
        1
    );
}

#[test]
fn retries_model_call_after_provider_error_without_schema_retry_instruction() {
    let module = load_agent("tests/agents/model-retry.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: ProviderRetryModels { calls: 0 },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("research plan"))]),
        )
        .unwrap();

    assert_eq!(vm.models.calls, 2);
    assert_eq!(
        result.outputs["result"]["summary"],
        json!("valid provider retry result")
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "model_call" && event.status == TraceStatus::Error)
            .count(),
        1
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "model_call" && event.status == TraceStatus::Ok)
            .count(),
        1
    );
}

#[test]
fn accepts_model_content_wrapper_when_parsed_json_matches_schema() {
    let module = load_agent("tests/agents/model-retry.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: ContentWrappedModels,
    };

    let result = vm
        .run(
            &module,
            State::from_iter([("text".to_string(), json!("research plan"))]),
        )
        .unwrap();

    assert_eq!(
        result.outputs["result"]["summary"],
        json!("wrapped model result")
    );
    let model_event = result
        .trace
        .iter()
        .find(|event| event.action == "model_call")
        .expect("model_call event");
    assert_eq!(model_event.status, TraceStatus::Ok);
    assert_eq!(
        model_event.output.as_ref().unwrap()["rationale"],
        json!("content parsed into the declared output schema")
    );
}

#[test]
fn retries_model_call_after_provider_token_limit_with_compacted_input() {
    let module = load_agent("tests/agents/model-token-limit-retry.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: TokenLimitRetryModels { calls: 0 },
    };

    let result = vm
        .run(
            &module,
            State::from_iter([(
                "text".to_string(),
                json!("This research context is intentionally long and should be compacted."),
            )]),
        )
        .unwrap();

    assert_eq!(vm.models.calls, 2);
    assert_eq!(
        result.outputs["result"]["summary"],
        json!("compacted retry result")
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "model_call" && event.status == TraceStatus::Error)
            .count(),
        1
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "model_call" && event.status == TraceStatus::Ok)
            .count(),
        1
    );
}

#[test]
fn writes_jsonl_trace_and_replays_final_output() {
    let extract = load_agent("tests/agents/schema-extract.air.yaml");
    let mut vm = valid_vm();
    let result = vm
        .run(
            &extract,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
        )
        .unwrap();

    let model_event = result
        .trace
        .iter()
        .find(|event| event.action == "model_call")
        .expect("model_call event");
    assert_eq!(model_event.status, TraceStatus::Ok);
    assert!(model_event.input.is_some());
    assert!(model_event.output.is_some());

    let mut trace = result.trace.clone();
    trace.push(system_return_event(result.outputs.clone()));
    let path = std::env::temp_dir().join(format!(
        "air-trace-{}-{}.jsonl",
        std::process::id(),
        "schema-contract"
    ));
    write_trace_jsonl(&path, &trace).unwrap();

    let events = read_trace_jsonl(&path).unwrap();
    let replayed = replay_outputs(&events).unwrap();
    assert_eq!(replayed["extracted"]["product_area"], json!("billing"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn redacted_trace_writer_masks_sensitive_fields_and_limits_size() {
    let trace = vec![TraceEvent {
        agent: "test-agent".to_string(),
        step: 0,
        rule: "test".to_string(),
        action: "model_call".to_string(),
        input: Some(json!({
            "api_key": "sk-live-secret",
            "nested": {
                "password": "super-secret-password",
                "text": "safe"
            },
            "long": "x".repeat(32)
        })),
        output: Some(json!({
            "token": "runtime-token",
            "summary": "done"
        })),
        meta: Some(json!({
            "headers": {
                "authorization": "Bearer provider-secret"
            }
        })),
        status: TraceStatus::Error,
        error: Some("provider returned Authorization: Bearer error-secret".to_string()),
    }];
    let path = std::env::temp_dir().join(format!(
        "air-redacted-trace-{}-{}.jsonl",
        std::process::id(),
        "schema-contract"
    ));
    let options = TraceWriteOptions {
        redact_sensitive: true,
        max_string_chars: Some(8),
        max_event_bytes: Some(512),
    };

    write_trace_jsonl_with_options(&path, &trace, &options).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(path);

    assert!(!raw.contains("sk-live-secret"));
    assert!(!raw.contains("super-secret-password"));
    assert!(!raw.contains("runtime-token"));
    assert!(!raw.contains("provider-secret"));
    assert!(!raw.contains("error-secret"));
    assert!(raw.contains("[AIR_REDACTED]"));
    assert!(raw.contains("[AIR_TRUNCATED]"));
}

#[test]
fn notifies_observer_as_actions_complete() {
    let extract = load_agent("tests/agents/schema-extract.air.yaml");
    let mut vm = valid_vm();
    let mut observed = Vec::new();

    let result = vm
        .run_with_observer(
            &extract,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
            |event| {
                observed.push((
                    event.agent.clone(),
                    event.action.clone(),
                    event.status.clone(),
                ))
            },
        )
        .unwrap();

    let trace_summary: Vec<_> = result
        .trace
        .iter()
        .map(|event| {
            (
                event.agent.clone(),
                event.action.clone(),
                event.status.clone(),
            )
        })
        .collect();
    assert_eq!(observed, trace_summary);
    assert_eq!(
        observed
            .iter()
            .map(|(_, action, _)| action.as_str())
            .collect::<Vec<_>>(),
        ["set", "model_call_start", "model_call", "set", "return"]
    );
    let start_meta = result
        .trace
        .iter()
        .find(|event| event.action == "model_call_start")
        .and_then(|event| event.meta.as_ref())
        .expect("model_call_start meta");
    assert_eq!(start_meta["model"], json!("extractor"));
    assert_eq!(start_meta["attempt"], json!(1));
    assert!(start_meta["input_bytes"].as_u64().unwrap() > 0);

    let complete_meta = result
        .trace
        .iter()
        .find(|event| event.action == "model_call")
        .and_then(|event| event.meta.as_ref())
        .expect("model_call meta");
    assert_eq!(complete_meta["model"], json!("extractor"));
    assert_eq!(complete_meta["will_retry"], json!(false));
    assert_eq!(complete_meta["input_bytes"], start_meta["input_bytes"]);
    assert!(complete_meta.get("elapsed_ms").is_some());
}

#[test]
fn model_call_trace_records_provider_request_stats_when_available() {
    let extract = load_agent("tests/agents/schema-extract.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: RequestStatsModels {
            output: valid_extracted(),
            stats: Some(ModelRequestStats {
                provider_request_bytes: 1234,
                provider_user_content_bytes: 456,
                provider_tools_bytes: 789,
                provider_request: Some(json!({
                    "model": "test-model",
                    "messages": [{"role": "user", "content": "hello"}]
                })),
                provider_response: Some(json!({
                    "choices": [{
                        "message": {"content": "{\"ok\":true}"}
                    }]
                })),
            }),
        },
    };

    let result = vm
        .run(
            &extract,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
        )
        .unwrap();
    let complete_meta = result
        .trace
        .iter()
        .find(|event| event.action == "model_call")
        .and_then(|event| event.meta.as_ref())
        .expect("model_call meta");

    assert_eq!(complete_meta["provider_request_bytes"], json!(1234));
    assert_eq!(complete_meta["provider_user_content_bytes"], json!(456));
    assert_eq!(complete_meta["provider_tools_bytes"], json!(789));
    assert_eq!(
        complete_meta["provider_request"]["model"],
        json!("test-model")
    );
    assert_eq!(
        complete_meta["provider_response"]["choices"][0]["message"]["content"],
        json!("{\"ok\":true}")
    );
}

#[test]
fn notifies_observer_when_model_output_fails_schema() {
    let extract = load_agent("tests/agents/schema-extract.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: json!({"customer_issue": "double charge", "product_area": "billing"}),
            score: valid_score(),
            report: valid_report(),
        },
    };
    let mut observed = Vec::new();

    let error = vm
        .run_with_observer(
            &extract,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
            |event| observed.push(event.clone()),
        )
        .unwrap_err();

    assert_schema_error_contains(error, "extracted.sentiment");
    let actions = observed
        .iter()
        .map(|event| (event.action.as_str(), &event.status))
        .collect::<Vec<_>>();
    assert_eq!(
        actions,
        [
            ("set", &TraceStatus::Ok),
            ("model_call_start", &TraceStatus::Ok),
            ("model_call", &TraceStatus::Error),
        ]
    );
    let error_event = observed.last().unwrap();
    assert!(error_event.output.is_some());
    assert_eq!(
        error_event
            .meta
            .as_ref()
            .and_then(|meta| meta.get("will_retry")),
        Some(&json!(false))
    );
    assert!(error_event
        .error
        .as_ref()
        .is_some_and(|error| error.contains("extracted.sentiment")));
}

#[test]
fn rejects_model_output_missing_required_field() {
    let extract = load_agent("tests/agents/schema-extract.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: json!({"customer_issue": "double charge", "product_area": "billing"}),
            score: valid_score(),
            report: valid_report(),
        },
    };

    let error = vm
        .run(
            &extract,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
        )
        .unwrap_err();

    assert_schema_error_contains(error, "extracted.sentiment");
}

#[test]
fn allows_model_output_additional_fields_by_default() {
    let extract = load_agent("tests/agents/schema-extract.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: json!({
                "customer_issue": "double charge",
                "product_area": "billing",
                "sentiment": "negative",
                "urgency_signals": ["cancel"],
                "provider_metadata": "kept for compatibility"
            }),
            score: valid_score(),
            report: valid_report(),
        },
    };

    let output = vm
        .run(
            &extract,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
        )
        .unwrap();

    assert_eq!(
        output.outputs["extracted"]["provider_metadata"],
        json!("kept for compatibility")
    );
}

#[test]
fn rejects_model_output_additional_fields_when_schema_is_strict() {
    let mut extract = load_agent("tests/agents/schema-extract.air.yaml");
    let Some(air_core::TypeSpec::Detailed(extracted_schema)) = extract.outputs.get_mut("extracted")
    else {
        panic!("expected detailed extracted output schema");
    };
    extracted_schema.additional_properties = false;
    let mut vm = Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: json!({
                "customer_issue": "double charge",
                "product_area": "billing",
                "sentiment": "negative",
                "urgency_signals": ["cancel"],
                "provider_metadata": "should be rejected"
            }),
            score: valid_score(),
            report: valid_report(),
        },
    };

    let error = vm
        .run(
            &extract,
            State::from_iter([("text".to_string(), json!("billing issue"))]),
        )
        .unwrap_err();

    assert_schema_error_contains(
        error,
        "extracted.provider_metadata unexpected additional field",
    );
}

#[test]
fn rejects_model_output_with_wrong_primitive_type() {
    let score = load_agent("tests/agents/schema-score.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: valid_extracted(),
            score: json!({"issue_summary": "billing", "risk_score": "low", "priority": "high", "reason": "low severity"}),
            report: valid_report(),
        },
    };

    let error = vm
        .run(
            &score,
            State::from_iter([
                ("text".to_string(), json!("billing issue")),
                ("extracted".to_string(), valid_extracted()),
            ]),
        )
        .unwrap_err();

    assert_schema_error_contains(error, "score.risk_score expected number");
}

#[test]
fn rejects_report_model_output_with_wrong_primitive_type() {
    let report = load_agent("tests/agents/schema-report.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: valid_extracted(),
            score: valid_score(),
            report: json!({"summary": "ok", "recommended_action": "reply today", "owner": "billing_support", "approved": "yes"}),
        },
    };

    let error = vm
        .run(
            &report,
            State::from_iter([
                ("text".to_string(), json!("billing issue")),
                ("extracted".to_string(), valid_extracted()),
                ("score".to_string(), valid_score()),
            ]),
        )
        .unwrap_err();

    assert_schema_error_contains(error, "report.approved expected boolean");
}

#[test]
fn rejects_set_action_value_that_violates_state_schema() {
    let mut module = load_agent("tests/agents/approval-gate.air.yaml");
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let StateAction::Set { values } = &mut workflow.rules[0].actions[1] else {
        panic!("expected set action");
    };
    values.insert(
        "result".to_string(),
        json!({
            "deployment_id": "deploy-123",
            "approved": "yes"
        }),
    );
    let mut vm = Vm {
        tools: ApprovingTools,
        models: SchemaModels {
            extract: valid_extracted(),
            score: valid_score(),
            report: valid_report(),
        },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("deployment_id".to_string(), json!("deploy-123"))]),
        )
        .unwrap_err();

    assert_schema_error_contains(error, "result.approved expected boolean");
}

#[test]
fn rejects_return_value_that_violates_output_schema() {
    let mut module = load_agent("tests/agents/approval-gate.air.yaml");
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let StateAction::Set { values } = &mut workflow.rules[0].actions[1] else {
        panic!("expected set action");
    };
    module.state.remove("result");
    values.insert(
        "result".to_string(),
        json!({
            "deployment_id": "deploy-123",
            "approved": "yes"
        }),
    );
    let mut vm = Vm {
        tools: ApprovingTools,
        models: SchemaModels {
            extract: valid_extracted(),
            score: valid_score(),
            report: valid_report(),
        },
    };

    let error = vm
        .run(
            &module,
            State::from_iter([("deployment_id".to_string(), json!("deploy-123"))]),
        )
        .unwrap_err();

    assert_schema_error_contains(error, "result.approved expected boolean");
}

#[test]
fn rejects_expression_path_missing_nested_schema_field() {
    let mut module = load_agent("tests/agents/expr-input.air.yaml");
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let StateAction::ModelCall { input, .. } = &mut workflow.rules[1].actions[0] else {
        panic!("expected model_call action");
    };
    let air_core::InputSpec::Expr(air_core::Expr::Object { object }) = input else {
        panic!("expected object expression");
    };
    object.insert(
        "bad".to_string(),
        air_core::Expr::Path {
            path: "extracted.not_a_field".to_string(),
        },
    );

    let report = air_verify::verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR094"),
        "expected AIR094, got {:?}",
        report.diagnostics
    );
}

#[test]
fn runtime_context_is_available_to_conditions_and_expressions() {
    let module = load_agent("tests/agents/runtime-context.air.yaml");
    let mut vm = Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: valid_extracted(),
            score: valid_score(),
            report: valid_report(),
        },
    };

    let result = vm.run(&module, State::new()).unwrap();

    assert_eq!(result.outputs["answer"]["step_number"], json!(1));
    assert_eq!(result.outputs["answer"]["remaining_steps"], json!(1));
    assert_eq!(result.outputs["answer"]["is_last_action_step"], json!(true));
}

#[test]
fn verifier_rejects_unknown_runtime_context_path() {
    let mut module = load_agent("tests/agents/runtime-context.air.yaml");
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    workflow.rules[0].when = "phase == \"init\" && _air.not_a_field == true".to_string();

    let report = air_verify::verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR094"),
        "expected AIR094, got {:?}",
        report.diagnostics
    );
}

#[test]
fn verifier_rejects_schema_fields_that_shadow_runtime_context() {
    let mut module = load_agent("tests/agents/runtime-context.air.yaml");
    module.state.insert(
        "_air".to_string(),
        air_core::TypeSpec::Shorthand(air_core::PrimitiveType::Object),
    );

    let report = air_verify::verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR095"),
        "expected AIR095, got {:?}",
        report.diagnostics
    );
}

fn valid_vm() -> Vm<SchemaTools, SchemaModels> {
    Vm {
        tools: SchemaTools,
        models: SchemaModels {
            extract: valid_extracted(),
            score: valid_score(),
            report: valid_report(),
        },
    }
}

fn valid_extracted() -> Value {
    json!({
        "customer_issue": "double charge after subscription upgrade",
        "product_area": "billing",
        "sentiment": "negative",
        "urgency_signals": ["charged twice", "will cancel"]
    })
}

fn valid_score() -> Value {
    json!({
        "issue_summary": "duplicate billing with churn risk",
        "risk_score": 0.86,
        "priority": "high",
        "reason": "billing error and explicit cancellation threat"
    })
}

fn valid_report() -> Value {
    json!({
        "summary": "Customer reports duplicate billing after upgrade.",
        "recommended_action": "Escalate to billing support and respond today.",
        "owner": "billing_support",
        "approved": true
    })
}

fn assert_schema_error_contains(error: RuntimeError, needle: &str) {
    let message = error.to_string();
    assert!(
        message.contains(needle),
        "expected error to contain {needle:?}, got {message:?}"
    );
}

fn set_first_call_timeout(module: &mut air_core::AirModule, kind: &str, timeout_seconds: u64) {
    let Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };

    for action in workflow.rules.iter_mut().flat_map(|rule| &mut rule.actions) {
        match (kind, action) {
            (
                "model_call",
                StateAction::ModelCall {
                    timeout_seconds: timeout,
                    ..
                },
            )
            | (
                "tool_call",
                StateAction::ToolCall {
                    timeout_seconds: timeout,
                    ..
                },
            ) => {
                *timeout = timeout_seconds;
                return;
            }
            _ => {}
        }
    }

    panic!("expected {kind} action");
}

fn load_agent(path: &str) -> air_core::AirModule {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    air_parser::parse_air_file(path).unwrap()
}
