use air_parser::parse_air_file;
use air_verify::verify;

#[test]
fn accepts_pr_review_example() {
    let module = parse_air_file("../../tests/agents/pr-review-example.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_composable_input_expressions() {
    let module = parse_air_file("../../tests/agents/expr-input.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_compound_state_machine_conditions() {
    let module = parse_air_file("../../tests/agents/conditional-loop.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_disjunctive_state_machine_conditions() {
    let module = parse_air_file("../../tests/agents/conditional-or.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_append_actions_to_state_arrays() {
    let module = parse_air_file("../../tests/agents/append-evidence.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_tool_dispatch_actions() {
    let module = parse_air_file("../../tests/agents/tool-dispatch.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_state_machine_approval_gate() {
    let module = parse_air_file("../../tests/agents/approval-gate.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_tool_dispatch_without_approval_path_for_required_capability() {
    let mut module = parse_air_file("../../tests/agents/tool-dispatch.air.yaml").unwrap();
    module
        .policy
        .require_approval
        .push("retrieval.local".to_string());

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR073"),
        "expected AIR073, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_approval_for_capability_not_required_by_policy() {
    let mut module = parse_air_file("../../tests/agents/approval-gate.air.yaml").unwrap();
    module.policy.require_approval.clear();

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR093"),
        "expected AIR093, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_append_actions_to_non_array_state() {
    let mut module = parse_air_file("../../tests/agents/append-evidence.air.yaml").unwrap();
    module.state.insert(
        "evidence".to_string(),
        air_core::TypeSpec::Shorthand(air_core::PrimitiveType::String),
    );

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR089"),
        "expected AIR089, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_model_or_tool_actions_that_write_phase() {
    let mut module = parse_air_file("../../tests/agents/model-smoke.air.yaml").unwrap();
    let air_core::Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let air_core::StateAction::ModelCall { output, .. } = &mut workflow.rules[1].actions[0] else {
        panic!("expected model_call");
    };
    *output = "phase".to_string();

    let report = verify(&module);

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
fn rejects_unsupported_state_machine_conditions() {
    let mut module = parse_air_file("../../tests/agents/conditional-loop.air.yaml").unwrap();
    let air_core::Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    workflow.rules[0].when = "phase starts_with \"init\"".to_string();

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR086"),
        "expected AIR086, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_expression_references_to_unknown_fields() {
    let module = parse_air_file("../../tests/fixtures-invalid-expr-ref.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR077"),
        "expected AIR077, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_zero_length_truncate_expression() {
    let mut module = parse_air_file("../../tests/agents/expr-input.air.yaml").unwrap();
    let air_core::Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let air_core::StateAction::ModelCall { input, .. } = &mut workflow.rules[1].actions[0] else {
        panic!("expected model call");
    };
    let air_core::InputSpec::Expr(air_core::Expr::Object { object }) = input else {
        panic!("expected object input");
    };
    object.insert(
        "bad".to_string(),
        air_core::Expr::Truncate {
            truncate: Box::new(air_core::Expr::Ref {
                reference: "text".to_string(),
            }),
            max_chars: 0,
        },
    );

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR090"),
        "expected AIR090, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_provider_token_limit_retry_policy() {
    let module = parse_air_file("../../tests/agents/model-token-limit-retry.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_unimplemented_dag_node_kinds_at_parse_time() {
    let result = serde_json::from_value::<air_core::AirModule>(serde_json::json!({
        "agent": {
            "name": "unimplemented-dag-node-agent",
            "version": "0.1.0"
        },
        "outputs": {
            "result": "object"
        },
        "workflow": {
            "kind": "dag",
            "entry": "branch",
            "nodes": [
                {
                    "id": "branch",
                    "kind": "branch"
                },
                {
                    "id": "done",
                    "kind": "return"
                }
            ],
            "edges": [
                {
                    "from": "branch",
                    "to": "done"
                }
            ]
        }
    }));

    assert!(result.is_err(), "branch node kind should not parse");
}

#[test]
fn rejects_zero_token_limit_retry_budget() {
    let mut module = parse_air_file("../../tests/agents/model-token-limit-retry.air.yaml").unwrap();
    let air_core::Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    let air_core::StateAction::ModelCall { retry, .. } = &mut workflow.rules[1].actions[0] else {
        panic!("expected model call");
    };
    retry
        .as_mut()
        .unwrap()
        .on_provider_error
        .as_mut()
        .unwrap()
        .token_limit
        .as_mut()
        .unwrap()
        .max_input_chars = vec![0];

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR092"),
        "expected AIR092, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_dangerous_capability_without_approval_path() {
    let module = parse_air_file("../../tests/fixtures-invalid-approval.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR043"),
        "expected AIR043, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_cycles_without_traversing_approval_paths_forever() {
    let module = parse_air_file("../../tests/fixtures-invalid-cycle.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR042"),
        "expected AIR042, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_state_machine_dangerous_capability_without_approval() {
    let module =
        parse_air_file("../../tests/fixtures-invalid-state-machine-approval.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR073"),
        "expected AIR073, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_state_machine_dangerous_capability_without_approval_on_one_branch() {
    let module =
        parse_air_file("../../tests/fixtures-invalid-state-machine-approval-branch.air.yaml")
            .unwrap();
    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR073"),
        "expected AIR073, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_state_machine_phase_values_outside_schema() {
    let module =
        parse_air_file("../../tests/fixtures-invalid-state-machine-phase.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR069"),
        "expected AIR069, got {:?}",
        report.diagnostics
    );
}
