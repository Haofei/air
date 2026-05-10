use air_core::{NodeKind, StateAction, Workflow};
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanExplanation {
    plan: String,
    version: String,
    risk: String,
    risk_reason: String,
    modules: usize,
    dynamic_fanouts: Vec<DynamicFanoutExplanation>,
    capabilities: Vec<CapabilityExplanation>,
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
    required_approvals: Vec<String>,
    auto_executable: bool,
    provenance: ProvenanceExplanation,
    backend_compatibility: Vec<BackendCompatibilityExplanation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DynamicFanoutExplanation {
    id: String,
    module: String,
    max_items: usize,
    max_parallel: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CapabilityExplanation {
    name: String,
    approval: String,
    modules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProvenanceExplanation {
    plan_hash: String,
    store_hash: String,
    module_hashes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BackendCompatibilityExplanation {
    backend: String,
    status: String,
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CallBudget {
    model_calls: usize,
    tool_calls: usize,
}

#[derive(Debug, Default)]
struct CapabilityAccumulator {
    modules: BTreeSet<String>,
    requires_approval: bool,
}

pub(crate) fn build_plan_explanation(
    plan: &air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: &Path,
) -> Result<PlanExplanation> {
    let module_usage = collect_module_usage(plan);
    let modules = load_referenced_modules(plan, store, base_dir)?;
    let mut capability_map = plan
        .requires
        .capabilities
        .iter()
        .cloned()
        .map(|capability| (capability, CapabilityAccumulator::default()))
        .collect::<BTreeMap<_, _>>();
    let mut required_approvals = BTreeSet::new();
    let mut max_estimated_model_calls = 0usize;
    let mut max_estimated_tool_calls = 0usize;

    for (module_id, module) in &modules {
        let instances = module_usage.get(module_id).copied().unwrap_or_default();
        let budget = estimate_module_call_budget(module);
        max_estimated_model_calls =
            max_estimated_model_calls.saturating_add(budget.model_calls.saturating_mul(instances));
        max_estimated_tool_calls =
            max_estimated_tool_calls.saturating_add(budget.tool_calls.saturating_mul(instances));

        let module_approvals = module
            .policy
            .require_approval
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        required_approvals.extend(module_approvals.iter().cloned());

        for capability in &module.requires.capabilities {
            let entry = capability_map.entry(capability.clone()).or_default();
            entry.modules.insert(module_id.clone());
            if module_approvals.contains(capability) {
                entry.requires_approval = true;
            }
        }
    }

    let required_approvals = required_approvals.into_iter().collect::<Vec<_>>();
    let capabilities = capability_map
        .into_iter()
        .map(|(name, entry)| CapabilityExplanation {
            name,
            approval: if entry.requires_approval {
                "required".to_string()
            } else {
                "optional".to_string()
            },
            modules: entry.modules.into_iter().collect(),
        })
        .collect::<Vec<_>>();
    let (risk, risk_reason) = estimate_plan_risk(
        &required_approvals,
        max_estimated_model_calls,
        max_estimated_tool_calls,
        capabilities.len(),
    );

    let provenance = ProvenanceExplanation {
        plan_hash: canonical_sha256(plan)?,
        store_hash: canonical_sha256(store)?,
        module_hashes: modules
            .iter()
            .map(|(module_id, module)| Ok((module_id.clone(), canonical_sha256(module)?)))
            .collect::<Result<BTreeMap<_, _>>>()?,
    };

    Ok(PlanExplanation {
        plan: plan.plan.name.clone(),
        version: plan.plan.version.clone(),
        risk: risk.to_string(),
        risk_reason: risk_reason.to_string(),
        modules: plan.nodes.len(),
        dynamic_fanouts: plan
            .dynamic
            .as_ref()
            .map(|dynamic| {
                dynamic
                    .fanouts
                    .iter()
                    .map(|fanout| DynamicFanoutExplanation {
                        id: fanout.id.clone(),
                        module: fanout.module.clone(),
                        max_items: fanout.max_items,
                        max_parallel: fanout.max_parallel,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        capabilities,
        max_estimated_model_calls,
        max_estimated_tool_calls,
        required_approvals: required_approvals.clone(),
        auto_executable: required_approvals.is_empty(),
        provenance,
        backend_compatibility: collect_backend_compatibility(plan, store, base_dir),
    })
}

pub(crate) fn format_plan_explanation(explanation: &PlanExplanation) -> String {
    let mut output = String::new();
    writeln!(output, "Plan: {}@{}", explanation.plan, explanation.version).unwrap();
    writeln!(
        output,
        "Risk: {} ({})",
        explanation.risk, explanation.risk_reason
    )
    .unwrap();
    writeln!(output, "Modules: {}", explanation.modules).unwrap();
    writeln!(output, "Dynamic fanouts:").unwrap();
    if explanation.dynamic_fanouts.is_empty() {
        writeln!(output, "  none").unwrap();
    } else {
        for fanout in &explanation.dynamic_fanouts {
            writeln!(
                output,
                "  - {}: module={}, max_items={}, max_parallel={}",
                fanout.id,
                fanout.module,
                fanout.max_items,
                fanout
                    .max_parallel
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "auto".to_string())
            )
            .unwrap();
        }
    }

    writeln!(output, "Capabilities:").unwrap();
    if explanation.capabilities.is_empty() {
        writeln!(output, "  none declared").unwrap();
    } else {
        for capability in &explanation.capabilities {
            let modules = if capability.modules.is_empty() {
                "declared only".to_string()
            } else {
                capability.modules.join(", ")
            };
            writeln!(
                output,
                "  - {}: approval={}, modules={}",
                capability.name, capability.approval, modules
            )
            .unwrap();
        }
    }

    writeln!(
        output,
        "Model calls:\n  max estimated: {}",
        explanation.max_estimated_model_calls
    )
    .unwrap();
    writeln!(
        output,
        "Tool calls:\n  max estimated: {}",
        explanation.max_estimated_tool_calls
    )
    .unwrap();
    writeln!(output, "Approvals:").unwrap();
    if explanation.required_approvals.is_empty() {
        writeln!(output, "  none required").unwrap();
    } else {
        for capability in &explanation.required_approvals {
            writeln!(output, "  - {}", capability).unwrap();
        }
    }
    writeln!(
        output,
        "Auto executable: {}",
        if explanation.auto_executable {
            "yes"
        } else {
            "no"
        }
    )
    .unwrap();
    writeln!(output, "Provenance:").unwrap();
    writeln!(output, "  plan_hash: {}", explanation.provenance.plan_hash).unwrap();
    writeln!(
        output,
        "  store_hash: {}",
        explanation.provenance.store_hash
    )
    .unwrap();
    writeln!(output, "  module_hashes:").unwrap();
    for (module_id, hash) in &explanation.provenance.module_hashes {
        writeln!(output, "    - {}: {}", module_id, hash).unwrap();
    }

    writeln!(output, "Backend compatibility:").unwrap();
    for backend in &explanation.backend_compatibility {
        match &backend.reason {
            Some(reason) => writeln!(
                output,
                "  - {}: {} ({})",
                backend.backend, backend.status, reason
            )
            .unwrap(),
            None => writeln!(output, "  - {}: {}", backend.backend, backend.status).unwrap(),
        }
    }

    output
}

impl PlanExplanation {
    pub(crate) fn max_estimated_model_calls(&self) -> usize {
        self.max_estimated_model_calls
    }

    pub(crate) fn max_estimated_tool_calls(&self) -> usize {
        self.max_estimated_tool_calls
    }
}

fn estimate_plan_risk(
    required_approvals: &[String],
    max_estimated_model_calls: usize,
    max_estimated_tool_calls: usize,
    capability_count: usize,
) -> (&'static str, &'static str) {
    if !required_approvals.is_empty() {
        return ("high", "approval-required capabilities present");
    }
    if max_estimated_tool_calls > 0 || capability_count > 0 {
        return ("medium", "capability-scoped tool access present");
    }
    if max_estimated_model_calls > 0 {
        return ("low", "bounded model-only execution");
    }
    ("low", "no model or tool calls detected")
}

fn collect_backend_compatibility(
    plan: &air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: &Path,
) -> Vec<BackendCompatibilityExplanation> {
    let mut backends = vec![BackendCompatibilityExplanation {
        backend: "native".to_string(),
        status: "ok".to_string(),
        reason: None,
    }];

    backends.push(
        match air_backend_langgraph::lower_run_plan(plan, store, base_dir) {
            Ok(_) => BackendCompatibilityExplanation {
                backend: "langgraph".to_string(),
                status: "ok".to_string(),
                reason: None,
            },
            Err(error) => BackendCompatibilityExplanation {
                backend: "langgraph".to_string(),
                status: "unsupported".to_string(),
                reason: Some(error.to_string()),
            },
        },
    );
    backends.push(
        match air_backend_openai_agents_js::lower_run_plan_strict(plan, store, base_dir) {
            Ok(_) => BackendCompatibilityExplanation {
                backend: "openai-js-strict".to_string(),
                status: "ok".to_string(),
                reason: None,
            },
            Err(error) => BackendCompatibilityExplanation {
                backend: "openai-js-strict".to_string(),
                status: "unsupported".to_string(),
                reason: Some(error.to_string()),
            },
        },
    );

    backends
}

fn collect_module_usage(plan: &air_linker::RunPlan) -> BTreeMap<String, usize> {
    let mut usage = BTreeMap::new();
    for node in &plan.nodes {
        *usage.entry(node.module.clone()).or_default() += 1;
    }
    if let Some(dynamic) = &plan.dynamic {
        let node_ids = plan
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        let mut fanout_instances = BTreeMap::<String, usize>::new();
        let mut unresolved = dynamic.fanouts.iter().collect::<Vec<_>>();

        while !unresolved.is_empty() {
            let mut progressed = false;
            let mut next_unresolved = Vec::new();

            for fanout in unresolved {
                let parent_instances = if node_ids.contains(fanout.after.as_str()) {
                    Some(1)
                } else {
                    fanout_instances.get(&fanout.after).copied()
                };

                if let Some(parent_instances) = parent_instances {
                    let instances = parent_instances.saturating_mul(fanout.max_items);
                    fanout_instances.insert(fanout.id.clone(), instances);
                    *usage.entry(fanout.module.clone()).or_default() += instances;
                    progressed = true;
                } else {
                    next_unresolved.push(fanout);
                }
            }

            if !progressed {
                for fanout in next_unresolved {
                    fanout_instances.insert(fanout.id.clone(), fanout.max_items);
                    *usage.entry(fanout.module.clone()).or_default() += fanout.max_items;
                }
                break;
            }

            unresolved = next_unresolved;
        }
    }
    usage
}

fn load_referenced_modules(
    plan: &air_linker::RunPlan,
    store: &air_linker::ModuleStore,
    base_dir: &Path,
) -> Result<BTreeMap<String, air_core::AirModule>> {
    let mut module_ids = plan
        .nodes
        .iter()
        .map(|node| node.module.clone())
        .collect::<BTreeSet<_>>();
    if let Some(dynamic) = &plan.dynamic {
        module_ids.extend(dynamic.fanouts.iter().map(|fanout| fanout.module.clone()));
    }

    module_ids
        .into_iter()
        .map(|module_id| {
            let module_ref = store.modules.get(&module_id).ok_or_else(|| {
                anyhow::anyhow!("run plan references unknown store module {module_id}")
            })?;
            let path = air_linker::resolve_module_path(base_dir, &module_id, &module_ref.path)?;
            let module = air_parser::parse_air_file(path)?;
            Ok((module_id, module))
        })
        .collect()
}

fn estimate_module_call_budget(module: &air_core::AirModule) -> CallBudget {
    match &module.workflow {
        Workflow::Dag(workflow) => CallBudget {
            model_calls: apply_policy_cap(
                module.policy.max_model_calls,
                workflow
                    .nodes
                    .iter()
                    .filter(|node| matches!(node.kind, NodeKind::ModelCall))
                    .map(|node| retry_attempts(&node.retry))
                    .sum(),
            ),
            tool_calls: apply_policy_cap(
                module.policy.max_tool_calls,
                workflow
                    .nodes
                    .iter()
                    .filter(|node| matches!(node.kind, NodeKind::ToolCall))
                    .map(|node| retry_attempts(&node.retry))
                    .sum(),
            ),
        },
        Workflow::StateMachine(workflow) => {
            let model_calls = workflow
                .rules
                .iter()
                .flat_map(|rule| &rule.actions)
                .map(|action| match action {
                    StateAction::ModelCall { retry, .. } => retry_attempts(retry),
                    _ => 0,
                })
                .sum::<usize>()
                .saturating_mul(workflow.max_steps as usize);
            let tool_calls = workflow
                .rules
                .iter()
                .flat_map(|rule| &rule.actions)
                .map(|action| match action {
                    StateAction::ToolCall { retry, .. } => retry_attempts(retry),
                    StateAction::ToolDispatch { retry, .. } => retry_attempts(retry),
                    _ => 0,
                })
                .sum::<usize>()
                .saturating_mul(workflow.max_steps as usize);

            CallBudget {
                model_calls: apply_policy_cap(module.policy.max_model_calls, model_calls),
                tool_calls: apply_policy_cap(module.policy.max_tool_calls, tool_calls),
            }
        }
    }
}

fn retry_attempts(retry: &Option<air_core::RetryPolicy>) -> usize {
    retry
        .as_ref()
        .map(|retry| retry.max_attempts.max(1) as usize)
        .unwrap_or(1)
}

fn apply_policy_cap(policy_limit: Option<u32>, estimate: usize) -> usize {
    policy_limit
        .map(|limit| estimate.min(limit as usize))
        .unwrap_or(estimate)
}

fn canonical_sha256<T: Serialize>(value: &T) -> Result<String> {
    let encoded = serde_json::to_string(value)?;
    let digest = ring::digest::digest(&ring::digest::SHA256, encoded.as_bytes());
    let hex = digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("sha256:{hex}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::module_base_dir_for_store_path;
    use std::collections::BTreeMap;

    #[test]
    fn build_plan_explanation_reports_dynamic_governance_summary() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let plan_path = root.join("examples/deep-research/deep-research-dynamic.air-plan.yaml");
        let store_path = root.join("examples/deep-research/module-store.air-store.yaml");
        let plan = air_linker::parse_run_plan_file(&plan_path).unwrap();
        let store = air_linker::parse_module_store_file(&store_path).unwrap();
        let base_dir = module_base_dir_for_store_path(&store, &store_path);

        let explanation = build_plan_explanation(&plan, &store, &base_dir).unwrap();
        let rendered = format_plan_explanation(&explanation);

        assert_eq!(explanation.plan, "deep-research-dynamic-topic-fanout");
        assert_eq!(explanation.modules, 2);
        assert_eq!(explanation.risk, "medium");
        assert!(explanation.auto_executable);
        assert_eq!(explanation.dynamic_fanouts.len(), 1);
        assert_eq!(explanation.dynamic_fanouts[0].id, "topic_wave");
        assert_eq!(
            explanation.dynamic_fanouts[0].module,
            "deep_research.research_topic@0.1.0"
        );
        assert_eq!(explanation.dynamic_fanouts[0].max_items, 4);
        assert_eq!(explanation.dynamic_fanouts[0].max_parallel, Some(4));
        assert!(explanation.max_estimated_model_calls > 0);
        assert!(explanation.max_estimated_tool_calls > 0);
        assert!(explanation.provenance.plan_hash.starts_with("sha256:"));
        assert!(explanation.provenance.store_hash.starts_with("sha256:"));
        assert!(explanation
            .provenance
            .module_hashes
            .contains_key("deep_research.plan_array@0.1.0"));
        assert!(explanation
            .backend_compatibility
            .iter()
            .any(|backend| backend.backend == "native" && backend.status == "ok"));
        assert!(explanation
            .backend_compatibility
            .iter()
            .any(|backend| backend.backend == "langgraph"));
        assert!(explanation
            .backend_compatibility
            .iter()
            .any(|backend| backend.backend == "openai-js-strict"));
        assert!(rendered.contains("Dynamic fanouts:"));
        assert!(rendered.contains(
            "topic_wave: module=deep_research.research_topic@0.1.0, max_items=4, max_parallel=4"
        ));
        assert!(rendered.contains("Backend compatibility:"));
    }

    #[test]
    fn collect_module_usage_counts_nested_dynamic_fanouts_per_parent() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let plan = air_linker::parse_run_plan_file(
            root.join("tests/plans/dynamic-nested-fanout.air-plan.yaml"),
        )
        .unwrap();

        let usage = collect_module_usage(&plan);

        assert_eq!(usage["test.dynamic_plan@0.1.0"], 1);
        assert_eq!(usage["test.final_notes@0.1.0"], 1);
        assert_eq!(usage["test.topic_branch@0.1.0"], 2);
        assert_eq!(usage["test.topic_note@0.1.0"], 2);
    }

    #[test]
    fn estimate_module_call_budget_counts_state_machine_retries() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let module =
            air_parser::parse_air_file(root.join("tests/agents/model-retry.air.yaml")).unwrap();

        let budget = estimate_module_call_budget(&module);

        assert_eq!(budget.model_calls, 10);
        assert_eq!(budget.tool_calls, 0);
    }

    #[test]
    fn estimate_module_call_budget_counts_dag_retries() {
        let module = serde_yaml::from_str::<air_core::AirModule>(
            r#"
agent:
  name: dag-retry-agent
  version: 0.1.0
inputs:
  text: string
outputs:
  result: object
workflow:
  kind: dag
  entry: first
  nodes:
    - id: first
      kind: model_call
      model: retry_model
      output: result
      timeout_seconds: 30
      retry:
        max_attempts: 3
    - id: second
      kind: tool_call
      tool: retry_tool
      output: tool_result
      timeout_seconds: 30
      retry:
        max_attempts: 2
    - id: done
      kind: return
  edges:
    - from: first
      to: second
    - from: second
      to: done
"#,
        )
        .unwrap();

        let budget = estimate_module_call_budget(&module);

        assert_eq!(budget.model_calls, 3);
        assert_eq!(budget.tool_calls, 2);
    }

    #[test]
    fn format_plan_explanation_renders_unspecified_dynamic_parallelism_as_auto() {
        let explanation = PlanExplanation {
            plan: "dynamic-auto".to_string(),
            version: "0.1.0".to_string(),
            risk: "low".to_string(),
            risk_reason: "bounded model-only execution".to_string(),
            modules: 1,
            dynamic_fanouts: vec![DynamicFanoutExplanation {
                id: "topic_wave".to_string(),
                module: "test.topic@0.1.0".to_string(),
                max_items: 3,
                max_parallel: None,
            }],
            capabilities: Vec::new(),
            max_estimated_model_calls: 0,
            max_estimated_tool_calls: 0,
            required_approvals: Vec::new(),
            auto_executable: true,
            provenance: ProvenanceExplanation {
                plan_hash: "sha256:plan".to_string(),
                store_hash: "sha256:store".to_string(),
                module_hashes: BTreeMap::new(),
            },
            backend_compatibility: Vec::new(),
        };

        let rendered = format_plan_explanation(&explanation);

        assert!(rendered
            .contains("topic_wave: module=test.topic@0.1.0, max_items=3, max_parallel=auto"));
    }

    #[test]
    fn build_plan_explanation_marks_approval_gated_plan_as_high_risk() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let plan_path = root.join("tests/plans/approval-smoke.air-plan.yaml");
        let store_path = root.join("tests/plans/approval-smoke.air-store.yaml");
        let plan = air_linker::parse_run_plan_file(&plan_path).unwrap();
        let store = air_linker::parse_module_store_file(&store_path).unwrap();
        let base_dir = module_base_dir_for_store_path(&store, &store_path);

        let explanation = build_plan_explanation(&plan, &store, &base_dir).unwrap();
        let rendered = format_plan_explanation(&explanation);

        assert_eq!(explanation.risk, "high");
        assert_eq!(
            explanation.required_approvals,
            vec!["production.deploy".to_string()]
        );
        assert!(!explanation.auto_executable);
        assert!(explanation.capabilities.iter().any(|capability| {
            capability.name == "production.deploy" && capability.approval == "required"
        }));
        assert!(rendered.contains("Approvals:"));
        assert!(rendered.contains("  - production.deploy"));
    }
}
