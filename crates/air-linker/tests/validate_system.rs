use air_linker::{
    parse_module_store_file, parse_run_plan_file, parse_system_file, validate_run_plan,
    validate_system,
};

#[test]
fn accepts_valid_system() {
    let root = root();
    let system = parse_system_file(root.join("tests/systems/pr-review.air-system.yaml")).unwrap();
    let report = validate_system(&system, &root);

    assert!(
        report.is_success(),
        "expected valid system, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_unknown_connect_source_output() {
    assert_has_code(
        "tests/systems/invalid-connect-endpoint.air-system.yaml",
        "AIRL020",
    );
}

#[test]
fn rejects_connect_schema_mismatch() {
    assert_has_code(
        "tests/systems/invalid-connect-type.air-system.yaml",
        "AIRL023",
    );
}

#[test]
fn rejects_missing_required_input() {
    assert_has_code(
        "tests/systems/invalid-missing-required-input.air-system.yaml",
        "AIRL030",
    );
}

#[test]
fn rejects_unknown_system_output_endpoint() {
    assert_has_code(
        "tests/systems/invalid-output-endpoint.air-system.yaml",
        "AIRL040",
    );
}

#[test]
fn accepts_valid_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("tests/plans/module-store.air-store.yaml")).unwrap();
    let plan = parse_run_plan_file(root.join("tests/plans/schema-contract.air-plan.yaml")).unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_valid_composite_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("tests/plans/module-store.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/customer-triage-composite.air-plan.yaml"))
            .unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid composite run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_run_plan_connect_value_transforms() {
    let root = root();
    let store =
        parse_module_store_file(root.join("tests/plans/link-transform.air-store.yaml")).unwrap();
    let plan = parse_run_plan_file(root.join("tests/plans/link-transform.air-plan.yaml")).unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid link transform run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_complex_escalation_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("tests/plans/module-store.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/complex-escalation.air-plan.yaml")).unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid complex run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_deep_research_array_fan_in_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(root.join("examples/deep-research/deep-research.air-plan.yaml"))
        .unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid deep research run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_deep_research_array_topic_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan =
        parse_run_plan_file(root.join("examples/deep-research/deep-research-array.air-plan.yaml"))
            .unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid deep research array-topic run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_schedule_group_with_mismatched_predecessors() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let mut plan =
        parse_run_plan_file(root.join("examples/deep-research/deep-research-array.air-plan.yaml"))
            .unwrap();
    plan.schedule = Some(air_linker::SystemSchedule {
        max_parallel: Some(2),
        groups: vec![air_linker::ScheduleGroup {
            id: "invalid_wave".to_string(),
            nodes: vec!["plan".to_string(), "topic_1".to_string()],
            max_parallel: Some(2),
        }],
    });

    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIRL077"),
        "expected AIRL077, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_deep_research_clarified_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-clarified.air-plan.yaml"),
    )
    .unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid clarified deep research run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_deep_research_supervised_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-supervised.air-plan.yaml"),
    )
    .unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid supervised deep research run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_deep_research_two_step_supervised_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-supervised-two-step.air-plan.yaml"),
    )
    .unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid two-step supervised deep research run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_deep_research_dynamic_topic_fanout_run_plan() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid dynamic deep research run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_run_plan_missing_required_module_capability() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let mut plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();
    plan.requires
        .capabilities
        .retain(|capability| capability != "network.search");

    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIRP060"),
        "expected AIRP060, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_multiple_dynamic_fanout_boundaries() {
    let root = root();
    let store =
        parse_module_store_file(root.join("tests/plans/dynamic-smoke.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/dynamic-multiple-boundaries.air-plan.yaml"))
            .unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid multi-boundary dynamic run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_nested_dynamic_fanout_after_fanout() {
    let root = root();
    let store =
        parse_module_store_file(root.join("tests/plans/dynamic-smoke.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/dynamic-nested-fanout.air-plan.yaml")).unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report.is_success(),
        "expected valid nested dynamic fanout run plan, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_dynamic_fanout_source_that_is_not_an_array() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let mut plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();
    plan.dynamic.as_mut().unwrap().fanouts[0].source = "plan.plan.research_brief".to_string();

    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIRP048"),
        "expected AIRP048, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_dynamic_fanout_bound_smaller_than_source_schema() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();
    let mut plan = parse_run_plan_file(
        root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml"),
    )
    .unwrap();
    plan.dynamic.as_mut().unwrap().fanouts[0].max_items = 3;

    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIRP049"),
        "expected AIRP049, got {:?}",
        report.diagnostics
    );
}

#[test]
fn accepts_deep_research_store_recipes() {
    let root = root();
    let store =
        parse_module_store_file(root.join("examples/deep-research/module-store.air-store.yaml"))
            .unwrap();

    assert!(
        !store.recipes.is_empty(),
        "expected deep research module store to expose planning recipes"
    );
    for recipe in &store.recipes {
        let report = validate_run_plan(&recipe.plan, &store, &root);
        assert!(
            report.is_success(),
            "expected recipe {} to be valid, got {:?}",
            recipe.id,
            report.diagnostics
        );
    }
}

#[test]
fn rejects_append_fan_in_to_non_array_input() {
    let root = root();
    let mut system =
        parse_system_file(root.join("tests/systems/schema-contract.air-system.yaml")).unwrap();
    system.connect.push(air_linker::Connection {
        from: Some("extract.extracted".to_string()),
        to: "report.text[]".to_string(),
        value: None,
    });
    let report = validate_system(&system, &root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIRL025"),
        "expected AIRL025, got {:?}",
        report.diagnostics
    );
}

#[test]
fn rejects_run_plan_unknown_store_module() {
    let root = root();
    let store =
        parse_module_store_file(root.join("tests/plans/module-store.air-store.yaml")).unwrap();
    let plan =
        parse_run_plan_file(root.join("tests/plans/invalid-unknown-module.air-plan.yaml")).unwrap();
    let report = validate_run_plan(&plan, &store, &root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "AIRP012"),
        "expected AIRP012, got {:?}",
        report.diagnostics
    );
}

fn assert_has_code(path: &str, code: &'static str) {
    let root = root();
    let system = parse_system_file(root.join(path)).unwrap();
    let report = validate_system(&system, &root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code),
        "expected {code}, got {:?}",
        report.diagnostics
    );
}

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
