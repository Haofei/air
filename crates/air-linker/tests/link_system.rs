use air_linker::{
    parse_module_store_file, parse_run_plan_file, parse_system_file, resume_run_plan, run_run_plan,
    run_run_plan_parallel_with_observer_and_checkpoint, run_run_plan_with_observer_and_checkpoint,
    run_system, run_system_parallel_with_resume_with_observer_and_checkpoint,
    run_system_with_observer, specialize_run_plan_trace, AirSystem, ModuleKind, ModuleRef,
    ModuleVisibility, ResumeState, RunStatus, ScheduleGroup, SystemMetadata, SystemSchedule,
};
use air_runtime::{ModelProvider, RuntimeError, State, ToolProvider};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

#[derive(Default)]
struct MockTools;

impl ToolProvider for MockTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        panic!("unexpected tool {name} with {input}")
    }
}

#[derive(Default)]
struct MockModels;

impl ModelProvider for MockModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "extractor" => Ok(json!({
                "customer_issue": "double charge after subscription upgrade",
                "product_area": "billing",
                "sentiment": "negative",
                "urgency_signals": ["charged twice", "will cancel"]
            })),
            "scorer" => Ok(json!({
                "issue_summary": "duplicate billing with churn risk",
                "risk_score": 0.86,
                "priority": "high",
                "reason": "billing error and explicit cancellation threat"
            })),
            "reporter" => Ok(json!({
                "summary": format!("{}: {}", input["score"]["priority"].as_str().unwrap(), input["score"]["issue_summary"].as_str().unwrap()),
                "recommended_action": "Escalate to billing support and respond today.",
                "owner": "billing_support",
                "approved": true
            })),
            "compliance_checker" => Ok(json!({
                "requires_legal_review": true,
                "policy_refs": ["refund-policy", "enterprise-sla"],
                "reason": format!("{} requires policy review", input["report"]["owner"].as_str().unwrap())
            })),
            "retention_planner" => Ok(json!({
                "churn_risk": if input["account_tier"] == "enterprise" { "critical" } else { "high" },
                "offer": "executive callback and one-month service credit",
                "owner": "customer_success"
            })),
            "finance_estimator" => Ok(json!({
                "refund_required": true,
                "estimated_credit": input["invoice_count"].as_i64().unwrap() as f64 * 100.0,
                "finance_priority": "urgent"
            })),
            "incident_classifier" => Ok(json!({
                "severity": if input["region"] == "eu" { "sev1" } else { "sev2" },
                "region": input["region"].as_str().unwrap(),
                "customer_visible": true
            })),
            "executive_summarizer" => Ok(json!({
                "summary": format!(
                    "{} | {} | {}",
                    input["report"]["summary"].as_str().unwrap(),
                    input["incident"]["severity"].as_str().unwrap(),
                    input["retention"]["churn_risk"].as_str().unwrap()
                ),
                "actions": [
                    input["report"]["recommended_action"].as_str().unwrap(),
                    input["retention"]["offer"].as_str().unwrap(),
                    "open incident review and refund workflow"
                ],
                "approvals": input["compliance"]["policy_refs"].as_array().unwrap(),
                "escalation_required": true,
                "owners": [
                    input["report"]["owner"].as_str().unwrap(),
                    input["retention"]["owner"].as_str().unwrap(),
                    "finance_ops"
                ]
            })),
            "link_transform_echo" => Ok(input.clone()),
            "semantic_adapter" => {
                assert_eq!(
                    input["target_schema"]["required"],
                    json!(["research_brief", "topic", "topic_count", "owner", "notes"])
                );
                let raw = &input["raw"];
                let topics = raw["topics"].as_array().unwrap();
                Ok(json!({
                    "research_brief": raw["brief"],
                    "topic": topics[0],
                    "topic_count": topics.len(),
                    "owner": "semantic_research_ops",
                    "notes": [
                        format!("adapted: {}", topics[0].as_str().unwrap()),
                        format!("evidence: {}", topics[1].as_str().unwrap())
                    ]
                }))
            }
            other => panic!("unexpected model {other}"),
        }
    }
}

#[derive(Default)]
struct ParallelProbeTools;

impl ToolProvider for ParallelProbeTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        panic!("unexpected tool {name} with {input}")
    }
}

struct ParallelProbeModels {
    current: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
}

impl ModelProvider for ParallelProbeModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "reasoning");
        let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_seen.fetch_max(current, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(json!({
            "content": input.as_str().unwrap_or_default()
        }))
    }
}

#[derive(Default)]
struct SmokeModels;

impl ModelProvider for SmokeModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        assert_eq!(name, "reasoning");
        Ok(json!({
            "content": input.as_str().unwrap_or_default()
        }))
    }
}

#[derive(Default)]
struct DeepResearchTools;

impl ToolProvider for DeepResearchTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "web.search" => Ok(json!({
                "query": input["query"].as_str().unwrap(),
                "documents": [
                    {
                        "id": format!("doc:{}", input["query"].as_str().unwrap()),
                        "title": format!("Evidence for {}", input["query"].as_str().unwrap()),
                        "content": "market adoption, supply-chain readiness, cost curves, and cited analyst notes"
                    }
                ]
            })),
            "research.think" => Ok(json!({
                "reflection": input["reflection"].as_str().unwrap(),
            })),
            other => panic!("unexpected tool {other} with {input}"),
        }
    }
}

struct ParallelDeepResearchTools {
    current: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
}

impl ToolProvider for ParallelDeepResearchTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "web.search" => {
                let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_seen.fetch_max(current, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                self.current.fetch_sub(1, Ordering::SeqCst);
                Ok(json!({
                    "query": input["query"].as_str().unwrap(),
                    "documents": [
                        {
                            "id": format!("doc:{}", input["query"].as_str().unwrap()),
                            "title": format!("Evidence for {}", input["query"].as_str().unwrap()),
                            "content": "market adoption, supply-chain readiness, cost curves, and cited analyst notes"
                        }
                    ]
                }))
            }
            "research.think" => Ok(json!({
                "reflection": input["reflection"].as_str().unwrap(),
            })),
            other => panic!("unexpected tool {other} with {input}"),
        }
    }
}

#[derive(Default)]
struct DeepResearchModels {
    refiner_calls: BTreeMap<String, usize>,
}

impl ModelProvider for DeepResearchModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "research_clarifier" => Ok(json!({
                "needs_clarification": false,
                "question": "",
                "verification": "Research scope is specific enough to proceed.",
                "normalized_question": format!(
                    "{} with explicit 2026-2030 commercial readiness criteria",
                    input["question"].as_str().unwrap()
                ),
                "assumptions": [
                    "Compare technology readiness for automaker product planning.",
                    "Prefer bounded topic fan-out over runtime graph mutation."
                ]
            })),
            "research_planner" => Ok(json!({
                "research_brief": format!("Research brief for {}", input["question"].as_str().unwrap()),
                "topic_1": "800V EV platform commercialization readiness",
                "topic_2": "silicon carbide drive supply-chain and cost readiness",
                "topic_3": "solid-state batteries versus distributed drive adoption timeline"
            })),
            "research_planner_array" => Ok(json!({
                "research_brief": format!("Research brief for {}", input["question"].as_str().unwrap()),
                "topics": [
                    "800V EV platform commercialization readiness",
                    "silicon carbide drive supply-chain and cost readiness",
                    "solid-state battery volume manufacturing readiness",
                    "distributed drive adoption timeline"
                ]
            })),
            "research_compressor" => Ok(json!({
                "topic": input["topic"].as_str().unwrap(),
                "summary": format!(
                    "{} :: {} ({})",
                    input["topic"].as_str().unwrap(),
                    input["evidence"]["latest_search"]["documents"][0]["content"].as_str().unwrap(),
                    input["evidence"]["final_direction"]["rationale"].as_str().unwrap()
                ),
                "sources": [
                    format!("doc:{}", input["topic"].as_str().unwrap())
                ]
            })),
            "research_refiner" => {
                let topic = input["topic"].as_str().unwrap().to_string();
                let calls = self.refiner_calls.entry(topic.clone()).or_default();
                *calls += 1;
                Ok(json!({
                    "complete": *calls >= 2,
                    "follow_up_query": if *calls >= 2 {
                        String::new()
                    } else {
                        format!("{topic} follow-up risks")
                    },
                    "rationale": if *calls >= 2 {
                        "Enough bounded evidence has been gathered."
                    } else {
                        "Probe the weakest readiness assumption from the latest search."
                    }
                }))
            }
            "research_supervisor" => Ok(json!({
                "action": "research_complete",
                "needs_more": false,
                "follow_up_topics": ["", ""],
                "rationale": "Initial bounded notes are sufficient for the final report."
            })),
            "final_reporter" => {
                let notes = input["notes"].as_array().unwrap();
                let findings = notes
                    .iter()
                    .enumerate()
                    .map(|(index, note)| {
                        format!("{}. {}", index + 1, note["summary"].as_str().unwrap())
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(json!({
                    "report": format!(
                        "{}\n\nFindings:\n{}",
                        input["research_brief"].as_str().unwrap(),
                        findings
                    ),
                    "sources": notes.iter().map(|note| json!(note["sources"][0].as_str().unwrap())).collect::<Vec<_>>(),
                    "limitations": [
                        "AIR RunPlan instantiates bounded researchers before execution; unbounded runtime fanout remains a future IR feature."
                    ]
                }))
            }
            other => panic!("unexpected model {other} with {input}"),
        }
    }
}

#[derive(Default)]
struct ShortTopicPlannerModels;

impl ModelProvider for ShortTopicPlannerModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "research_planner_array" => Ok(json!({
                "research_brief": format!("Research brief for {}", input["question"].as_str().unwrap()),
                "topics": [
                    "800V EV platform commercialization readiness",
                    "silicon carbide drive supply-chain and cost readiness",
                    "solid-state battery volume manufacturing readiness"
                ]
            })),
            other => DeepResearchModels::default().call_model(other, input),
        }
    }
}

#[derive(Default)]
struct ClarificationNeededModels;

impl ModelProvider for ClarificationNeededModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "research_clarifier" => Ok(json!({
                "needs_clarification": true,
                "question": "Which vehicle segments and geographies should this research prioritize?",
                "verification": "",
                "normalized_question": input["question"].as_str().unwrap(),
                "assumptions": []
            })),
            other => DeepResearchModels::default().call_model(other, input),
        }
    }
}

#[derive(Default)]
struct MoreResearchSupervisorModels {
    inner: DeepResearchModels,
}

impl ModelProvider for MoreResearchSupervisorModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "research_supervisor" => Ok(json!({
                "action": "conduct_research",
                "needs_more": true,
                "follow_up_topics": [
                    "solid-state battery volume manufacturing readiness",
                    "distributed drive adoption timeline"
                ],
                "rationale": "Run the bounded second wave to cover remaining topics."
            })),
            other => self.inner.call_model(other, input),
        }
    }
}

#[derive(Default)]
struct TwoWaveThenCompleteSupervisorModels {
    inner: DeepResearchModels,
    supervisor_calls: usize,
}

impl ModelProvider for TwoWaveThenCompleteSupervisorModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "research_supervisor" => {
                self.supervisor_calls += 1;
                match self.supervisor_calls {
                    1 => {
                        assert_eq!(input["notes"].as_array().unwrap().len(), 2);
                        Ok(json!({
                            "action": "conduct_research",
                            "needs_more": true,
                            "follow_up_topics": [
                                "solid-state battery volume manufacturing readiness",
                                "distributed drive adoption timeline"
                            ],
                            "rationale": "Run the bounded second wave to cover remaining topics."
                        }))
                    }
                    2 => {
                        assert_eq!(input["notes"].as_array().unwrap().len(), 4);
                        Ok(json!({
                            "action": "research_complete",
                            "needs_more": false,
                            "follow_up_topics": ["", ""],
                            "rationale": "The second wave closed the remaining gaps."
                        }))
                    }
                    other => panic!("unexpected supervisor call {other}"),
                }
            }
            other => self.inner.call_model(other, input),
        }
    }
}

#[derive(Default)]
struct ThirdWaveSupervisorModels {
    inner: DeepResearchModels,
    supervisor_calls: usize,
}

impl ModelProvider for ThirdWaveSupervisorModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "research_supervisor" => {
                self.supervisor_calls += 1;
                match self.supervisor_calls {
                    1 => {
                        assert_eq!(input["notes"].as_array().unwrap().len(), 2);
                        Ok(json!({
                            "action": "conduct_research",
                            "needs_more": true,
                            "follow_up_topics": [
                                "solid-state battery volume manufacturing readiness",
                                "distributed drive adoption timeline"
                            ],
                            "rationale": "Run the bounded second wave to cover remaining topics."
                        }))
                    }
                    2 => {
                        assert_eq!(input["notes"].as_array().unwrap().len(), 4);
                        Ok(json!({
                            "action": "conduct_research",
                            "needs_more": true,
                            "follow_up_topics": [
                                "battery recycling and end-of-life readiness",
                                "charging infrastructure readiness for high-voltage platforms"
                            ],
                            "rationale": "Run the bounded third wave for lifecycle and infrastructure gaps."
                        }))
                    }
                    other => panic!("unexpected supervisor call {other}"),
                }
            }
            other => self.inner.call_model(other, input),
        }
    }
}

#[test]
fn links_and_runs_schema_contract_system() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let system =
        parse_system_file(root.join("tests/systems/schema-contract.air-system.yaml")).unwrap();

    let result = run_system(
        &system,
        &root,
        State::from_iter([("text".to_string(), json!("billing issue"))]),
        MockTools,
        MockModels,
    )
    .unwrap();

    assert_eq!(result.outputs["report"]["approved"], json!(true));
    assert_eq!(
        result.outputs["report"]["summary"],
        json!("high: duplicate billing with churn risk")
    );
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(
        result.module_outputs["extract"]["extracted"]["product_area"],
        "billing"
    );
    assert_eq!(
        result.module_outputs["score"]["score"]["risk_score"],
        json!(0.86)
    );

    let trace_agents: Vec<_> = result
        .trace
        .iter()
        .map(|event| event.agent.as_str())
        .collect();
    assert_eq!(
        trace_agents,
        [
            "extract-agent",
            "extract-agent",
            "extract-agent",
            "extract-agent",
            "extract-agent",
            "score-agent",
            "score-agent",
            "score-agent",
            "score-agent",
            "score-agent",
            "report-agent",
            "report-agent",
            "report-agent",
            "report-agent",
            "report-agent",
        ]
    );
}

#[test]
fn schedule_groups_execute_modules_in_parallel_with_provider_factories() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let module_ref = ModuleRef {
        path: root.join("tests/agents/model-smoke.air.yaml"),
        kind: ModuleKind::Primitive,
        visibility: ModuleVisibility::Public,
        description: None,
        tags: vec![],
        covers: vec![],
        priority: 0,
    };
    let system = AirSystem {
        system: SystemMetadata {
            name: "parallel-smoke".to_string(),
            version: "0.1.0".to_string(),
        },
        modules: BTreeMap::from([
            ("left".to_string(), module_ref.clone()),
            ("right".to_string(), module_ref),
        ]),
        entry: "left".to_string(),
        edges: vec![],
        connect: vec![
            air_linker::Connection {
                from: Some("$input.prompt".to_string()),
                to: "left.prompt".to_string(),
                value: None,
            },
            air_linker::Connection {
                from: Some("$input.prompt".to_string()),
                to: "right.prompt".to_string(),
                value: None,
            },
        ],
        outputs: BTreeMap::from([
            ("left".to_string(), "left.response".to_string()),
            ("right".to_string(), "right.response".to_string()),
        ]),
        halts: vec![],
        node_conditions: BTreeMap::new(),
        schedule: Some(SystemSchedule {
            max_parallel: Some(2),
            groups: vec![ScheduleGroup {
                id: "parallel_wave".to_string(),
                nodes: vec!["left".to_string(), "right".to_string()],
                max_parallel: Some(2),
            }],
        }),
    };
    let current = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    let result = run_system_parallel_with_resume_with_observer_and_checkpoint(
        &system,
        &root,
        State::from_iter([("prompt".to_string(), json!("parallel"))]),
        ResumeState::default(),
        || ParallelProbeTools,
        || ParallelProbeModels {
            current: current.clone(),
            max_seen: max_seen.clone(),
        },
        |_| {},
        |_| Ok(()),
    )
    .unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.outputs["left"]["content"], json!("parallel"));
    assert_eq!(result.outputs["right"]["content"], json!("parallel"));
    assert_eq!(max_seen.load(Ordering::SeqCst), 2);
    assert!(result.trace.iter().any(|event| {
        event.agent == "$system"
            && event.action == "schedule_batch"
            && event.input.as_ref().unwrap()["execution"] == "parallel"
    }));
}

#[test]
fn specializes_run_plan_from_trace_and_builds_cache_identity() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("tests/plans/parallel-smoke.air-store.yaml")).unwrap();
    let plan = parse_run_plan_file(root.join("tests/plans/parallel-smoke.air-plan.yaml")).unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([("prompt".to_string(), json!("jit"))]),
        ParallelProbeTools,
        SmokeModels,
    )
    .unwrap();
    let specialized = specialize_run_plan_trace(&result.trace, &store, &root).unwrap();

    assert_eq!(specialized.plan, plan);
    assert_eq!(
        specialized.cache_identity["kind"],
        json!("air.trace_specialization.v1")
    );
    assert_eq!(
        specialized.cache_identity["nodes"]["left"]["store_module"],
        json!("test.model_smoke@0.1.0")
    );
    assert_eq!(
        specialized.cache_identity["nodes"]["left"]["inputs"]["prompt"],
        json!("string")
    );
}

#[test]
fn resolves_and_runs_deep_research_run_plan() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(root.join("examples/deep-research/deep-research.air-plan.yaml"))
        .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert!(result.outputs["final_report"]["report"]
        .as_str()
        .unwrap()
        .contains("Findings"));
    assert_eq!(
        result.outputs["final_report"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        result.module_outputs["plan"]["plan"]["topic_1"],
        json!("800V EV platform commercialization readiness")
    );

    for agent in [
        "deep-research-plan-agent",
        "deep-research-topic-agent",
        "deep-research-final-report-agent",
    ] {
        assert!(
            result.trace.iter().any(|event| event.agent == agent),
            "expected trace from {agent}"
        );
    }

    let tool_calls = result
        .trace
        .iter()
        .filter(|event| event.action == "tool_call")
        .count();
    assert_eq!(tool_calls, 12);

    let researcher_returns = result
        .trace
        .iter()
        .filter(|event| event.agent == "deep-research-topic-agent" && event.action == "return")
        .count();
    assert_eq!(researcher_returns, 3);

    let model_calls = result
        .trace
        .iter()
        .filter(|event| event.action == "model_call")
        .count();
    assert_eq!(model_calls, 11);
}

#[test]
fn run_plan_connect_value_supports_typed_transforms() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("tests/plans/link-transform.air-store.yaml")).unwrap();
    let plan = parse_run_plan_file(root.join("tests/plans/link-transform.air-plan.yaml")).unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([("default_owner".to_string(), json!("research_operations"))]),
        MockTools,
        MockModels,
    )
    .unwrap();

    assert_eq!(
        result.outputs["request"],
        json!({
            "research_brief": "Research migration readiness",
            "topic": "dynamic fan-out",
            "topic_count": 2,
            "owner": "research_operations",
            "notes": ["planner", "typed transforms"]
        })
    );
}

#[test]
fn run_plan_uses_semantic_adapter_module_for_complex_interface_conversion() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("tests/plans/semantic-adapter.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/semantic-adapter.air-plan.yaml")).unwrap();

    let result = run_run_plan(&plan, &store, &root, State::new(), MockTools, MockModels).unwrap();

    assert_eq!(
        result.outputs["adapted"],
        json!({
            "research_brief": "Research migration readiness",
            "topic": "dynamic fan-out",
            "topic_count": 2,
            "owner": "semantic_research_ops",
            "notes": ["adapted: dynamic fan-out", "evidence: typed transforms"]
        })
    );
    assert_eq!(result.outputs["request"], result.outputs["adapted"]);

    assert!(result.trace.iter().any(|event| {
        event.agent == "semantic-adapter-agent"
            && event.action == "model_call"
            && event.output.as_ref() == Some(&result.outputs["adapted"])
    }));
}

#[test]
fn runs_deep_research_dynamic_topic_fanout_run_plan() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(
        result.outputs["final_report"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    for node in ["topic_1", "topic_2", "topic_3", "topic_4"] {
        assert!(
            result.module_outputs.contains_key(node),
            "expected dynamic node {node}"
        );
    }
    assert!(result.trace.iter().any(|event| {
        event.agent == "topic_1/deep-research-topic-agent" && event.action == "model_call"
    }));
    assert!(result.trace.iter().any(|event| {
        event.agent == "$planner"
            && event.rule == "dynamic_fanout"
            && event.action == "materialize"
            && event.output.as_ref().unwrap()["nodes"]
                .as_array()
                .unwrap()
                .len()
                == 4
    }));
    assert!(result.trace.iter().any(|event| {
        event.agent == "$planner"
            && event.rule == "dynamic_fanout"
            && event.action == "resolve_dynamic_plan"
    }));
}

#[test]
fn dynamic_topic_fanout_respects_parallel_runner() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();
    let current = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    let result = run_run_plan_parallel_with_observer_and_checkpoint(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        || ParallelDeepResearchTools {
            current: current.clone(),
            max_seen: max_seen.clone(),
        },
        DeepResearchModels::default,
        |_| {},
        |_| Ok(()),
    )
    .unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert!(
        max_seen.load(Ordering::SeqCst) > 1,
        "expected dynamic fanout researchers to overlap"
    );
    assert!(result.trace.iter().any(|event| {
        event.agent == "$system"
            && event.rule == "dynamic_fanout"
            && event.action == "schedule_batch"
            && event.input.as_ref().unwrap()["execution"] == "parallel"
    }));
}

#[test]
fn runs_multiple_dynamic_fanout_boundaries_before_final_fan_in() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("tests/plans/dynamic-smoke.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/dynamic-multiple-boundaries.air-plan.yaml"))
            .unwrap();

    let result = run_run_plan(&plan, &store, &root, State::new(), MockTools, SmokeModels).unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.outputs["report"], json!("done"));
    for node in ["topic_a_1", "topic_a_2", "topic_b_1", "topic_b_2"] {
        assert!(
            result.module_outputs.contains_key(node),
            "expected dynamic node {node}"
        );
    }
    let materialize_events = result
        .trace
        .iter()
        .filter(|event| event.agent == "$planner" && event.action == "materialize")
        .count();
    assert_eq!(materialize_events, 2);
    let specialized = specialize_run_plan_trace(&result.trace, &store, &root).unwrap();
    assert!(specialized.plan.dynamic.is_none());
    assert!(specialized
        .plan
        .nodes
        .iter()
        .any(|node| node.id == "topic_b_2"));
}

#[test]
fn runs_nested_dynamic_fanout_after_parent_fanout() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("tests/plans/dynamic-smoke.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/dynamic-nested-fanout.air-plan.yaml")).unwrap();

    let result = run_run_plan(&plan, &store, &root, State::new(), MockTools, SmokeModels).unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.outputs["report"], json!("done"));
    for node in ["parent_1", "parent_2", "child_1_1", "child_2_1"] {
        assert!(
            result.module_outputs.contains_key(node),
            "expected dynamic node {node}"
        );
    }
    let specialized = specialize_run_plan_trace(&result.trace, &store, &root).unwrap();
    assert!(specialized.plan.dynamic.is_none());
    assert!(specialized
        .plan
        .connect
        .iter()
        .any(
            |connection| connection.from.as_deref() == Some("parent_1.followups[0]")
                && connection.to == "child_1_1.topic"
        ));
}

#[test]
fn resumes_dynamic_topic_fanout_after_planner_checkpoint() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();
    let inputs = State::from_iter([(
        "question".to_string(),
        json!("compare EV technology readiness for 2026-2030 product plans"),
    )]);
    let mut checkpoints = Vec::new();

    let completed = run_run_plan_with_observer_and_checkpoint(
        &plan,
        &store,
        &root,
        inputs.clone(),
        DeepResearchTools,
        DeepResearchModels::default(),
        |_| {},
        |checkpoint| {
            checkpoints.push(checkpoint.clone());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(completed.status, RunStatus::Completed);
    assert_eq!(checkpoints[0].completed_module, "plan");

    let resumed = resume_run_plan(
        &plan,
        &store,
        &root,
        inputs,
        ResumeState {
            module_outputs: checkpoints[0].module_outputs.clone(),
            output_overrides: BTreeMap::new(),
        },
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert_eq!(resumed.status, RunStatus::Completed);
    assert_eq!(
        resumed.outputs["final_report"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(resumed.module_outputs.contains_key("topic_4"));
    assert!(resumed.trace.iter().any(|event| {
        event.agent == "$planner" && event.rule == "run_plan" && event.action == "resume"
    }));
}

#[test]
fn resumes_dynamic_topic_fanout_after_partial_researcher_checkpoint() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();
    let inputs = State::from_iter([(
        "question".to_string(),
        json!("compare EV technology readiness for 2026-2030 product plans"),
    )]);
    let mut checkpoints = Vec::new();

    run_run_plan_with_observer_and_checkpoint(
        &plan,
        &store,
        &root,
        inputs.clone(),
        DeepResearchTools,
        DeepResearchModels::default(),
        |_| {},
        |checkpoint| {
            checkpoints.push(checkpoint.clone());
            Ok(())
        },
    )
    .unwrap();
    let topic_2_checkpoint = checkpoints
        .iter()
        .find(|checkpoint| checkpoint.completed_module == "topic_2")
        .expect("topic_2 checkpoint");

    let resumed = resume_run_plan(
        &plan,
        &store,
        &root,
        inputs,
        ResumeState {
            module_outputs: topic_2_checkpoint.module_outputs.clone(),
            output_overrides: BTreeMap::new(),
        },
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert_eq!(resumed.status, RunStatus::Completed);
    assert!(resumed.module_outputs.contains_key("topic_1"));
    assert!(resumed.module_outputs.contains_key("topic_4"));
    assert!(!resumed
        .trace
        .iter()
        .any(|event| event.agent == "topic_1/deep-research-topic-agent"));
    assert!(resumed
        .trace
        .iter()
        .any(|event| event.agent == "topic_3/deep-research-topic-agent"));
}

#[test]
fn specializes_dynamic_run_plan_trace_to_static_hot_path() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();
    let specialized = specialize_run_plan_trace(&result.trace, &store, &root).unwrap();

    assert!(specialized.plan.dynamic.is_none());
    assert_eq!(specialized.plan.nodes.len(), 6);
    assert!(specialized
        .plan
        .nodes
        .iter()
        .any(|node| node.id == "topic_4"));
    assert_eq!(
        specialized.cache_identity["nodes"]["topic_1"]["store_module"],
        json!("deep_research.research_topic@0.1.0")
    );
}

#[test]
fn resolves_and_runs_deep_research_array_topic_run_plan() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan =
        parse_run_plan_file(root.join("examples/deep-research/deep-research-array.air-plan.yaml"))
            .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert_eq!(
        result.module_outputs["plan"]["plan"]["topics"][3],
        json!("distributed drive adoption timeline")
    );
    assert_eq!(
        result.outputs["final_report"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let researcher_returns = result
        .trace
        .iter()
        .filter(|event| event.agent == "deep-research-topic-agent" && event.action == "return")
        .count();
    assert_eq!(researcher_returns, 4);

    let tool_calls = result
        .trace
        .iter()
        .filter(|event| event.action == "tool_call")
        .count();
    assert_eq!(tool_calls, 16);

    let model_calls = result
        .trace
        .iter()
        .filter(|event| event.action == "model_call")
        .count();
    assert_eq!(model_calls, 14);
    assert!(result.trace.iter().any(|event| {
        event.agent == "$system"
            && event.action == "schedule_batch"
            && event.input.as_ref().is_some_and(|input| {
                input["group"] == json!("initial_research_wave")
                    && input["nodes"]
                        .as_array()
                        .is_some_and(|nodes| nodes.len() == 4)
            })
    }));
}

#[test]
fn rejects_deep_research_plan_array_with_too_few_topics() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan =
        parse_run_plan_file(root.join("examples/deep-research/deep-research-array.air-plan.yaml"))
            .unwrap();

    let error = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        ShortTopicPlannerModels,
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("expected at least 4 items got 3"),
        "unexpected error: {error}"
    );
}

#[test]
fn checkpoints_deep_research_plan_after_each_completed_module_and_resumes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan =
        parse_run_plan_file(root.join("examples/deep-research/deep-research-array.air-plan.yaml"))
            .unwrap();
    let inputs = State::from_iter([(
        "question".to_string(),
        json!("compare EV technology readiness for 2026-2030 product plans"),
    )]);
    let mut checkpoints = Vec::new();

    let result = run_run_plan_with_observer_and_checkpoint(
        &plan,
        &store,
        &root,
        inputs.clone(),
        DeepResearchTools,
        DeepResearchModels::default(),
        |_| {},
        |checkpoint| {
            checkpoints.push(checkpoint.clone());
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(checkpoints.len(), 6);
    assert_eq!(checkpoints[0].completed_module, "plan");
    assert!(checkpoints[0].module_outputs.contains_key("plan"));
    assert_eq!(checkpoints.last().unwrap().completed_module, "final");

    let resumed = resume_run_plan(
        &plan,
        &store,
        &root,
        inputs,
        ResumeState {
            module_outputs: checkpoints[0].module_outputs.clone(),
            output_overrides: BTreeMap::new(),
        },
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert_eq!(resumed.status, RunStatus::Completed);
    assert_eq!(
        resumed.outputs["final_report"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(!resumed.trace.iter().any(|event| {
        event.agent == "deep-research-plan-agent" && event.action == "model_call"
    }));
}

#[test]
fn resolves_and_runs_deep_research_clarified_run_plan() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-clarified.air-plan.yaml"),
    )
    .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert_eq!(
        result.outputs["clarification"]["needs_clarification"],
        json!(false)
    );
    assert!(result.outputs["clarification"]["normalized_question"]
        .as_str()
        .unwrap()
        .contains("commercial readiness criteria"));
    assert_eq!(
        result.module_outputs["plan"]["plan"]["research_brief"],
        json!(
            "Research brief for compare EV technology readiness for 2026-2030 product plans with explicit 2026-2030 commercial readiness criteria"
        )
    );
    assert_eq!(
        result.outputs["result"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let tool_calls = result
        .trace
        .iter()
        .filter(|event| event.action == "tool_call")
        .count();
    assert_eq!(tool_calls, 16);

    let model_calls = result
        .trace
        .iter()
        .filter(|event| event.action == "model_call")
        .count();
    assert_eq!(model_calls, 15);
    assert_eq!(result.status, RunStatus::Completed);
}

#[test]
fn supervised_deep_research_skips_second_wave_when_not_needed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-supervised.air-plan.yaml"),
    )
    .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(
        result.outputs["supervisor_decision"]["needs_more"],
        json!(false)
    );
    assert_eq!(
        result.outputs["supervisor_decision"]["action"],
        json!("research_complete")
    );
    assert!(result.module_outputs.contains_key("topic_1"));
    assert!(result.module_outputs.contains_key("topic_2"));
    assert!(!result.module_outputs.contains_key("topic_3"));
    assert!(!result.module_outputs.contains_key("topic_4"));
    assert_eq!(
        result.outputs["result"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "skip")
            .count(),
        2
    );
}

#[test]
fn supervised_deep_research_runs_second_wave_when_needed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-supervised.air-plan.yaml"),
    )
    .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        MoreResearchSupervisorModels::default(),
    )
    .unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(
        result.outputs["supervisor_decision"]["needs_more"],
        json!(true)
    );
    assert_eq!(
        result.outputs["supervisor_decision"]["action"],
        json!("conduct_research")
    );
    assert!(result.module_outputs.contains_key("topic_3"));
    assert!(result.module_outputs.contains_key("topic_4"));
    assert_eq!(
        result.module_outputs["topic_3"]["note"]["topic"],
        json!("solid-state battery volume manufacturing readiness")
    );
    assert_eq!(
        result.module_outputs["topic_4"]["note"]["topic"],
        json!("distributed drive adoption timeline")
    );
    assert_eq!(
        result.outputs["result"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "skip")
            .count(),
        0
    );
}

#[test]
fn two_step_supervised_deep_research_stops_after_second_wave_when_complete() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-supervised-two-step.air-plan.yaml"),
    )
    .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        TwoWaveThenCompleteSupervisorModels::default(),
    )
    .unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert!(result.module_outputs.contains_key("supervisor_1"));
    assert!(result.module_outputs.contains_key("supervisor_2"));
    assert!(result.module_outputs.contains_key("topic_3"));
    assert!(result.module_outputs.contains_key("topic_4"));
    assert!(!result.module_outputs.contains_key("topic_5"));
    assert!(!result.module_outputs.contains_key("topic_6"));
    assert_eq!(
        result.module_outputs["supervisor_2"]["decision"]["action"],
        json!("research_complete")
    );
    assert_eq!(
        result.outputs["result"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "skip")
            .count(),
        2
    );
}

#[test]
fn two_step_supervised_deep_research_runs_third_wave_when_needed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-supervised-two-step.air-plan.yaml"),
    )
    .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("compare EV technology readiness for 2026-2030 product plans"),
        )]),
        DeepResearchTools,
        ThirdWaveSupervisorModels::default(),
    )
    .unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert!(result.module_outputs.contains_key("topic_5"));
    assert!(result.module_outputs.contains_key("topic_6"));
    assert_eq!(
        result.module_outputs["topic_5"]["note"]["topic"],
        json!("battery recycling and end-of-life readiness")
    );
    assert_eq!(
        result.module_outputs["topic_6"]["note"]["topic"],
        json!("charging infrastructure readiness for high-voltage platforms")
    );
    assert_eq!(
        result.outputs["result"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(
                |event| event.agent == "deep-research-supervisor-agent" && event.action == "return"
            )
            .count(),
        2
    );
    assert_eq!(
        result
            .trace
            .iter()
            .filter(|event| event.action == "skip")
            .count(),
        0
    );
}

#[test]
fn halts_deep_research_when_clarification_is_needed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-clarified.air-plan.yaml"),
    )
    .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("research EV technology readiness"),
        )]),
        DeepResearchTools,
        ClarificationNeededModels,
    )
    .unwrap();

    assert_eq!(
        result.outputs["clarification"]["needs_clarification"],
        json!(true)
    );
    assert!(result.outputs.get("result").is_none());
    assert_eq!(
        result.status,
        RunStatus::Halted {
            after: "clarify".to_string(),
            reference: "clarify.clarification.needs_clarification".to_string(),
            equals: json!(true),
        }
    );
    assert!(result.module_outputs.contains_key("clarify"));
    assert!(!result.module_outputs.contains_key("plan"));
    assert!(result
        .trace
        .iter()
        .any(|event| event.agent == "$system" && event.action == "halt"));

    let model_calls = result
        .trace
        .iter()
        .filter(|event| event.action == "model_call")
        .count();
    assert_eq!(model_calls, 1);
}

#[test]
fn resumes_deep_research_after_clarification_halt() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-clarified.air-plan.yaml"),
    )
    .unwrap();

    let halted = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("research EV technology readiness"),
        )]),
        DeepResearchTools,
        ClarificationNeededModels,
    )
    .unwrap();

    assert!(matches!(halted.status, RunStatus::Halted { .. }));

    let resumed = resume_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([(
            "question".to_string(),
            json!("research EV technology readiness"),
        )]),
        ResumeState {
            module_outputs: halted.module_outputs,
            output_overrides: BTreeMap::from([(
                "clarify.clarification.normalized_question".to_string(),
                json!("compare EV technology readiness for North America passenger vehicles in 2026-2030"),
            )]),
        },
        DeepResearchTools,
        DeepResearchModels::default(),
    )
    .unwrap();

    assert_eq!(resumed.status, RunStatus::Completed);
    assert_eq!(
        resumed.module_outputs["plan"]["plan"]["research_brief"],
        json!(
            "Research brief for compare EV technology readiness for North America passenger vehicles in 2026-2030"
        )
    );
    assert_eq!(
        resumed.outputs["result"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(resumed
        .trace
        .iter()
        .any(|event| event.agent == "$planner" && event.action == "resume"));
    assert!(!resumed.trace.iter().any(|event| {
        event.agent == "deep-research-clarify-scope-agent" && event.action == "model_call"
    }));

    let tool_calls = resumed
        .trace
        .iter()
        .filter(|event| event.action == "tool_call")
        .count();
    assert_eq!(tool_calls, 16);

    let model_calls = resumed
        .trace
        .iter()
        .filter(|event| event.action == "model_call")
        .count();
    assert_eq!(model_calls, 14);
}

#[test]
fn notifies_observer_across_linked_modules() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let system =
        parse_system_file(root.join("tests/systems/schema-contract.air-system.yaml")).unwrap();
    let mut observed = Vec::new();

    let result = run_system_with_observer(
        &system,
        &root,
        State::from_iter([("text".to_string(), json!("billing issue"))]),
        MockTools,
        MockModels,
        |event| observed.push((event.agent.clone(), event.action.clone())),
    )
    .unwrap();

    let trace_summary: Vec<_> = result
        .trace
        .iter()
        .map(|event| (event.agent.clone(), event.action.clone()))
        .collect();
    assert_eq!(observed, trace_summary);
    assert_eq!(observed.first().unwrap().0, "extract-agent");
    assert_eq!(observed.last().unwrap().0, "report-agent");
}

#[test]
fn resolves_and_runs_dynamic_run_plan() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("tests/plans/module-store.air-store.yaml")).unwrap();
    let plan = parse_run_plan_file(root.join("tests/plans/schema-contract.air-plan.yaml")).unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([("text".to_string(), json!("billing issue"))]),
        MockTools,
        MockModels,
    )
    .unwrap();

    assert_eq!(result.outputs["report"]["approved"], json!(true));
    assert_eq!(result.trace.first().unwrap().agent, "$planner");
    assert_eq!(result.trace.first().unwrap().action, "resolve_plan");
    assert!(result
        .trace
        .iter()
        .any(|event| event.agent == "$planner" && event.action == "decision"));
    assert_eq!(
        result.module_outputs["score"]["score"]["priority"],
        json!("high")
    );
}

#[test]
fn resolves_and_runs_public_composite_run_plan() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("tests/plans/module-store.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/customer-triage-composite.air-plan.yaml"))
            .unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([("text".to_string(), json!("billing issue"))]),
        MockTools,
        MockModels,
    )
    .unwrap();

    assert_eq!(result.outputs["report"]["approved"], json!(true));
    assert_eq!(
        result.module_outputs["triage"]["report"]["owner"],
        json!("billing_support")
    );
    assert!(result
        .trace
        .iter()
        .any(|event| event.agent == "$planner" && event.action == "decision"));
    assert!(result
        .trace
        .iter()
        .any(|event| event.agent == "customer-triage-agent"));
}

#[test]
fn resolves_and_runs_complex_escalation_run_plan() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store =
        parse_module_store_file(root.join("tests/plans/module-store.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/complex-escalation.air-plan.yaml")).unwrap();

    let result = run_run_plan(
        &plan,
        &store,
        &root,
        State::from_iter([
            (
                "text".to_string(),
                json!("enterprise double charge cancellation threat"),
            ),
            ("account_tier".to_string(), json!("enterprise")),
            ("region".to_string(), json!("eu")),
            ("invoice_count".to_string(), json!(2)),
        ]),
        MockTools,
        MockModels,
    )
    .unwrap();

    assert_eq!(
        result.outputs["final_case"]["escalation_required"],
        json!(true)
    );
    assert_eq!(
        result.module_outputs["incident"]["incident"]["severity"],
        json!("sev1")
    );
    assert_eq!(
        result.module_outputs["finance"]["finance"]["estimated_credit"],
        json!(200.0)
    );
    assert_eq!(
        result.module_outputs["retention"]["retention"]["churn_risk"],
        json!("critical")
    );

    let planner_decisions = result
        .trace
        .iter()
        .filter(|event| event.agent == "$planner" && event.action == "decision")
        .count();
    assert_eq!(planner_decisions, 3);

    for agent in [
        "customer-triage-agent",
        "compliance-check-agent",
        "retention-plan-agent",
        "finance-impact-agent",
        "incident-classify-agent",
        "executive-case-summary-agent",
    ] {
        assert!(
            result.trace.iter().any(|event| event.agent == agent),
            "expected trace from {agent}"
        );
    }

    let model_calls = result
        .trace
        .iter()
        .filter(|event| event.action == "model_call")
        .count();
    assert_eq!(model_calls, 8);
}
