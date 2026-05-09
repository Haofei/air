use air_runtime::{ModelProvider, RuntimeError, State, ToolProvider, Vm};
use serde_json::{json, Value};

#[derive(Default)]
struct MockTools;

impl ToolProvider for MockTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "github.read_pr" => Ok(json!({
                "repo": input,
                "title": "TDD AIR runtime",
                "diff": "+ add state machine VM"
            })),
            "slack.report" => Ok(json!({
                "channel": "#agents",
                "text": input["summary"],
                "sent": true
            })),
            other => panic!("unexpected tool {other}"),
        }
    }
}

#[derive(Default)]
struct MockModels;

impl ModelProvider for MockModels {
    fn call_model(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match name {
            "reviewer" => Ok(json!({
                "summary": format!("reviewed {}", input["title"].as_str().unwrap()),
                "risk_score": 0.2
            })),
            other => panic!("unexpected model {other}"),
        }
    }
}

#[test]
fn runs_three_state_machine_agents_with_mock_providers() {
    let read = load_agent("tests/agents/read-pr.air.yaml");
    let review = load_agent("tests/agents/review.air.yaml");
    let report = load_agent("tests/agents/report.air.yaml");

    for module in [&read, &review, &report] {
        let verification = air_verify::verify(module);
        assert!(
            verification.is_success(),
            "invalid fixture {}: {:?}",
            module.agent.name,
            verification.diagnostics
        );
    }

    let mut vm = Vm {
        tools: MockTools,
        models: MockModels,
    };

    let read_result = vm
        .run(
            &read,
            State::from_iter([
                ("repo".to_string(), json!("acme/app")),
                ("pr_number".to_string(), json!(42)),
            ]),
        )
        .unwrap();
    assert_eq!(
        read_result.outputs["pr_payload"]["title"],
        "TDD AIR runtime"
    );

    let review_result = vm
        .run(
            &review,
            State::from_iter([(
                "pr_payload".to_string(),
                read_result.outputs["pr_payload"].clone(),
            )]),
        )
        .unwrap();
    assert_eq!(review_result.outputs["review"]["risk_score"], json!(0.2));

    let report_result = vm
        .run(
            &report,
            State::from_iter([(
                "review".to_string(),
                review_result.outputs["review"].clone(),
            )]),
        )
        .unwrap();

    assert_eq!(report_result.outputs["report"]["sent"], json!(true));
    assert_eq!(
        report_result.outputs["report"]["text"],
        json!("reviewed TDD AIR runtime")
    );

    let trace_actions: Vec<_> = read_result
        .trace
        .iter()
        .chain(review_result.trace.iter())
        .chain(report_result.trace.iter())
        .map(|event| event.action.as_str())
        .collect();
    assert_eq!(
        trace_actions,
        [
            "set",
            "tool_call_start",
            "tool_call",
            "set",
            "return",
            "set",
            "model_call_start",
            "model_call",
            "set",
            "return",
            "set",
            "tool_call_start",
            "tool_call",
            "set",
            "return"
        ]
    );
}

fn load_agent(path: &str) -> air_core::AirModule {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    air_parser::parse_air_file(path).unwrap()
}
