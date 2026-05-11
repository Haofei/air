use air_core::{
    normalize_path, path_segments, AirModule, DagWorkflow, DetailedTypeKind, Diagnostic, NodeKind,
    Severity, StateAction, StateMachineWorkflow, StateRule, TypeSpec, Workflow, WorkflowNode,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    pub diagnostics: Vec<Diagnostic>,
}

impl VerificationReport {
    pub fn is_success(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

pub fn verify(module: &AirModule) -> VerificationReport {
    let mut verifier = Verifier::default();
    verifier.verify(module);
    VerificationReport {
        diagnostics: verifier.diagnostics,
    }
}

#[derive(Default)]
struct Verifier {
    diagnostics: Vec<Diagnostic>,
}

impl Verifier {
    fn verify(&mut self, module: &AirModule) {
        self.verify_metadata(module);
        self.verify_schema("inputs", &module.inputs);
        self.verify_schema("outputs", &module.outputs);
        self.verify_schema("state", &module.state);
        self.verify_tools(module);
        self.verify_workflow(module);
        self.verify_policy(module);
    }

    fn verify_metadata(&mut self, module: &AirModule) {
        if module.agent.name.trim().is_empty() {
            self.error("AIR001", "agent.name must not be empty");
        }

        if module.agent.version.trim().is_empty() {
            self.error("AIR002", "agent.version must not be empty");
        }
    }

    fn verify_schema(&mut self, label: &str, schema: &air_core::SchemaMap) {
        for (field, spec) in schema {
            if field == "_air" {
                self.error(
                    "AIR095",
                    format!("{label}.{field} uses reserved AIR runtime context field _air"),
                );
            }
            self.verify_type_spec(&format!("{label}.{field}"), spec);
        }
    }

    fn verify_type_spec(&mut self, path: &str, spec: &air_core::TypeSpec) {
        let air_core::TypeSpec::Detailed(detailed) = spec else {
            return;
        };

        match detailed.kind {
            air_core::DetailedTypeKind::Array if detailed.items.is_none() => {
                self.error(
                    "AIR010",
                    format!("{path} is an array type but does not define items"),
                );
            }
            air_core::DetailedTypeKind::Object => {
                for required in &detailed.required {
                    if !detailed.properties.contains_key(required) {
                        self.error(
                            "AIR011",
                            format!(
                                "{path} requires property \"{required}\" but it is not declared"
                            ),
                        );
                    }
                }

                for (field, nested) in &detailed.properties {
                    self.verify_type_spec(&format!("{path}.{field}"), nested);
                }
            }
            air_core::DetailedTypeKind::Enum if detailed.enum_values.is_empty() => {
                self.error(
                    "AIR012",
                    format!("{path} is an enum type but does not define enum values"),
                );
            }
            _ => {}
        }
    }

    fn verify_tools(&mut self, module: &AirModule) {
        let mut seen = BTreeSet::new();
        let required_capabilities: BTreeSet<_> = module.requires.capabilities.iter().collect();
        let denied: BTreeSet<_> = module.denied.iter().collect();

        for tool in &module.tools {
            if tool.name.trim().is_empty() {
                self.error("AIR020", "tool.name must not be empty");
                continue;
            }

            if !seen.insert(&tool.name) {
                self.error(
                    "AIR021",
                    format!("duplicate tool declaration: {}", tool.name),
                );
            }

            if let Some(capability) = &tool.capability {
                if denied.contains(capability) {
                    self.error(
                        "AIR022",
                        format!(
                            "tool {} requires denied capability {}",
                            tool.name, capability
                        ),
                    );
                }

                if !required_capabilities.contains(capability) {
                    self.error(
                        "AIR023",
                        format!(
                            "tool {} requires capability {}, but it is missing from requires.capabilities",
                            tool.name, capability
                        ),
                    );
                }
            }
        }
    }

    fn verify_workflow(&mut self, module: &AirModule) {
        match &module.workflow {
            Workflow::Dag(workflow) => self.verify_dag_workflow(module, workflow),
            Workflow::StateMachine(workflow) => {
                self.verify_state_machine_workflow(module, workflow)
            }
        }
    }

    fn verify_dag_workflow(&mut self, module: &AirModule, workflow: &DagWorkflow) {
        let mut nodes: BTreeMap<&str, &WorkflowNode> = BTreeMap::new();
        let mut duplicate_nodes = BTreeSet::new();

        for node in &workflow.nodes {
            if node.id.trim().is_empty() {
                self.error("AIR030", "workflow node id must not be empty");
                continue;
            }

            if nodes.insert(node.id.as_str(), node).is_some() {
                duplicate_nodes.insert(node.id.clone());
            }

            match node.kind {
                NodeKind::ToolCall => {
                    if node.tool.as_deref().unwrap_or_default().trim().is_empty() {
                        self.error(
                            "AIR031",
                            format!("tool_call node {} must declare tool", node.id),
                        );
                    }
                }
                NodeKind::ModelCall => {
                    if node.model.as_deref().unwrap_or_default().trim().is_empty() {
                        self.error(
                            "AIR032",
                            format!("model_call node {} must declare model", node.id),
                        );
                    }
                }
                NodeKind::Return => {
                    if let Some(output) = &node.output {
                        if !module.outputs.contains_key(output) {
                            self.error(
                                "AIR033",
                                format!(
                                    "return node {} references unknown output {}",
                                    node.id, output
                                ),
                            );
                        }
                    }
                }
                NodeKind::Approval => {
                    if node.approval_for.is_empty() {
                        self.error(
                            "AIR044",
                            format!("approval node {} must declare approval_for", node.id),
                        );
                    }
                    for capability in &node.approval_for {
                        if !module.policy.require_approval.contains(capability) {
                            self.error(
                                "AIR045",
                                format!(
                                    "approval node {} references capability {capability}, but policy.require_approval does not require it",
                                    node.id
                                ),
                            );
                        }
                    }
                }
            }

            if matches!(node.kind, NodeKind::ToolCall | NodeKind::ModelCall)
                && node.timeout_seconds.is_none()
            {
                self.error(
                    "AIR034",
                    format!("node {} must define timeout_seconds", node.id),
                );
            }

            if let Some(retry) = &node.retry {
                if retry.max_attempts == 0 {
                    self.error(
                        "AIR035",
                        format!("node {} retry.max_attempts must be at least 1", node.id),
                    );
                }
                if retry.max_attempts > 10 {
                    self.error(
                        "AIR036",
                        format!(
                            "node {} retry.max_attempts={} exceeds v0.1 maximum of 10",
                            node.id, retry.max_attempts
                        ),
                    );
                }
            }
        }

        for duplicate in duplicate_nodes {
            self.error("AIR037", format!("duplicate workflow node id: {duplicate}"));
        }

        if !nodes.contains_key(workflow.entry.as_str()) {
            self.error(
                "AIR038",
                format!("workflow.entry references unknown node {}", workflow.entry),
            );
        }

        let tools: BTreeSet<_> = module.tools.iter().map(|tool| &tool.name).collect();
        for node in &workflow.nodes {
            if let Some(tool) = &node.tool {
                if !tools.contains(tool) {
                    self.error(
                        "AIR039",
                        format!("node {} references unknown tool {}", node.id, tool),
                    );
                }
            }
        }

        let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &workflow.edges {
            if !nodes.contains_key(edge.from.as_str()) {
                self.error(
                    "AIR040",
                    format!("edge references unknown from node {}", edge.from),
                );
            }
            if !nodes.contains_key(edge.to.as_str()) {
                self.error(
                    "AIR041",
                    format!("edge references unknown to node {}", edge.to),
                );
            }
            adjacency
                .entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }

        if self.verify_acyclic(&adjacency) {
            self.verify_dag_approval_paths(module, workflow, &nodes, &adjacency);
        }
    }

    fn verify_acyclic(&mut self, adjacency: &BTreeMap<&str, Vec<&str>>) -> bool {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Mark {
            Visiting,
            Done,
        }

        fn visit<'a>(
            node: &'a str,
            adjacency: &BTreeMap<&'a str, Vec<&'a str>>,
            marks: &mut BTreeMap<&'a str, Mark>,
        ) -> bool {
            match marks.get(node) {
                Some(Mark::Visiting) => return false,
                Some(Mark::Done) => return true,
                None => {}
            }

            marks.insert(node, Mark::Visiting);
            for next in adjacency.get(node).into_iter().flatten() {
                if !visit(next, adjacency, marks) {
                    return false;
                }
            }
            marks.insert(node, Mark::Done);
            true
        }

        let mut marks = BTreeMap::new();
        for node in adjacency.keys() {
            if !visit(node, adjacency, &mut marks) {
                self.error("AIR042", "workflow graph must be acyclic in AIR v0.1");
                return false;
            }
        }

        true
    }

    fn verify_dag_approval_paths(
        &mut self,
        module: &AirModule,
        workflow: &DagWorkflow,
        nodes: &BTreeMap<&str, &WorkflowNode>,
        adjacency: &BTreeMap<&str, Vec<&str>>,
    ) {
        let required: BTreeSet<_> = module.policy.require_approval.iter().cloned().collect();
        if required.is_empty() || !nodes.contains_key(workflow.entry.as_str()) {
            return;
        }

        let tools_by_name: BTreeMap<_, _> =
            module.tools.iter().map(|tool| (&tool.name, tool)).collect();
        let mut stack = vec![(workflow.entry.as_str(), BTreeSet::<String>::new())];

        while let Some((node_id, approved)) = stack.pop() {
            let Some(node) = nodes.get(node_id) else {
                continue;
            };

            let mut approved = approved;
            if node.kind == NodeKind::Approval {
                approved.extend(node.approval_for.iter().cloned());
            }

            if node.kind == NodeKind::ToolCall {
                if let Some(tool_name) = &node.tool {
                    if let Some(tool) = tools_by_name.get(tool_name) {
                        if let Some(capability) = &tool.capability {
                            if required.contains(capability) && !approved.contains(capability) {
                                self.error(
                                    "AIR043",
                                    format!(
                                        "node {} calls capability {}, but no approval gate exists on that path",
                                        node.id, capability
                                    ),
                                );
                            }
                        }
                    }
                }
            }

            for next in adjacency.get(node_id).into_iter().flatten() {
                stack.push((next, approved.clone()));
            }
        }
    }

    fn verify_state_machine_workflow(
        &mut self,
        module: &AirModule,
        workflow: &StateMachineWorkflow,
    ) {
        if workflow.initial.trim().is_empty() {
            self.error("AIR060", "state_machine.initial must not be empty");
        }

        if workflow.max_steps == 0 {
            self.error("AIR061", "state_machine.max_steps must be at least 1");
        }

        if workflow.terminal.is_empty() {
            self.error(
                "AIR062",
                "state_machine.terminal must declare at least one terminal state",
            );
        }

        if !module.state.contains_key("phase") {
            self.error(
                "AIR063",
                "state_machine modules must declare state.phase in AIR v0.1",
            );
        } else if let Some(phase_values) = enum_values(module.state.get("phase")) {
            if !phase_values.contains(&workflow.initial) {
                self.error(
                    "AIR069",
                    format!(
                        "state_machine.initial {} is not present in state.phase enum",
                        workflow.initial
                    ),
                );
            }

            for terminal in &workflow.terminal {
                if !phase_values.contains(terminal) {
                    self.error(
                        "AIR069",
                        format!(
                            "state_machine terminal {} is not present in state.phase enum",
                            terminal
                        ),
                    );
                }
            }
        }

        let mut seen_rules = BTreeSet::new();
        let tools_by_name: BTreeMap<_, _> = module
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool))
            .collect();

        for rule in &workflow.rules {
            if rule.id.trim().is_empty() {
                self.error("AIR064", "state_machine rule id must not be empty");
                continue;
            }

            if !seen_rules.insert(rule.id.as_str()) {
                self.error(
                    "AIR065",
                    format!("duplicate state_machine rule id: {}", rule.id),
                );
            }

            if rule.when.trim().is_empty() {
                self.error(
                    "AIR066",
                    format!("state_machine rule {} must declare when", rule.id),
                );
            } else {
                self.verify_condition(&rule.id, &rule.when, module);
            }

            if rule.actions.is_empty() {
                self.error(
                    "AIR067",
                    format!(
                        "state_machine rule {} must declare at least one action",
                        rule.id
                    ),
                );
            }

            for action in &rule.actions {
                self.verify_state_action(module, &tools_by_name, &rule.id, action);
            }
        }

        if workflow.rules.is_empty() {
            self.error("AIR068", "state_machine.rules must not be empty");
        }

        self.verify_state_machine_approval_paths(module, workflow, &tools_by_name);
    }

    fn verify_state_action(
        &mut self,
        module: &AirModule,
        tools_by_name: &BTreeMap<&str, &air_core::ToolSpec>,
        rule_id: &str,
        action: &StateAction,
    ) {
        match action {
            StateAction::Set { values } => {
                if values.is_empty() {
                    self.error(
                        "AIR070",
                        format!("set action in rule {rule_id} must declare values"),
                    );
                }
            }
            StateAction::Append { target, value } => {
                self.verify_control_field_write(rule_id, "append target", target);
                if !module.state.contains_key(target) {
                    self.error(
                        "AIR088",
                        format!("append action in rule {rule_id} references unknown state field {target}"),
                    );
                } else if !is_array_type(module.state.get(target)) {
                    self.error(
                        "AIR089",
                        format!("append action in rule {rule_id} target {target} must be an array state field"),
                    );
                }
                self.verify_input_spec(rule_id, "append value", value, module);
            }
            StateAction::ModelCall {
                model,
                input,
                output,
                timeout_seconds,
                retry,
            } => {
                if model.trim().is_empty() {
                    self.error(
                        "AIR071",
                        format!("model_call action in rule {rule_id} must declare model"),
                    );
                }
                self.verify_input_spec(rule_id, "model_call input", input, module);
                self.verify_control_field_write(rule_id, "model_call output", output);
                self.verify_state_ref(rule_id, "model_call output", output, module);
                self.verify_timeout(rule_id, "model_call", *timeout_seconds);
                self.verify_retry(rule_id, retry);
            }
            StateAction::ToolCall {
                tool,
                input,
                output,
                timeout_seconds,
                retry,
            } => {
                let Some(_tool_spec) = tools_by_name.get(tool.as_str()) else {
                    self.error(
                        "AIR072",
                        format!(
                            "tool_call action in rule {rule_id} references unknown tool {tool}"
                        ),
                    );
                    self.verify_input_spec(rule_id, "tool_call input", input, module);
                    self.verify_control_field_write(rule_id, "tool_call output", output);
                    self.verify_state_ref(rule_id, "tool_call output", output, module);
                    self.verify_timeout(rule_id, "tool_call", *timeout_seconds);
                    self.verify_retry(rule_id, retry);
                    return;
                };

                self.verify_input_spec(rule_id, "tool_call input", input, module);
                self.verify_control_field_write(rule_id, "tool_call output", output);
                self.verify_state_ref(rule_id, "tool_call output", output, module);
                self.verify_timeout(rule_id, "tool_call", *timeout_seconds);
                self.verify_retry(rule_id, retry);
            }
            StateAction::ToolDispatch {
                input,
                output,
                timeout_seconds,
                retry,
            } => {
                if tools_by_name.is_empty() {
                    self.error(
                        "AIR096",
                        format!("tool_dispatch action in rule {rule_id} requires at least one declared tool"),
                    );
                }
                self.verify_input_spec(rule_id, "tool_dispatch input", input, module);
                self.verify_control_field_write(rule_id, "tool_dispatch output", output);
                self.verify_state_ref(rule_id, "tool_dispatch output", output, module);
                self.verify_timeout(rule_id, "tool_dispatch", *timeout_seconds);
                self.verify_retry(rule_id, retry);
            }
            StateAction::ToolBatchDispatch {
                input,
                output,
                timeout_seconds,
                max_calls,
                write_scope,
                retry,
                ..
            } => {
                if tools_by_name.is_empty() {
                    self.error(
                        "AIR096",
                        format!(
                            "tool_batch_dispatch action in rule {rule_id} requires at least one declared tool"
                        ),
                    );
                }
                if *max_calls == 0 {
                    self.error(
                        "AIR097",
                        format!(
                            "tool_batch_dispatch action in rule {rule_id} max_calls must be at least 1"
                        ),
                    );
                }
                self.verify_input_spec(rule_id, "tool_batch_dispatch input", input, module);
                if let Some(write_scope) = write_scope {
                    self.verify_input_spec(
                        rule_id,
                        "tool_batch_dispatch write_scope",
                        write_scope,
                        module,
                    );
                }
                self.verify_control_field_write(rule_id, "tool_batch_dispatch output", output);
                self.verify_state_ref(rule_id, "tool_batch_dispatch output", output, module);
                self.verify_timeout(rule_id, "tool_batch_dispatch", *timeout_seconds);
                self.verify_retry(rule_id, retry);
            }
            StateAction::Approval { approval_for } => {
                if approval_for.is_empty() {
                    self.error(
                        "AIR074",
                        format!("approval action in rule {rule_id} must declare approval_for"),
                    );
                }
                for capability in approval_for {
                    if !module.policy.require_approval.contains(capability) {
                        self.error(
                            "AIR093",
                            format!(
                                "approval action in rule {rule_id} references capability {capability}, but policy.require_approval does not require it"
                            ),
                        );
                    }
                }
            }
            StateAction::Return { output } => {
                if !module.outputs.contains_key(output) {
                    self.error(
                        "AIR075",
                        format!(
                            "return action in rule {rule_id} references unknown output {output}"
                        ),
                    );
                }
            }
        }
    }

    fn verify_control_field_write(&mut self, rule_id: &str, label: &str, field: &str) {
        if normalize_path(field) == "phase" {
            self.error(
                "AIR094",
                format!(
                    "{label} in rule {rule_id} cannot write state.phase; use an explicit set action for state_machine transitions"
                ),
            );
        }
    }

    fn verify_state_machine_approval_paths(
        &mut self,
        module: &AirModule,
        workflow: &StateMachineWorkflow,
        tools_by_name: &BTreeMap<&str, &air_core::ToolSpec>,
    ) {
        let required: BTreeSet<_> = module.policy.require_approval.iter().cloned().collect();
        if required.is_empty() || workflow.rules.is_empty() {
            return;
        }

        let phases = enum_values(module.state.get("phase")).unwrap_or_else(|| {
            let mut phases = BTreeSet::new();
            phases.insert(workflow.initial.clone());
            phases.extend(workflow.terminal.iter().cloned());
            phases
        });
        let terminal: BTreeSet<_> = workflow.terminal.iter().cloned().collect();
        let mut visited = BTreeSet::new();
        let mut reported = BTreeSet::new();
        let mut stack = vec![StateMachineApprovalState {
            phase: workflow.initial.clone(),
            approved: BTreeSet::new(),
        }];

        while let Some(state) = stack.pop() {
            if !visited.insert(state.key()) {
                continue;
            }

            for rule in workflow
                .rules
                .iter()
                .filter(|rule| condition_may_match_phase(&rule.when, &state.phase))
            {
                let mut approved = state.approved.clone();
                for action in &rule.actions {
                    match action {
                        StateAction::Approval { approval_for } => {
                            approved.extend(
                                approval_for
                                    .iter()
                                    .filter(|capability| required.contains(*capability))
                                    .cloned(),
                            );
                        }
                        StateAction::ToolCall { tool, .. } => {
                            if let Some(capability) = tools_by_name
                                .get(tool.as_str())
                                .and_then(|tool_spec| tool_spec.capability.as_ref())
                            {
                                if required.contains(capability)
                                    && !approved.contains(capability)
                                    && reported.insert((rule.id.clone(), capability.clone()))
                                {
                                    self.error(
                                        "AIR073",
                                        format!(
                                            "tool_call action in rule {} calls capability {}, but not every reachable state_machine path includes approval first",
                                            rule.id, capability
                                        ),
                                    );
                                }
                            }
                        }
                        StateAction::ToolDispatch { .. }
                        | StateAction::ToolBatchDispatch { .. } => {
                            for capability in tools_by_name
                                .values()
                                .filter_map(|tool_spec| tool_spec.capability.as_ref())
                            {
                                if required.contains(capability)
                                    && !approved.contains(capability)
                                    && reported.insert((rule.id.clone(), capability.clone()))
                                {
                                    self.error(
                                        "AIR073",
                                        format!(
                                            "dynamic tool action in rule {} can call capability {}, but not every reachable state_machine path includes approval first",
                                            rule.id, capability
                                        ),
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if terminal.contains(&state.phase) {
                    continue;
                }

                for next_phase in next_phases_after_rule(rule, &state.phase, &phases) {
                    stack.push(StateMachineApprovalState {
                        phase: next_phase,
                        approved: approved.clone(),
                    });
                }
            }
        }
    }

    fn verify_state_ref(&mut self, rule_id: &str, label: &str, field: &str, module: &AirModule) {
        if field.trim().is_empty() {
            self.error(
                "AIR076",
                format!("{label} in rule {rule_id} must not be empty"),
            );
            return;
        }

        if field == "_air" {
            return;
        }

        if !(module.inputs.contains_key(field)
            || module.outputs.contains_key(field)
            || module.state.contains_key(field))
        {
            self.error(
                "AIR077",
                format!("{label} in rule {rule_id} references unknown field {field}"),
            );
        }
    }

    fn verify_condition(&mut self, rule_id: &str, condition: &str, module: &AirModule) {
        for group in condition.split("||") {
            for clause in group.split("&&") {
                let clause = clause.trim();
                let Some((left, right)) =
                    clause.split_once("==").or_else(|| clause.split_once("!="))
                else {
                    self.error(
                        "AIR086",
                        format!(
                            "state_machine rule {rule_id} has unsupported condition clause {clause}"
                        ),
                    );
                    continue;
                };
                if right.trim().is_empty() {
                    self.error(
                        "AIR087",
                        format!("state_machine rule {rule_id} has empty condition literal"),
                    );
                }
                self.verify_path_ref(rule_id, "condition", left.trim(), module);
            }
        }
    }

    fn verify_input_spec(
        &mut self,
        rule_id: &str,
        label: &str,
        input: &air_core::InputSpec,
        module: &AirModule,
    ) {
        match input {
            air_core::InputSpec::Field(field) => {
                self.verify_state_ref(rule_id, label, field, module);
            }
            air_core::InputSpec::Fields { fields } => {
                if fields.is_empty() {
                    self.error(
                        "AIR081",
                        format!("{label} in rule {rule_id} must declare at least one field"),
                    );
                }

                for (alias, field) in fields {
                    if alias.trim().is_empty() {
                        self.error(
                            "AIR082",
                            format!("{label} in rule {rule_id} has an empty field alias"),
                        );
                    }
                    self.verify_state_ref(rule_id, label, field, module);
                }
            }
            air_core::InputSpec::Expr(expr) => {
                self.verify_expr(rule_id, label, expr, module);
            }
        }
    }

    fn verify_expr(
        &mut self,
        rule_id: &str,
        label: &str,
        expr: &air_core::Expr,
        module: &AirModule,
    ) {
        match expr {
            air_core::Expr::Ref { reference } => {
                self.verify_path_ref(rule_id, label, reference, module);
            }
            air_core::Expr::Path { path } => {
                self.verify_path_ref(rule_id, label, path, module);
            }
            air_core::Expr::Literal { .. } => {}
            air_core::Expr::Object { object } => {
                if object.is_empty() {
                    self.error(
                        "AIR083",
                        format!("{label} in rule {rule_id} object expression must not be empty"),
                    );
                }
                for (key, nested) in object {
                    if key.trim().is_empty() {
                        self.error(
                            "AIR084",
                            format!("{label} in rule {rule_id} has an empty object key"),
                        );
                    }
                    self.verify_expr(rule_id, label, nested, module);
                }
            }
            air_core::Expr::Array { array } => {
                for nested in array {
                    self.verify_expr(rule_id, label, nested, module);
                }
            }
            air_core::Expr::Template { template } => {
                for path in template_paths(template) {
                    self.verify_path_ref(rule_id, label, &path, module);
                }
            }
            air_core::Expr::Truncate {
                truncate,
                max_chars,
            } => {
                if *max_chars == 0 {
                    self.error(
                        "AIR090",
                        format!("{label} in rule {rule_id} truncate.max_chars must be at least 1"),
                    );
                }
                self.verify_expr(rule_id, label, truncate, module);
            }
        }
    }

    fn verify_path_ref(&mut self, rule_id: &str, label: &str, path: &str, module: &AirModule) {
        let segments = path_segments(path);
        let Some(root) = segments
            .first()
            .filter(|segment| !segment.trim().is_empty())
        else {
            self.error(
                "AIR085",
                format!("{label} in rule {rule_id} has an empty expression path"),
            );
            return;
        };
        if root == "_air" {
            self.verify_runtime_context_path(rule_id, label, &segments);
            return;
        }
        self.verify_state_ref(rule_id, label, root, module);
        let Some(mut spec) = module_field_type(module, root) else {
            return;
        };
        let mut checked_path = root.to_string();
        for segment in segments.iter().skip(1) {
            if segment.trim().is_empty() {
                self.error(
                    "AIR085",
                    format!("{label} in rule {rule_id} has an empty expression path"),
                );
                return;
            }
            checked_path.push('.');
            checked_path.push_str(segment);
            let Some(next) = nested_property_type(spec, segment) else {
                if type_has_known_properties(spec) {
                    self.error(
                        "AIR094",
                        format!(
                            "{label} in rule {rule_id} references unknown nested path {checked_path}"
                        ),
                    );
                }
                return;
            };
            spec = next;
        }
    }

    fn verify_runtime_context_path(&mut self, rule_id: &str, label: &str, segments: &[String]) {
        if segments.len() == 1 {
            return;
        }
        if segments.len() > 2 {
            self.error(
                "AIR094",
                format!(
                    "{label} in rule {rule_id} references unknown nested path {}",
                    segments.join(".")
                ),
            );
            return;
        }
        let Some(field) = segments.get(1) else {
            return;
        };
        if !is_known_runtime_context_field(field) {
            self.error(
                "AIR094",
                format!(
                    "{label} in rule {rule_id} references unknown nested path {}",
                    segments.join(".")
                ),
            );
        }
    }

    fn verify_timeout(&mut self, rule_id: &str, action: &str, timeout_seconds: u64) {
        if timeout_seconds == 0 {
            self.error(
                "AIR078",
                format!("{action} action in rule {rule_id} timeout_seconds must be at least 1"),
            );
        }
    }

    fn verify_retry(&mut self, rule_id: &str, retry: &Option<air_core::RetryPolicy>) {
        if let Some(retry) = retry {
            if retry.max_attempts == 0 {
                self.error(
                    "AIR079",
                    format!("action in rule {rule_id} retry.max_attempts must be at least 1"),
                );
            }

            if retry.max_attempts > 10 {
                self.error(
                    "AIR080",
                    format!(
                        "action in rule {rule_id} retry.max_attempts={} exceeds v0.1 maximum of 10",
                        retry.max_attempts
                    ),
                );
            }

            if let Some(token_limit) = retry
                .on_provider_error
                .as_ref()
                .and_then(|policy| policy.token_limit.as_ref())
            {
                if token_limit.max_input_chars.is_empty() {
                    self.error(
                        "AIR091",
                        format!(
                            "action in rule {rule_id} token_limit.max_input_chars must not be empty"
                        ),
                    );
                }
                for max_chars in &token_limit.max_input_chars {
                    if *max_chars == 0 {
                        self.error(
                            "AIR092",
                            format!(
                                "action in rule {rule_id} token_limit.max_input_chars values must be at least 1"
                            ),
                        );
                    }
                }
            }
        }
    }

    fn verify_policy(&mut self, module: &AirModule) {
        if matches!(module.policy.max_tool_calls, Some(0)) {
            self.error("AIR050", "policy.max_tool_calls must be at least 1");
        }

        if matches!(module.policy.max_model_calls, Some(0)) {
            self.error("AIR052", "policy.max_model_calls must be at least 1");
        }

        if matches!(module.policy.max_repeated_tool_calls, Some(0)) {
            self.error(
                "AIR053",
                "policy.max_repeated_tool_calls must be at least 1",
            );
        }

        if matches!(module.policy.timeout_seconds, Some(0)) {
            self.error("AIR051", "policy.timeout_seconds must be at least 1");
        }
    }

    fn error(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(code, message));
    }
}

fn is_known_runtime_context_field(field: &str) -> bool {
    matches!(
        field,
        "step"
            | "step_number"
            | "max_steps"
            | "remaining_steps"
            | "is_last_step"
            | "is_last_action_step"
    )
}

fn enum_values(spec: Option<&TypeSpec>) -> Option<BTreeSet<String>> {
    let Some(TypeSpec::Detailed(detailed)) = spec else {
        return None;
    };

    if detailed.kind != DetailedTypeKind::Enum {
        return None;
    }

    Some(detailed.enum_values.iter().cloned().collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateMachineApprovalState {
    phase: String,
    approved: BTreeSet<String>,
}

impl StateMachineApprovalState {
    fn key(&self) -> (String, Vec<String>) {
        (
            self.phase.clone(),
            self.approved.iter().cloned().collect::<Vec<_>>(),
        )
    }
}

fn condition_may_match_phase(condition: &str, phase: &str) -> bool {
    condition.split("||").any(|group| {
        group
            .split("&&")
            .all(|clause| condition_clause_may_match_phase(clause.trim(), phase))
    })
}

fn condition_clause_may_match_phase(clause: &str, phase: &str) -> bool {
    let (left, right, expected_equal) = if let Some((left, right)) = clause.split_once("==") {
        (left, right, true)
    } else if let Some((left, right)) = clause.split_once("!=") {
        (left, right, false)
    } else {
        return true;
    };

    if normalize_path(left.trim()) != "phase" {
        return true;
    }

    let Some(expected) = parse_condition_literal(right.trim()) else {
        return true;
    };
    let serde_json::Value::String(expected_phase) = expected else {
        return true;
    };

    (expected_phase == phase) == expected_equal
}

fn parse_condition_literal(raw: &str) -> Option<serde_json::Value> {
    if raw.is_empty() {
        return None;
    }
    if (raw.starts_with('"') && raw.ends_with('"'))
        || (raw.starts_with('\'') && raw.ends_with('\''))
    {
        return Some(serde_json::Value::String(raw[1..raw.len() - 1].to_string()));
    }
    serde_json::from_str(raw).ok()
}

fn next_phases_after_rule(
    rule: &StateRule,
    current_phase: &str,
    phases: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut next = None;
    for action in &rule.actions {
        if let StateAction::Set { values } = action {
            if let Some(value) = values.get("phase") {
                next = match value {
                    serde_json::Value::String(phase) => Some(BTreeSet::from_iter([phase.clone()])),
                    _ => Some(phases.clone()),
                };
            }
        }
    }
    next.unwrap_or_else(|| BTreeSet::from_iter([current_phase.to_string()]))
}

fn module_field_type<'a>(module: &'a AirModule, field: &str) -> Option<&'a TypeSpec> {
    module
        .inputs
        .get(field)
        .or_else(|| module.state.get(field))
        .or_else(|| module.outputs.get(field))
}

fn nested_property_type<'a>(spec: &'a TypeSpec, segment: &str) -> Option<&'a TypeSpec> {
    let TypeSpec::Detailed(detailed) = spec else {
        return None;
    };

    match detailed.kind {
        DetailedTypeKind::Object => detailed.properties.get(segment),
        DetailedTypeKind::Array => {
            if segment.parse::<usize>().is_ok() {
                detailed.items.as_deref()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn type_has_known_properties(spec: &TypeSpec) -> bool {
    match spec {
        TypeSpec::Detailed(detailed) => match detailed.kind {
            DetailedTypeKind::Object => {
                !detailed.properties.is_empty() || !detailed.additional_properties
            }
            DetailedTypeKind::Array => detailed.items.is_some(),
            _ => false,
        },
        TypeSpec::Shorthand(_) => false,
    }
}

fn is_array_type(spec: Option<&TypeSpec>) -> bool {
    matches!(
        spec,
        Some(TypeSpec::Detailed(detailed))
            if detailed.kind == air_core::DetailedTypeKind::Array
    )
}

fn template_paths(template: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            break;
        };

        let path = after_start[..end].trim();
        if !path.is_empty() {
            paths.push(path.to_string());
        }
        rest = &after_start[end + 2..];
    }

    paths
}
