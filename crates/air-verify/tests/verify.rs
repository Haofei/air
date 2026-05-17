use air_parser::parse_air_file;
use air_verify::verify;

fn parse_air_yaml(source: &str) -> air_core::AirModule {
    serde_yaml::from_str(source).unwrap()
}

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
fn accepts_array_index_paths_in_conditions() {
    let mut module = parse_air_file("../../tests/agents/tool-batch-dispatch.air.yaml").unwrap();
    let air_core::Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    workflow.rules[3].when =
        r#"phase == "done" && observations[0].tool == "docs.search""#.to_string();

    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_runtime_context_numeric_conditions() {
    let mut module = parse_air_file("../../tests/agents/expr-input.air.yaml").unwrap();
    let air_core::Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    workflow.rules[1].when = r#"phase == "call" && _air.model_calls >= 0"#.to_string();

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
fn accepts_tool_batch_dispatch_actions() {
    let module = parse_air_file("../../tests/agents/tool-batch-dispatch.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_zero_tool_batch_dispatch_bound() {
    let mut module = parse_air_file("../../tests/agents/tool-batch-dispatch.air.yaml").unwrap();
    let air_core::Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    for action in workflow.rules.iter_mut().flat_map(|rule| &mut rule.actions) {
        if let air_core::StateAction::ToolBatchDispatch { max_calls, .. } = action {
            *max_calls = 0;
        }
    }

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR097"),
        "expected AIR097, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_unknown_tool_batch_dispatch_allowlist_tool() {
    let mut module = parse_air_file("../../tests/agents/tool-batch-dispatch.air.yaml").unwrap();
    let air_core::Workflow::StateMachine(workflow) = &mut module.workflow else {
        panic!("expected state machine");
    };
    for action in workflow.rules.iter_mut().flat_map(|rule| &mut rule.actions) {
        if let air_core::StateAction::ToolBatchDispatch { allowed_tools, .. } = action {
            *allowed_tools = vec!["missing.tool".to_string()];
        }
    }

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR099"),
        "expected AIR099, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_repeated_tool_call_policy() {
    let module = parse_air_file("../../tests/agents/repeated-tool-call.air.yaml").unwrap();
    let report = verify(&module);

    assert!(
        report.is_success(),
        "expected success, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_zero_repeated_tool_call_policy() {
    let mut module = parse_air_file("../../tests/agents/repeated-tool-call.air.yaml").unwrap();
    module.policy.max_repeated_tool_calls = Some(0);

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR053"),
        "expected AIR053, got {:?}",
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
    let module = parse_air_yaml(INVALID_EXPR_REF);
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
fn rejects_zero_budget_take_last_within_bytes_expression() {
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
        air_core::Expr::TakeLastWithinBytes {
            take_last_within_bytes: Box::new(air_core::Expr::Ref {
                reference: "notes".to_string(),
            }),
            max_bytes: 0,
        },
    );

    let report = verify(&module);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIR097"),
        "expected AIR097, got {:?}",
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
    let module = parse_air_yaml(INVALID_APPROVAL);
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
    let module = parse_air_yaml(INVALID_CYCLE);
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
    let module = parse_air_yaml(INVALID_STATE_MACHINE_APPROVAL);
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
    let module = parse_air_yaml(INVALID_STATE_MACHINE_APPROVAL_BRANCH);
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
    let module = parse_air_yaml(INVALID_STATE_MACHINE_PHASE);
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

const INVALID_EXPR_REF: &str = r#"
agent:
  name: invalid-expr-ref-agent
  version: 0.1.0

inputs:
  text: string

outputs:
  response: object

state:
  phase:
    type: enum
    enum: [init, call, done]
  response: object

workflow:
  kind: state_machine
  initial: init
  max_steps: 5
  rules:
    - id: init
      when: phase == "init"
      actions:
        - kind: set
          values:
            phase: call
    - id: call
      when: phase == "call"
      actions:
        - kind: model_call
          model: expr_model
          input:
            object:
              missing:
                path: missing.value
          output: response
          timeout_seconds: 30
        - kind: set
          values:
            phase: done
    - id: done
      when: phase == "done"
      actions:
        - kind: return
          output: response
"#;

const INVALID_APPROVAL: &str = r#"
agent:
  name: invalid-approval
  version: 0.1.0

outputs:
  deployment_id: string

requires:
  capabilities:
    - production.deploy

tools:
  - name: deploy.production
    capability: production.deploy

workflow:
  kind: dag
  entry: deploy
  nodes:
    - id: deploy
      kind: tool_call
      tool: deploy.production
      timeout_seconds: 60
      retry:
        max_attempts: 1
    - id: done
      kind: return
      output: deployment_id
  edges:
    - from: deploy
      to: done

policy:
  require_approval:
    - production.deploy
"#;

const INVALID_CYCLE: &str = r#"
agent:
  name: invalid-cycle
  version: 0.1.0

outputs:
  done: string

requires:
  capabilities:
    - production.deploy

tools:
  - name: deploy.production
    capability: production.deploy

workflow:
  kind: dag
  entry: first
  nodes:
    - id: first
      kind: tool_call
      tool: deploy.production
      timeout_seconds: 30
      retry:
        max_attempts: 1
    - id: second
      kind: approval
      approval_for:
        - production.deploy
  edges:
    - from: first
      to: second
    - from: second
      to: first

policy:
  require_approval:
    - production.deploy
"#;

const INVALID_STATE_MACHINE_APPROVAL: &str = r#"
agent:
  name: invalid-state-machine-approval
  version: 0.1.0

outputs:
  deployment_id: string

state:
  phase:
    type: enum
    enum: [deploy, done, failed]
  deployment_id: string

requires:
  capabilities:
    - production.deploy

tools:
  - name: deploy.production
    capability: production.deploy

workflow:
  kind: state_machine
  initial: deploy
  max_steps: 5
  rules:
    - id: deploy
      when: phase == "deploy"
      actions:
        - kind: tool_call
          tool: deploy.production
          input: deployment_id
          output: deployment_id
          timeout_seconds: 60
          retry:
            max_attempts: 1
        - kind: set
          values:
            phase: done

policy:
  require_approval:
    - production.deploy
"#;

const INVALID_STATE_MACHINE_APPROVAL_BRANCH: &str = r#"
agent:
  name: invalid-state-machine-approval-branch
  version: 0.1.0

inputs:
  route: string

outputs:
  result: string

state:
  phase:
    type: enum
    enum: [start, approved_path, danger_path, done]
  route: string
  result: string

requires:
  capabilities:
    - production.deploy

tools:
  - name: deploy.production
    capability: production.deploy

workflow:
  kind: state_machine
  initial: start
  max_steps: 5
  rules:
    - id: choose-approved-path
      when: phase == "start" && route == "approved"
      actions:
        - kind: set
          values:
            phase: approved_path
    - id: choose-danger-path
      when: phase == "start" && route == "danger"
      actions:
        - kind: set
          values:
            phase: danger_path
    - id: approved-path
      when: phase == "approved_path"
      actions:
        - kind: approval
          approval_for: [production.deploy]
        - kind: set
          values:
            result: approved
            phase: done
    - id: danger-path
      when: phase == "danger_path"
      actions:
        - kind: tool_call
          tool: deploy.production
          input: route
          output: result
          timeout_seconds: 60
          retry:
            max_attempts: 1
        - kind: set
          values:
            phase: done
    - id: done
      when: phase == "done"
      actions:
        - kind: return
          output: result

policy:
  require_approval:
    - production.deploy
"#;

const INVALID_STATE_MACHINE_PHASE: &str = r#"
agent:
  name: invalid-state-machine-phase
  version: 0.1.0

outputs:
  done: string

state:
  phase:
    type: enum
    enum: [init, done]

workflow:
  kind: state_machine
  initial: boot
  max_steps: 5
  rules:
    - id: init
      when: phase == "init"
      actions:
        - kind: set
          values:
            phase: done
"#;
